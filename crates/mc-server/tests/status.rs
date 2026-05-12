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
use tokio::net::TcpStream;

/// Bind to port 0, read back the assigned port, then start the server on
/// the freshly-known address. We can't ask `mc_net::run` for the port
/// after the fact because it owns the listener internally; instead we
/// peek the address before launching the server with the real config.
async fn start_server(motd: &str) -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: motd.to_string(),
        max_players: 17,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks: std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        world: None,
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, Compression::Disabled).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn read_one_frame(stream: &mut TcpStream, buf: &mut BytesMut) -> mc_protocol::RawFrame {
    loop {
        if let Some(frame) = try_decode_frame(buf, Compression::Disabled).unwrap() {
            return frame;
        }
        let read = stream.read_buf(buf).await.unwrap();
        assert!(read > 0, "server closed before sending a complete frame");
    }
}

#[tokio::test]
async fn handshake_status_ping_round_trip() {
    let addr = start_server("Hello from Solaris").await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);

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
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
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
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, PongResponse::ID);
    let pong = PongResponse::decode(&mut frame.body).unwrap();
    assert_eq!(pong.payload, 0xDEAD_BEEF);

    // 5. Server should close the connection (read returns 0). Give it a
    //    moment, then assert.
    let mut scratch = [0u8; 1];
    let n = stream.read(&mut scratch).await.unwrap_or(0);
    assert_eq!(n, 0, "expected the server to close after pong");
}
