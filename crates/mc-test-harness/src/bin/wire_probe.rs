//! Wire-capture probe.
//!
//! Connects to a Minecraft Java Edition server, walks the protocol
//! through Handshake → Login → Configuration → Play (offline-mode),
//! and dumps every received clientbound frame. Used to validate
//! Solaris' guessed packet IDs against a real vanilla server.
//!
//! Usage:
//!   wire-probe --addr 127.0.0.1:25566 [--name probe] [--ticks 10]

use std::time::Duration;

use anyhow::{Context, Result};
use bytes::BytesMut;
use clap::Parser;
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::codec::ReadMc;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, ServerboundKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "wire-probe",
    about = "Dump clientbound frames from an MC server"
)]
struct Cli {
    /// Address of the target server, e.g. 127.0.0.1:25566.
    #[arg(long)]
    addr: String,
    /// Player name to send in LoginStart.
    #[arg(long, default_value = "probe")]
    name: String,
    /// How many seconds to keep reading clientbound frames after Play
    /// state is reached. Default 5s — long enough to see the spawn
    /// burst, short enough not to wait for the first keepalive.
    #[arg(long, default_value_t = 5)]
    play_seconds: u64,
}

fn hexdump_short(bytes: &[u8], max: usize) -> String {
    let display = &bytes[..bytes.len().min(max)];
    let mut s = String::new();
    for b in display {
        s.push_str(&format!("{b:02x} "));
    }
    if bytes.len() > max {
        s.push('…');
    }
    s.trim_end().to_string()
}

/// State for tracking the connection.
struct Probe {
    stream: TcpStream,
    rbuf: BytesMut,
    compression: Compression,
}

impl Probe {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            rbuf: BytesMut::with_capacity(8192),
            compression: Compression::Disabled,
        }
    }

    async fn write_packet<P: Packet>(&mut self, packet: &P) -> Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        let framed = encode_frame(P::ID, &body, self.compression)?;
        self.stream.write_all(&framed).await?;
        Ok(())
    }

    /// Read a single frame, blocking on the socket as needed.
    async fn read_frame(&mut self) -> Result<mc_protocol::RawFrame> {
        loop {
            if let Some(frame) = try_decode_frame(&mut self.rbuf, self.compression)? {
                return Ok(frame);
            }
            let n = self.stream.read_buf(&mut self.rbuf).await?;
            if n == 0 {
                anyhow::bail!("peer closed");
            }
        }
    }

    /// Read whatever frames arrive in `duration`, dumping each.
    async fn dump_for(&mut self, label: &str, duration: Duration) -> Result<usize> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut count = 0;
        loop {
            let timeout = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                self.read_frame(),
            )
            .await;
            match timeout {
                Ok(Ok(frame)) => {
                    println!(
                        "[{label}] id=0x{:02X} body_len={} body={}",
                        frame.id,
                        frame.body.len(),
                        hexdump_short(&frame.body, 48)
                    );
                    // If a Set Compression packet is implied (very small
                    // body, id matching login.0x03), enable compression
                    // for subsequent frames. We don't try to be smart
                    // here — we just detect the marker pattern. The
                    // server-state machine is more reliable, but this
                    // probe is a minimum viable wire dumper.
                    count += 1;
                }
                Ok(Err(e)) => {
                    println!("[{label}] read ended: {e}");
                    return Ok(count);
                }
                Err(_) => {
                    return Ok(count);
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    let cli = Cli::parse();
    let stream = TcpStream::connect(&cli.addr)
        .await
        .with_context(|| format!("connect to {}", cli.addr))?;
    stream.set_nodelay(true)?;
    let mut probe = Probe::new(stream);

    let port = cli
        .addr
        .rsplit_once(':')
        .map(|(_, p)| p.parse().unwrap_or(25565))
        .unwrap_or(25565);

    // Handshake → Login
    probe
        .write_packet(&Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: port,
            next_state: NextState::Login,
        })
        .await?;

    probe
        .write_packet(&LoginStart {
            name: cli.name.clone(),
            player_uuid: Uuid::nil(),
        })
        .await?;

    println!("=== LOGIN STATE ===");
    println!("(reading frames until we see a known terminal packet)");

    // Read up to a small fixed budget of login frames, looking for
    // LoginSuccess. SetCompression may arrive first and enable
    // compression for all subsequent frames including LoginSuccess
    // itself.
    loop {
        let frame = probe.read_frame().await?;
        println!(
            "[LOGIN] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        );

        // Set Compression has id 0x03 in modern protocol and body is a
        // single VarInt. Detect this heuristically and flip our codec.
        if frame.id == 0x03
            && frame.body.len() <= 5
            && let Ok(threshold) = frame.body.clone().read_varint()
        {
            println!(
                "    ↳ looks like SetCompression(threshold={threshold}); enabling compression"
            );
            probe.compression = if threshold < 0 {
                Compression::Disabled
            } else {
                Compression::Threshold(threshold as usize)
            };
            continue;
        }

        // LoginSuccess detection: try to decode and if it has the right
        // shape, ack.
        if frame.id == LoginSuccess::ID {
            let mut body = frame.body.clone();
            if let Ok(success) = LoginSuccess::decode(&mut body) {
                println!(
                    "    ↳ LoginSuccess: name={} uuid={} properties={}",
                    success.name,
                    success.uuid,
                    success.properties.len()
                );
                probe.write_packet(&LoginAcknowledged).await?;
                break;
            }
        }

        // Disconnect/anything-unexpected: dump and bail.
        if frame.id == 0x00 {
            let preview = String::from_utf8_lossy(&frame.body);
            println!("    ↳ possible LoginDisconnect: {preview}");
            return Ok(());
        }
    }

    println!();
    println!("=== CONFIGURATION STATE ===");

    // Wait for ClientboundKnownPacks, echo back, then drain RegistryData
    // / UpdateTags / FinishConfiguration. Send Ack when we see Finish.
    let mut sent_known_packs_response = false;
    let mut got_finish = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !got_finish {
        let frame = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            probe.read_frame(),
        )
        .await
        .context("timed out waiting for Configuration frames")??;

        println!(
            "[CONFIG] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        );

        // Heuristic: ClientboundKnownPacks has a non-trivial body
        // (starts with a VarInt count > 0 and then strings). We try
        // a typed decode first.
        if !sent_known_packs_response
            && let Ok(known) = ClientboundKnownPacks::decode(&mut frame.body.clone())
            && !known.packs.is_empty()
            && known.packs[0]
                .namespace
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_')
        {
            println!(
                "    ↳ ClientboundKnownPacks (id 0x{:02X}): {:?}",
                frame.id, known.packs
            );
            probe
                .write_packet(&ServerboundKnownPacks {
                    packs: known.packs.clone(),
                })
                .await?;
            sent_known_packs_response = true;
            continue;
        }

        // FinishConfiguration is empty body. The first empty-body
        // frame we see after KnownPacks is almost certainly it.
        if frame.body.is_empty() && sent_known_packs_response {
            println!(
                "    ↳ assumed FinishConfiguration (id 0x{:02X}); sending Ack",
                frame.id
            );
            probe.write_packet(&AcknowledgeFinishConfiguration).await?;
            got_finish = true;
        }
    }

    println!();
    println!("=== PLAY STATE (reading for {}s) ===", cli.play_seconds);
    let n = probe
        .dump_for("PLAY", Duration::from_secs(cli.play_seconds))
        .await?;
    println!("captured {n} Play-state frames");

    Ok(())
}
