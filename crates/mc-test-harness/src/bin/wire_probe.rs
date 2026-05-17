//! Wire-capture probe.
//!
//! Connects to a Minecraft Java Edition server, walks the protocol
//! through Handshake → Login → Configuration → Play (offline-mode),
//! and dumps every received clientbound frame. Used to validate
//! Solaris' guessed packet IDs against a real vanilla server.
//!
//! Usage:
//!   wire-probe --addr 127.0.0.1:25566 [--name probe] [--play-seconds 10]
//!   wire-probe --addr 127.0.0.1:25565 --server-kind vanilla \
//!     --scenario player-water-entry --out .analysis/physics-oracles/player-water-entry.vanilla.capture

use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, LoginPlay, MovePlayerFlags, PlayerCommandAction,
    PlayerInput, ServerboundKeepAlive, ServerboundMovePlayerPosRot, ServerboundPlayerCommand,
    ServerboundPlayerInput, SynchronizePlayerPosition,
};
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
    /// Stable scenario name written into captures.
    #[arg(long, default_value = "spawn-burst")]
    scenario: String,
    /// Server kind written into captures: vanilla, solaris, or local.
    #[arg(long, default_value = "local")]
    server_kind: String,
    /// Optional capture output path. Use a gitignored path such as
    /// .analysis/physics-oracles/<scenario>.<server-kind>.capture.
    #[arg(long)]
    out: Option<PathBuf>,
}

struct CaptureWriter {
    file: Option<File>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeScenario {
    Passive,
    PlayerShallowWaterEntry,
}

impl ProbeScenario {
    fn from_name(name: &str) -> Self {
        match name {
            "player-shallow-water-entry" => Self::PlayerShallowWaterEntry,
            _ => Self::Passive,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PlayStart {
    entity_id: i32,
    sync: SynchronizePlayerPosition,
}

fn log_frame(
    capture: &mut CaptureWriter,
    label: &str,
    frame: &mc_protocol::RawFrame,
) -> Result<()> {
    capture.line(format!(
        "[{label}] id=0x{:02X} body_len={} body={}",
        frame.id,
        frame.body.len(),
        hexdump_short(&frame.body, 48)
    ))?;
    Ok(())
}

impl CaptureWriter {
    fn new(path: Option<PathBuf>) -> Result<Self> {
        let file = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("create capture directory {}", parent.display())
                    })?;
                }
                Some(
                    File::create(&path)
                        .with_context(|| format!("create capture {}", path.display()))?,
                )
            }
            None => None,
        };
        Ok(Self { file })
    }

    fn line(&mut self, line: impl AsRef<str>) -> io::Result<()> {
        let line = line.as_ref();
        println!("{line}");
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
        }
        Ok(())
    }
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

    async fn write_packet_logged<P: Packet>(
        &mut self,
        packet: &P,
        state: &str,
        name: &str,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        let mut body = BytesMut::new();
        packet.encode(&mut body)?;
        capture.line(format!(
            "[{state}->SERVER:{name}] id=0x{:02X} body_len={} body={}",
            P::ID,
            body.len(),
            hexdump_short(&body, 48)
        ))?;
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
    async fn dump_for(
        &mut self,
        label: &str,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<usize> {
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
                    log_frame(capture, label, &frame)?;
                    if label == "PLAY" && frame.id == ClientboundKeepAlive::ID {
                        let mut body = frame.body.clone();
                        let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                        self.write_packet_logged(
                            &ServerboundKeepAlive { id: keepalive.id },
                            "PLAY",
                            "KeepAlive",
                            capture,
                        )
                        .await?;
                    }
                    // If a Set Compression packet is implied (very small
                    // body, id matching login.0x03), enable compression
                    // for subsequent frames. We don't try to be smart
                    // here — we just detect the marker pattern. The
                    // server-state machine is more reliable, but this
                    // probe is a minimum viable wire dumper.
                    count += 1;
                }
                Ok(Err(e)) => {
                    capture.line(format!("[{label}] read ended: {e}"))?;
                    return Ok(count);
                }
                Err(_) => {
                    return Ok(count);
                }
            }
        }
    }

    async fn dump_until_play_start(
        &mut self,
        duration: Duration,
        capture: &mut CaptureWriter,
    ) -> Result<PlayStart> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut entity_id = None;
        loop {
            let frame = tokio::time::timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
                self.read_frame(),
            )
            .await
            .context("timed out waiting for Play start frames")??;
            log_frame(capture, "PLAY", &frame)?;
            if frame.id == ClientboundKeepAlive::ID {
                let mut body = frame.body.clone();
                let keepalive = ClientboundKeepAlive::decode(&mut body)?;
                self.write_packet_logged(
                    &ServerboundKeepAlive { id: keepalive.id },
                    "PLAY",
                    "KeepAlive",
                    capture,
                )
                .await?;
                continue;
            }
            if frame.id == LoginPlay::ID {
                let login = LoginPlay::decode(&mut frame.body.clone())?;
                capture.line(format!(
                    "    ↳ LoginPlay: entity_id={} dimension={} game_mode={}",
                    login.entity_id, login.dimension_name, login.game_mode
                ))?;
                entity_id = Some(login.entity_id);
                continue;
            }
            if frame.id == SynchronizePlayerPosition::ID {
                let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())?;
                let entity_id = entity_id.unwrap_or(0);
                if entity_id == 0 {
                    capture.line("    ↳ LoginPlay entity id not observed before Sync; using 0")?;
                }
                return Ok(PlayStart { entity_id, sync });
            }
        }
    }

    async fn run_player_shallow_water_entry(
        &mut self,
        play_seconds: u64,
        capture: &mut CaptureWriter,
    ) -> Result<()> {
        capture.line("=== SCRIPT player-shallow-water-entry ===")?;
        let start = self
            .dump_until_play_start(Duration::from_secs(play_seconds.max(5)), capture)
            .await?;
        self.write_packet_logged(
            &ConfirmTeleportation {
                teleport_id: start.sync.teleport_id,
            },
            "PLAY",
            "ConfirmTeleportation",
            capture,
        )
        .await?;

        let movement_flags = MovePlayerFlags::new(true, false);
        let water_steps = [
            ServerboundMovePlayerPosRot {
                x: -4.8,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
                flags: movement_flags,
            },
            ServerboundMovePlayerPosRot {
                x: -4.2,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: -15.0,
                flags: MovePlayerFlags::new(false, false),
            },
        ];
        for step in water_steps {
            self.write_packet_logged(&step, "PLAY", "MovePlayerPosRot", capture)
                .await?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.write_packet_logged(
            &ServerboundPlayerCommand {
                entity_id: start.entity_id,
                action: PlayerCommandAction::StartSprinting,
                data: 0,
            },
            "PLAY",
            "PlayerCommand(StartSprinting)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundPlayerInput {
                input: PlayerInput {
                    forward: true,
                    jump: true,
                    sprint: true,
                    ..PlayerInput::default()
                },
            },
            "PLAY",
            "PlayerInput(forward+jump+sprint)",
            capture,
        )
        .await?;
        self.write_packet_logged(
            &ServerboundMovePlayerPosRot {
                x: -3.6,
                y: 64.0,
                z: 0.5,
                yaw: 90.0,
                pitch: -15.0,
                flags: MovePlayerFlags::new(false, false),
            },
            "PLAY",
            "MovePlayerPosRot(swim-look)",
            capture,
        )
        .await?;

        let n = self
            .dump_for("PLAY", Duration::from_secs(play_seconds.max(3)), capture)
            .await?;
        capture.line(format!("script captured {n} post-input Play-state frames"))?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();
    let cli = Cli::parse();
    let mut capture = CaptureWriter::new(cli.out.clone())?;
    let captured_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time before Unix epoch")?
        .as_millis();
    capture.line("# solaris-wire-capture-v1")?;
    capture.line(format!("scenario={}", cli.scenario))?;
    capture.line(format!("server_kind={}", cli.server_kind))?;
    capture.line(format!("addr={}", cli.addr))?;
    capture.line(format!("player={}", cli.name))?;
    capture.line(format!("protocol_version={PROTOCOL_VERSION}"))?;
    capture.line(format!("play_seconds={}", cli.play_seconds))?;
    capture.line(format!("captured_at_unix_ms={captured_at}"))?;
    capture.line(
        "packet_source=.analysis/protocol-dump.txt or javap net.minecraft.network.protocol.*",
    )?;
    capture.line("")?;
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
        .write_packet_logged(
            &Handshake {
                protocol_version: PROTOCOL_VERSION,
                server_address: "127.0.0.1".into(),
                server_port: port,
                next_state: NextState::Login,
            },
            "HANDSHAKE",
            "Handshake(Login)",
            &mut capture,
        )
        .await?;

    probe
        .write_packet_logged(
            &LoginStart {
                name: cli.name.clone(),
                player_uuid: Uuid::nil(),
            },
            "LOGIN",
            "LoginStart",
            &mut capture,
        )
        .await?;

    capture.line("=== LOGIN STATE ===")?;
    capture.line("(reading frames until we see a known terminal packet)")?;

    // Read up to a small fixed budget of login frames, looking for
    // LoginSuccess. SetCompression may arrive first and enable
    // compression for all subsequent frames including LoginSuccess
    // itself.
    loop {
        let frame = probe.read_frame().await?;
        capture.line(format!(
            "[LOGIN] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        ))?;

        // Set Compression has id 0x03 in modern protocol and body is a
        // single VarInt. Detect this heuristically and flip our codec.
        if frame.id == 0x03
            && frame.body.len() <= 5
            && let Ok(threshold) = frame.body.clone().read_varint()
        {
            capture.line(format!(
                "    ↳ looks like SetCompression(threshold={threshold}); enabling compression"
            ))?;
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
                capture.line(format!(
                    "    ↳ LoginSuccess: name={} uuid={} properties={}",
                    success.name,
                    success.uuid,
                    success.properties.len()
                ))?;
                probe
                    .write_packet_logged(
                        &LoginAcknowledged,
                        "LOGIN",
                        "LoginAcknowledged",
                        &mut capture,
                    )
                    .await?;
                break;
            }
        }

        // Disconnect/anything-unexpected: dump and bail.
        if frame.id == 0x00 {
            let preview = String::from_utf8_lossy(&frame.body);
            capture.line(format!("    ↳ possible LoginDisconnect: {preview}"))?;
            return Ok(());
        }
    }

    capture.line("")?;
    capture.line("=== CONFIGURATION STATE ===")?;

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

        capture.line(format!(
            "[CONFIG] id=0x{:02X} body_len={} body={}",
            frame.id,
            frame.body.len(),
            hexdump_short(&frame.body, 48)
        ))?;

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
            capture.line(format!(
                "    ↳ ClientboundKnownPacks (id 0x{:02X}): {:?}",
                frame.id, known.packs
            ))?;
            probe
                .write_packet_logged(
                    &ServerboundKnownPacks {
                        packs: known.packs.clone(),
                    },
                    "CONFIG",
                    "KnownPacks",
                    &mut capture,
                )
                .await?;
            sent_known_packs_response = true;
            continue;
        }

        // FinishConfiguration is empty body. The first empty-body
        // frame we see after KnownPacks is almost certainly it.
        if frame.body.is_empty() && sent_known_packs_response {
            capture.line(format!(
                "    ↳ assumed FinishConfiguration (id 0x{:02X}); sending Ack",
                frame.id
            ))?;
            probe
                .write_packet_logged(
                    &AcknowledgeFinishConfiguration,
                    "CONFIG",
                    "AcknowledgeFinishConfiguration",
                    &mut capture,
                )
                .await?;
            got_finish = true;
        }
    }

    capture.line("")?;
    capture.line(format!(
        "=== PLAY STATE (reading for {}s) ===",
        cli.play_seconds
    ))?;
    match ProbeScenario::from_name(&cli.scenario) {
        ProbeScenario::Passive => {
            let n = probe
                .dump_for("PLAY", Duration::from_secs(cli.play_seconds), &mut capture)
                .await?;
            capture.line(format!("captured {n} Play-state frames"))?;
        }
        ProbeScenario::PlayerShallowWaterEntry => {
            probe
                .run_player_shallow_water_entry(cli.play_seconds, &mut capture)
                .await?;
        }
    }

    Ok(())
}
