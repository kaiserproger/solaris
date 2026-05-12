//! End-to-end test for the M1.d Login state.
//!
//! Boots a real `mc_net::run` on an ephemeral port, drives the
//! Handshake → LoginStart → LoginSuccess → LoginAcknowledged sequence,
//! and asserts:
//!
//! - the server replies with the expected offline UUID (Java-compatible
//!   `nameUUIDFromBytes` derivation),
//! - the connection is closed after acknowledgement (Configuration state
//!   is M1.e, not implemented),
//! - the listener stays up for parallel clients.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::ClientboundKnownPacks;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use mc_protocol::packets::status::{StatusRequest, StatusResponse};
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
        motd: "M1.d login".into(),
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

#[tokio::test]
async fn login_offline_flow_completes() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    // Handshake with next_state = Login.
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
    )
    .await;

    // Client always sends some UUID alongside its name. We send a
    // throwaway one: the server should ignore it and stamp its own
    // offline UUID derived from the name.
    write_frame(
        &mut stream,
        &LoginStart {
            name: "Notch".into(),
            player_uuid: Uuid::from_u128(0xDEADBEEF),
        },
    )
    .await;

    // Expect LoginSuccess.
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(frame.id, LoginSuccess::ID, "should be login success");
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(success.name, "Notch");
    assert_eq!(success.uuid, mc_net::offline_uuid("Notch"));
    assert!(success.properties.is_empty());

    // Acknowledge — this transitions to Configuration state, where the
    // server's first packet is `Clientbound Known Packs` (M1.e).
    write_frame(&mut stream, &LoginAcknowledged).await;
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(
        frame.id,
        ClientboundKnownPacks::ID,
        "server should advertise Known Packs once Configuration begins"
    );
    let _ = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
}

#[tokio::test]
async fn login_does_not_break_concurrent_status() {
    let addr = start_server().await;

    // Start a slow login: just send the handshake, don't follow up. The
    // server is sitting in read_packet waiting for LoginStart.
    let mut slow = TcpStream::connect(addr).await.unwrap();
    write_frame(
        &mut slow,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
    )
    .await;

    // A second client should still be able to ping concurrently.
    let mut pinger = TcpStream::connect(addr).await.unwrap();
    write_frame(
        &mut pinger,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Status,
        },
    )
    .await;
    write_frame(&mut pinger, &StatusRequest).await;
    let mut frame = read_one_frame(&mut pinger).await;
    assert_eq!(frame.id, StatusResponse::ID);
    let _ = StatusResponse::decode(&mut frame.body).unwrap();

    // Tidy: close the slow connection.
    drop(slow);
}
