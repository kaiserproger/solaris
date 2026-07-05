//! End-to-end tests for the Configuration state.
//!
//! Drives a raw TCP client all the way from Handshake through the
//! Configuration handshake (Known Packs round trip + Finish/Ack) and
//! asserts the server emits registry/tag data only after the client
//! acknowledges the advertised built-in pack.

use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_extension::{DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES, InboundEvent, ProtocolPhase, QueueRecvError};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::TARGET_RELEASE;
use mc_protocol::codec::Identifier;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, KnownPackEntry,
    RegistryData, ServerboundClientInformation, ServerboundCustomPayload, ServerboundKnownPacks,
    UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::LoginPlay;
use mc_protocol::packets::{ChatVisibility, ClientInformation, MainHand, ParticleStatus};
use mc_protocol::packets::{CustomPayload, Packet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

const OVERSIZED_CUSTOM_PAYLOAD_BYTES: usize = DEFAULT_MAX_CUSTOM_PAYLOAD_BYTES + 1;
const EXTENSION_CHANNEL: &str = "solaris:test";

async fn start_server() -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.e config".into(),
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
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn start_server_with_extension() -> (SocketAddr, mc_extension::ExtensionEndpoint) {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 configuration extension".into(),
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
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let (boundary, endpoint) =
        mc_extension::boundary_pair(NonZeroUsize::new(8).unwrap(), NonZeroUsize::new(1).unwrap());
    let policy = mc_extension::CustomPayloadPolicy::new(16, [EXTENSION_CHANNEL.to_owned()]);
    let bound = mc_net::bind_with_extension(cfg, boundary, policy)
        .await
        .expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    (addr, endpoint)
}

async fn write_frame<P: Packet>(stream: &mut TcpStream, packet: &P, compression: Compression) {
    let mut body = BytesMut::new();
    packet.encode(&mut body).unwrap();
    let framed = encode_frame(P::ID, &body, compression).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn recv_extension_event(endpoint: &mc_extension::ExtensionEndpoint) -> InboundEvent {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match endpoint.try_recv_event() {
                Ok(event) => return event,
                Err(QueueRecvError::Empty) => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(QueueRecvError::Closed) => panic!("extension event queue closed"),
                Err(error) => panic!("unexpected extension queue error: {error:?}"),
            }
        }
    })
    .await
    .expect("extension event was not delivered within 2s")
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

async fn read_optional_frame(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) -> Option<mc_protocol::RawFrame> {
    loop {
        if let Some(frame) = try_decode_frame(buf, compression).unwrap() {
            return Some(frame);
        }
        let read = stream.read_buf(buf).await.unwrap();
        if read == 0 {
            return None;
        }
    }
}

fn client_information_packet() -> ServerboundClientInformation {
    ServerboundClientInformation {
        information: ClientInformation {
            language: "en_us".to_string(),
            view_distance: 8,
            chat_visibility: ChatVisibility::Full,
            chat_colors: true,
            model_customisation: 0x7f,
            main_hand: MainHand::Right,
            text_filtering_enabled: false,
            allows_listing: true,
            particle_status: ParticleStatus::All,
        },
    }
}

async fn read_to_finish_configuration(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) {
    for _ in 0..mc_data::KNOWN_REGISTRIES.len() {
        let mut frame = read_one_frame(stream, buf, compression).await;
        assert_eq!(frame.id, RegistryData::ID);
        let _ = RegistryData::decode(&mut frame.body).unwrap();
    }
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, UpdateTags::ID);
    let _ = UpdateTags::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();
}

/// Walk the protocol up to (and including) `LoginAcknowledged`, leaving
/// the client sitting in Configuration state.
async fn run_through_login_ack(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    addr: SocketAddr,
    name: &str,
) -> Compression {
    write_frame(
        stream,
        &Handshake {
            protocol_version: PROTOCOL_VERSION,
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: NextState::Login,
        },
        Compression::Disabled,
    )
    .await;
    write_frame(
        stream,
        &LoginStart {
            name: name.into(),
            player_uuid: Uuid::nil(),
        },
        Compression::Disabled,
    )
    .await;
    let mut frame = read_one_frame(stream, buf, Compression::Disabled).await;
    assert_eq!(frame.id, SetCompression::ID);
    let set_compression = SetCompression::decode(&mut frame.body).unwrap();
    let compression = Compression::Threshold(set_compression.threshold as usize);
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, LoginSuccess::ID);
    let _ = LoginSuccess::decode(&mut frame.body).unwrap();
    write_frame(stream, &LoginAcknowledged, compression).await;
    compression
}

#[tokio::test]
async fn configuration_known_packs_and_finish_complete() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "Notch").await;

    // First Configuration packet from the server: Known Packs.
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
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
        compression,
    )
    .await;

    // Server now sends Registry Data — one packet per registry. With
    // the test stub each registry has 2 entries with has_data=false.
    let stub_registry_count = mc_data::KNOWN_REGISTRIES.len();
    let mut seen_dimension_type = false;
    for _ in 0..stub_registry_count {
        let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
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
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
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
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
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
    write_frame(&mut stream, &AcknowledgeFinishConfiguration, compression).await;
    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf, compression),
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
async fn configuration_rejects_missing_known_pack_echo() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "Alex").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    assert_eq!(known.packs.len(), 1);

    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: Vec::new() },
        compression,
    )
    .await;

    let next = tokio::time::timeout(
        Duration::from_secs(2),
        read_optional_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not reject missing Known Packs echo within 2s");
    assert!(
        next.is_none(),
        "server must close before sending has_data=false Registry Data without a confirmed built-in pack"
    );
}

#[tokio::test]
async fn configuration_rejects_excess_unsolicited_packets_before_known_packs() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "Spammy").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let _ = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    for _ in 0..33 {
        let information = client_information_packet();
        write_frame(&mut stream, &information, compression).await;
    }

    let next = tokio::time::timeout(
        Duration::from_secs(2),
        read_optional_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not enforce Configuration ignored-packet budget within 2s");
    assert!(
        next.is_none(),
        "server must close before a client can keep Configuration alive with endless optional packets"
    );
}

#[tokio::test]
async fn configuration_ignores_unknown_custom_payload_before_known_packs() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "PayloadDeny").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("other:channel").unwrap(),
                payload: b"small".to_vec(),
            },
        },
        compression,
    )
    .await;
    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: known.packs },
        compression,
    )
    .await;

    read_to_finish_configuration(&mut stream, &mut rbuf, compression).await;
}

#[tokio::test]
async fn configuration_extension_boundary_receives_allowed_payloads() {
    let (addr, endpoint) = start_server_with_extension().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "ConfigExtension").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("other:channel").unwrap(),
                payload: b"drop".to_vec(),
            },
        },
        compression,
    )
    .await;
    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse(EXTENSION_CHANNEL).unwrap(),
                payload: b"before".to_vec(),
            },
        },
        compression,
    )
    .await;

    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: known.packs },
        compression,
    )
    .await;
    read_to_finish_configuration(&mut stream, &mut rbuf, compression).await;

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse(EXTENSION_CHANNEL).unwrap(),
                payload: b"before_ack".to_vec(),
            },
        },
        compression,
    )
    .await;

    write_frame(&mut stream, &AcknowledgeFinishConfiguration, compression).await;
    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not advance to Play after Configuration extension payloads");
    assert_eq!(frame.id, LoginPlay::ID);

    let joined = recv_extension_event(&endpoint).await;
    let InboundEvent::PlayerJoined {
        player_id,
        username,
    } = joined
    else {
        panic!("expected PlayerJoined event, got {joined:?}");
    };
    assert_eq!(username, "ConfigExtension");

    let payload = recv_extension_event(&endpoint).await;
    let InboundEvent::CustomPayload(payload) = payload else {
        panic!("expected CustomPayload event, got {payload:?}");
    };
    assert_eq!(payload.player_id, player_id);
    assert_eq!(payload.phase, ProtocolPhase::Configuration);
    assert_eq!(payload.channel, EXTENSION_CHANNEL);
    assert_eq!(payload.payload.as_ref(), b"before");

    let payload = recv_extension_event(&endpoint).await;
    let InboundEvent::CustomPayload(payload) = payload else {
        panic!("expected CustomPayload event, got {payload:?}");
    };
    assert_eq!(payload.player_id, player_id);
    assert_eq!(payload.phase, ProtocolPhase::Configuration);
    assert_eq!(payload.channel, EXTENSION_CHANNEL);
    assert_eq!(payload.payload.as_ref(), b"before_ack");
}

#[tokio::test]
async fn configuration_ignores_oversized_custom_payload_before_known_packs() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "PayloadBig").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("other:channel").unwrap(),
                payload: vec![0; OVERSIZED_CUSTOM_PAYLOAD_BYTES],
            },
        },
        compression,
    )
    .await;
    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: known.packs },
        compression,
    )
    .await;

    read_to_finish_configuration(&mut stream, &mut rbuf, compression).await;
}

#[tokio::test]
async fn configuration_rejects_excess_unsolicited_packets_before_finish_ack() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "AckSpam").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: known.packs },
        compression,
    )
    .await;

    read_to_finish_configuration(&mut stream, &mut rbuf, compression).await;

    for _ in 0..33 {
        let information = client_information_packet();
        write_frame(&mut stream, &information, compression).await;
    }

    let next = tokio::time::timeout(
        Duration::from_secs(2),
        read_optional_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not enforce Configuration ack ignored-packet budget within 2s");
    assert!(
        next.is_none(),
        "server must close before a client can keep the finish-ack wait alive with optional packets"
    );
}

#[tokio::test]
async fn configuration_ignores_oversized_custom_payload_before_finish_ack() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(4096);
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "AckPayloadBig").await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let known = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    write_frame(
        &mut stream,
        &ServerboundKnownPacks { packs: known.packs },
        compression,
    )
    .await;
    read_to_finish_configuration(&mut stream, &mut rbuf, compression).await;

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse("other:channel").unwrap(),
                payload: vec![0; OVERSIZED_CUSTOM_PAYLOAD_BYTES],
            },
        },
        compression,
    )
    .await;
    write_frame(&mut stream, &AcknowledgeFinishConfiguration, compression).await;

    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not advance to Play after oversized Configuration payload before ack");
    assert_eq!(frame.id, LoginPlay::ID);
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
    let compression = run_through_login_ack(&mut stream, &mut rbuf, addr, "Steve").await;

    // Consume the server's Known Packs.
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let _ = ClientboundKnownPacks::decode(&mut frame.body).unwrap();

    // Send valid Client Information before Known Packs. The handler should
    // decode and ignore it while waiting for the expected response.
    let information = client_information_packet();
    write_frame(&mut stream, &information, compression).await;

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
        compression,
    )
    .await;

    // Drain Registry Data packets, one per stub registry, then drain
    // the M3.i Update Tags packet, then expect Finish Configuration.
    for _ in 0..mc_data::KNOWN_REGISTRIES.len() {
        let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
        assert_eq!(frame.id, RegistryData::ID);
        let _ = RegistryData::decode(&mut frame.body).unwrap();
    }
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, UpdateTags::ID);
    let _ = UpdateTags::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();

    // Send another valid-but-out-of-order Client Information before the ack —
    // handler should ignore it too while waiting for AcknowledgeFinishConfiguration.
    let information = client_information_packet();
    write_frame(&mut stream, &information, compression).await;

    write_frame(&mut stream, &AcknowledgeFinishConfiguration, compression).await;
    // Same as above: verify the state transition by waiting for the
    // first Play-state packet.
    let frame = tokio::time::timeout(
        Duration::from_secs(2),
        read_one_frame(&mut stream, &mut rbuf, compression),
    )
    .await
    .expect("server did not advance to Play within 2s");
    assert_eq!(frame.id, LoginPlay::ID);
}
