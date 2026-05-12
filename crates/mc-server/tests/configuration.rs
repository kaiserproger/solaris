//! End-to-end test for the M1.e Configuration state.
//!
//! Drives a raw TCP client all the way from Handshake through the
//! Configuration handshake (Known Packs round trip + Finish/Ack) and
//! asserts the server drops the connection after acknowledgement —
//! because Play state is M1.g, not implemented yet.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::TARGET_RELEASE;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, KnownPackEntry,
    ServerboundKnownPacks,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

async fn pick_ephemeral_port() -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    addr
}

async fn start_server() -> SocketAddr {
    let addr = pick_ephemeral_port().await;
    let cfg = mc_net::ServerConfig {
        bind_address: addr,
        motd: "M1.e config".into(),
        max_players: 8,
    };
    tokio::spawn(async move {
        let _ = mc_net::run(cfg).await;
    });
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not start listening on {addr}");
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, Compression::Disabled).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn read_one_frame(stream: &mut TcpStream) -> mc_protocol::RawFrame {
    let mut buf = BytesMut::with_capacity(4096);
    loop {
        if let Some(frame) = try_decode_frame(&mut buf, Compression::Disabled).unwrap() {
            return frame;
        }
        let read = stream.read_buf(&mut buf).await.unwrap();
        assert!(read > 0, "server closed before sending a complete frame");
    }
}

/// Walk the protocol up to (and including) `LoginAcknowledged`, leaving
/// the client sitting in Configuration state.
async fn run_through_login_ack(stream: &mut TcpStream, addr: SocketAddr, name: &str) {
    write_frame(
        stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
    )
    .await;
    write_frame(
        stream,
        &LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        },
    )
    .await;
    let mut frame = read_one_frame(stream).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let _ = LoginSuccess::decode(&mut frame.body).unwrap();
    write_frame(stream, &LoginAcknowledged).await;
}

#[tokio::test]
async fn configuration_known_packs_and_finish_complete() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    run_through_login_ack(&mut stream, addr, "Notch").await;

    // First Configuration packet from the server: Known Packs.
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(
        frame.id,
        ClientboundKnownPacks::ID,
        "expected Known Packs from server first"
    );
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(known.packs.len(), 1, "expected exactly one advertised pack");
    let pack = &known.packs[0];
    assert_eq!(pack.namespace, "minecraft");
    assert_eq!(pack.id, "core");
    assert_eq!(pack.version, TARGET_RELEASE);

    // Echo it back as "yes, I also have this pack".
    write_frame(
        &mut stream,
        &ServerboundKnownPacks {
            packs: known.packs.clone(),
        },
    )
    .await;

    // Server should now send Finish Configuration directly (we don't
    // emit Registry Data in M1.e).
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(
        frame.id,
        FinishConfiguration::ID,
        "expected Finish Configuration"
    );
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();

    // Acknowledge — that transitions us into Play, which the server
    // does not implement, so it should close.
    write_frame(&mut stream, &AcknowledgeFinishConfiguration).await;
    let mut scratch = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut scratch))
        .await
        .expect("server did not close on its own within 2s")
        .unwrap_or(0);
    assert_eq!(
        n, 0,
        "expected close after Acknowledge Finish Configuration"
    );
}

#[tokio::test]
async fn configuration_skips_unexpected_packets() {
    // The handler must tolerate packets the client sends in Configuration
    // state that aren't `Serverbound Known Packs` yet (e.g. Client
    // Information, plugin messages). We emulate one such packet by
    // writing a frame with an arbitrary packet ID before sending the
    // expected response.
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    run_through_login_ack(&mut stream, addr, "Steve").await;

    // Consume the server's Known Packs.
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let _ = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    // Send a "Client Information"-shaped junk frame with serverbound id
    // 0x00 in Configuration. The handler should ignore it.
    let junk = encode_frame(0x00, &[0u8; 4], Compression::Disabled).unwrap();
    stream.write_all(&junk).await.unwrap();

    // Now send the real Known Packs response.
    write_frame(
        &mut stream,
        &ServerboundKnownPacks {
            packs: vec![KnownPackEntry {
                namespace: "minecraft".into(),
                id: "core".into(),
                version: TARGET_RELEASE.into(),
            }],
        },
    )
    .await;

    // Expect Finish Configuration to follow.
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();

    // Send another junk frame before the ack — handler should ignore it
    // too while waiting for AcknowledgeFinishConfiguration.
    let junk = encode_frame(0x02, &[1, 2, 3], Compression::Disabled).unwrap();
    stream.write_all(&junk).await.unwrap();

    write_frame(&mut stream, &AcknowledgeFinishConfiguration).await;
    let mut scratch = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut scratch))
        .await
        .expect("server did not close on its own within 2s")
        .unwrap_or(0);
    assert_eq!(n, 0);
}
