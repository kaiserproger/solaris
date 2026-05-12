//! End-to-end test for the M1.c Handshake → Status → Ping → Pong dance.
//!
//! Boots a real `mc_net::run` task bound to an ephemeral port, then
//! drives a raw TCP client through the vanilla server-list-ping
//! choreography and asserts that the JSON the server returns parses, has
//! the right shape, and that the pong echoes our ping payload.
//!
//! This is the bar of M1.c: a vanilla 26.1.x client (and any standard
//! `mcstatus`-style tool) sees the server in the list.

use std::net::SocketAddr;

use bytes::{Buf, BytesMut};
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::status::{PingRequest, PongResponse, StatusRequest, StatusResponse};
use mc_protocol::{PROTOCOL_VERSION, TARGET_RELEASE};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Bind to port 0, read back the assigned port, then start the server on
/// the freshly-known address. We can't ask `mc_net::run` for the port
/// after the fact because it owns the listener internally; instead we
/// peek the address before launching the server with the real config.
async fn pick_ephemeral_port() -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    addr
}

async fn start_server(motd: &str) -> SocketAddr {
    let addr = pick_ephemeral_port().await;
    let cfg = mc_net::ServerConfig {
        bind_address: addr,
        motd: motd.to_string(),
        max_players: 17,
    };
    tokio::spawn(async move {
        let _ = mc_net::run(cfg).await;
    });
    // Spin until the listener is up. The race window is tiny but real.
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
async fn handshake_status_ping_round_trip() {
    let addr = start_server("Hello from Solaris").await;
    let mut stream = TcpStream::connect(addr).await.unwrap();

    // 1. Handshake (next_state = Status).
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Status,
        },
    )
    .await;

    // 2. Empty status request.
    write_frame(&mut stream, &StatusRequest).await;

    // 3. Read status response, decode JSON.
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(frame.id, StatusResponse::ID);
    let response = StatusResponse::decode(&mut frame.body).unwrap();
    assert!(frame.body.remaining() == 0);
    let json: serde_json::Value = serde_json::from_str(&response.json).unwrap();
    assert_eq!(json["version"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(json["version"]["name"], TARGET_RELEASE);
    assert_eq!(json["description"]["text"], "Hello from Solaris");
    assert_eq!(json["players"]["max"], 17);

    // 4. Ping → Pong.
    write_frame(
        &mut stream,
        &PingRequest {
            payload: 0xDEAD_BEEF,
        },
    )
    .await;
    let mut frame = read_one_frame(&mut stream).await;
    assert_eq!(frame.id, PongResponse::ID);
    let pong = PongResponse::decode(&mut frame.body).unwrap();
    assert_eq!(pong.payload, 0xDEAD_BEEF);

    // 5. Server should close the connection (read returns 0). Give it a
    //    moment, then assert.
    let mut scratch = [0u8; 1];
    let n = stream.read(&mut scratch).await.unwrap_or(0);
    assert_eq!(n, 0, "expected the server to close after pong");
}
