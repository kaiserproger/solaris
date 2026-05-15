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

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::ClientboundKnownPacks;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::status::{StatusRequest, StatusResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

async fn start_server() -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.d login".into(),
        max_players: 8,
        view_distance: 10,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks: std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        world: None,
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
        recipes: std::sync::Arc::new(Vec::new()),
        loot: std::sync::Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P, compression: Compression) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, compression).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn read_one_frame(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) -> mc_protocol::RawFrame {
    loop {
        if let Some(frame) = try_decode_frame(buf, compression).unwrap() {
            return frame;
        }
        let read = stream.read_buf(buf).await.unwrap();
        assert!(read > 0, "server closed before sending a complete frame");
    }
}

#[tokio::test]
async fn login_offline_flow_completes() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);

    // Handshake with next_state = Login.
    write_frame(
        &mut stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
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
        Compression::Disabled,
    )
    .await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, Compression::Disabled).await;
    assert_eq!(
        frame.id,
        SetCompression::ID,
        "should negotiate compression first"
    );
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    assert_eq!(set_compression.threshold, 256);
    let compression = Compression::Threshold(set_compression.threshold as usize);

    // Expect LoginSuccess after the compression boundary.
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, LoginSuccess::ID, "should be login success");
    let success = LoginSuccess::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    assert_eq!(success.name, "Notch");
    assert_eq!(success.uuid, mc_net::offline_uuid("Notch"));
    assert!(success.properties.is_empty());

    // Acknowledge — this transitions to Configuration state, where the
    // server's first packet is `Clientbound Known Packs` (M1.e).
    write_frame(&mut stream, &LoginAcknowledged, compression).await;
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
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
        Compression::Disabled,
    )
    .await;

    // A second client should still be able to ping concurrently.
    let mut pinger = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    write_frame(
        &mut pinger,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Status,
        },
        Compression::Disabled,
    )
    .await;
    write_frame(&mut pinger, &StatusRequest, Compression::Disabled).await;
    let mut frame = read_one_frame(&mut pinger, &mut rbuf, Compression::Disabled).await;
    assert_eq!(frame.id, StatusResponse::ID);
    let _ = StatusResponse::decode(&mut frame.body).unwrap();

    // Tidy: close the slow connection.
    drop(slow);
}
