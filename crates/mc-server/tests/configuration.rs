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
    RegistryData, ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess};
use mc_protocol::packets::play::LoginPlay;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

async fn start_server() -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.e config".into(),
        max_players: 8,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks: std::sync::Arc::new(
            mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
        ),
        world: None,
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
        block_light: None,
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

/// Walk the protocol up to (and including) `LoginAcknowledged`, leaving
/// the client sitting in Configuration state.
async fn run_through_login_ack(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    addr: SocketAddr,
    name: &str,
) {
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
    let mut frame = read_one_frame(stream, buf).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let _ = LoginSuccess::decode(&mut frame.body).unwrap();
    write_frame(stream, &LoginAcknowledged).await;
}

#[tokio::test]
async fn configuration_known_packs_and_finish_complete() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    run_through_login_ack(&mut stream, &mut rbuf, addr, "Notch").await;

    // First Configuration packet from the server: Known Packs.
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
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

    // Server now sends Registry Data — one packet per registry. With
    // the test stub each registry has 2 entries with has_data=false.
    let stub_registry_count = mc_data::KNOWN_REGISTRIES.len();
    let mut seen_dimension_type = false;
    for _ in 0..stub_registry_count {
        let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
        assert_eq!(
            frame.id,
            RegistryData::ID,
            "expected Registry Data, got {:#04x}",
            frame.id
        );
        let registry = RegistryData::decode(&mut frame.body).unwrap();
        assert_eq!(frame.body.remaining(), 0);
        assert_eq!(
            registry.entries.len(),
            2,
            "stub registry {} should have 2 entries",
            registry.registry_id
        );
        // Every entry must declare `has_data = false` — the protocol
        // path that needs NBT payload encoding is intentionally not
        // exercised yet.
        for entry in &registry.entries {
            assert!(
                entry.nbt_payload.is_none(),
                "stub should emit only has_data=false entries"
            );
        }
        if registry.registry_id.as_str() == "minecraft:dimension_type" {
            seen_dimension_type = true;
        }
    }
    assert!(
        seen_dimension_type,
        "expected a Registry Data packet for minecraft:dimension_type"
    );

    // M3.i: server emits Update Tags between the last Registry Data
    // and Finish Configuration. The test stub has an empty tag set so
    // the packet is byte-minimal (one VarInt(0)) but it must still
    // appear on the wire.
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(
        frame.id,
        UpdateTags::ID,
        "expected Update Tags before Finish Configuration"
    );
    let update_tags = UpdateTags::decode(&mut frame.body).unwrap();
    assert!(
        update_tags.registries.is_empty(),
        "stub TagsData should produce an empty Update Tags packet"
    );
    assert_eq!(frame.body.remaining(), 0);

    // Then Finish Configuration.
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(
        frame.id,
        FinishConfiguration::ID,
        "expected Finish Configuration after registries"
    );
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();

    // Acknowledge — that transitions us into Play. Verify the
    // transition by reading the next clientbound packet and confirming
    // it is Login (Play). Full coverage of the Play spawn burst is in
    // tests/play.rs.
    write_frame(&mut stream, &AcknowledgeFinishConfiguration).await;
    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf),
    )
    .await
    .expect("server did not advance to Play within 2s");
    assert_eq!(
        frame.id,
        LoginPlay::ID,
        "after AcknowledgeFinishConfiguration the server should emit \
         Login (Play) as the first Play-state packet"
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
    let mut rbuf = BytesMut::with_capacity(4096);
    run_through_login_ack(&mut stream, &mut rbuf, addr, "Steve").await;

    // Consume the server's Known Packs.
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
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

    // Drain Registry Data packets, one per stub registry, then drain
    // the M3.i Update Tags packet, then expect Finish Configuration.
    for _ in 0..mc_data::KNOWN_REGISTRIES.len() {
        let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
        assert_eq!(frame.id, RegistryData::ID);
        let _ = RegistryData::decode(&mut frame.body).unwrap();
    }
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, UpdateTags::ID);
    let _ = UpdateTags::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(&mut stream, &mut rbuf).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();

    // Send another junk frame before the ack — handler should ignore it
    // too while waiting for AcknowledgeFinishConfiguration.
    let junk = encode_frame(0x02, &[1, 2, 3], Compression::Disabled).unwrap();
    stream.write_all(&junk).await.unwrap();

    write_frame(&mut stream, &AcknowledgeFinishConfiguration).await;
    // Same as above: verify the state transition by waiting for the
    // first Play-state packet.
    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf),
    )
    .await
    .expect("server did not advance to Play within 2s");
    assert_eq!(frame.id, LoginPlay::ID);
}
