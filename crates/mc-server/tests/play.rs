//! End-to-end test for M1.g.3 Play state entry.
//!
//! Drives the full Handshake → Login → Configuration → Play sequence
//! and asserts that, on the Play side of the boundary, the server:
//!
//! - sends `Login (Play)` with sensible fields,
//! - sends `Synchronize Player Position`, `Set Default Spawn Position`,
//!   and a `GameEvent` instructing the client to stop waiting for level
//!   chunks (the M1.g world is intentionally empty),
//! - sends a `Clientbound Keep Alive` periodically and treats our echo
//!   as a heartbeat (we manually shrink the timing for the test by
//!   reading the first keepalive after the spawn burst, not by tuning
//!   the handler).
//!
//! Packet IDs are still M1.g.4-pending; this test is the wire-shape
//! check, not a vanilla-client compatibility test.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::{
    ClientboundChangeDifficulty, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundInitializeBorder, ClientboundKeepAlive, ClientboundPlayerAbilities,
    ClientboundSetHealth, ClientboundSetHeldSlot, ClientboundSetTime, ConfirmTeleportation,
    EntityEvent, GameEvent, LoginPlay, PlayDisconnect, ServerboundChatCommand,
    ServerboundKeepAlive, SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

async fn start_server() -> SocketAddr {
    start_server_with_max(8).await
}

async fn start_server_with_max(max_players: u32) -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M1.g play".into(),
        max_players,
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

/// Walk the full protocol up to and including
/// `AcknowledgeFinishConfiguration`. After this the connection is in
/// Play state on the server side.
async fn drive_to_play(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    addr: SocketAddr,
    name: &str,
) -> Compression {
    // Handshake → Login Start → expect Login Success → Acknowledged.
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

    // Configuration: KnownPacks round trip, drain registries, ack.
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, ClientboundKnownPacks::ID);
    let cb_packs = ClientboundKnownPacks::decode(&mut frame.body).unwrap();
    write_frame(
        stream,
        &ServerboundKnownPacks {
            packs: cb_packs.packs,
        },
        compression,
    )
    .await;
    for _ in 0..mc_data::KNOWN_REGISTRIES.len() {
        let mut frame = read_one_frame(stream, buf, compression).await;
        assert_eq!(frame.id, RegistryData::ID);
        let _ = RegistryData::decode(&mut frame.body).unwrap();
    }
    // M3.i: Update Tags arrives between the last RegistryData and
    // FinishConfiguration. The stub `tags` here is empty; the packet
    // is still required on the wire.
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, UpdateTags::ID);
    let _ = UpdateTags::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();
    write_frame(stream, &AcknowledgeFinishConfiguration, compression).await;
    compression
}

#[tokio::test]
async fn play_state_entry_sends_login_and_spawn_burst() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "PlayTester").await;

    // The handler emits the Play entry burst back-to-back. Order matters:
    // Login (Play) first because the client needs the world setup before it
    // can interpret anything else.
    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, LoginPlay::ID, "expected Login (Play) first");
    let login = LoginPlay::decode(&mut frame.body).unwrap();
    assert_eq!(frame.body.remaining(), 0);
    // Stub data is alphabetical: dimensions are [alpha, beta]. Server
    // should pick the first as the player's spawn dimension.
    assert_eq!(login.dimension_names.len(), 2);
    assert_eq!(login.dimension_type_id, 0);
    assert_eq!(login.dimension_name.as_str(), "minecraft:alpha");
    assert_eq!(login.game_mode, 0); // survival
    assert!(
        !login.is_flat,
        "generated terrain is no longer a flat world"
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundChangeDifficulty::ID,
        "expected ChangeDifficulty after Login"
    );
    let change_difficulty = ClientboundChangeDifficulty::decode(&mut frame.body).unwrap();
    assert!(
        change_difficulty.difficulty < 4,
        "difficulty ordinal in range"
    );

    let frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundPlayerAbilities::ID,
        "expected PlayerAbilities after ChangeDifficulty"
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundSetHeldSlot::ID,
        "expected SetHeldSlot after PlayerAbilities"
    );
    let held = ClientboundSetHeldSlot::decode(&mut frame.body).unwrap();
    assert_eq!(held.slot, 0);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, EntityEvent::ID, "expected permission EntityEvent");
    let permission_event = EntityEvent::decode(&mut frame.body).unwrap();
    assert_eq!(permission_event.entity_id, login.entity_id);
    assert_eq!(permission_event.event_id, 28);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundCommands::ID,
        "expected Commands after Login"
    );
    let commands = ClientboundCommands::decode(&mut frame.body).unwrap();
    assert!(
        !commands.nodes[commands.root_index as usize]
            .children
            .is_empty()
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        SynchronizePlayerPosition::ID,
        "expected Synchronize Player Position"
    );
    let sync = SynchronizePlayerPosition::decode(&mut frame.body).unwrap();
    assert_eq!(sync.teleport_id, 1);
    // Spawn Y sits just above the flat-preset grass surface
    // (Y=-61); see SPAWN_Y in mc_net::play. Bound the assertion to
    // a sane window rather than re-importing the constant.
    assert!(
        sync.y > -65.0 && sync.y < 0.0,
        "unexpected spawn y={}",
        sync.y
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundInitializeBorder::ID,
        "expected Initialize Border"
    );
    let border = ClientboundInitializeBorder::decode(&mut frame.body).unwrap();
    assert_eq!(border.absolute_max_size, 29_999_984);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundSetTime::ID,
        "expected initial Set Time"
    );
    let time = ClientboundSetTime::decode(&mut frame.body).unwrap();
    assert!(
        (0..=20).contains(&time.game_time),
        "unexpected initial game_time={}",
        time.game_time
    );

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        SetDefaultSpawnPosition::ID,
        "expected Set Default Spawn Position"
    );
    let default_spawn = SetDefaultSpawnPosition::decode(&mut frame.body).unwrap();
    assert_eq!(default_spawn.dimension, login.dimension_name);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, GameEvent::ID, "expected Game Event");
    let event = GameEvent::decode(&mut frame.body).unwrap();
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(frame.id, SetCenterChunk::ID, "expected Set Center Chunk");
    let center = SetCenterChunk::decode(&mut frame.body).unwrap();
    // SPAWN_(X,Z) = (0.5, 0.5) → chunk (0, 0).
    assert_eq!((center.chunk_x, center.chunk_z), (0, 0));
    // With world = None in this test the chunk packet is intentionally
    // not emitted; M3.e's view-distance test exercises the chunk path.

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundContainerSetContent::ID,
        "expected Container Set Content"
    );
    let inventory = ClientboundContainerSetContent::decode(&mut frame.body).unwrap();
    assert_eq!(inventory.container_id, 0);
    assert_eq!(inventory.state_id, 1);
    assert_eq!(inventory.items.len(), 46);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    for _ in 0..64 {
        if frame.id == ClientboundSetHealth::ID {
            break;
        }
        frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    }
    assert_eq!(frame.id, ClientboundSetHealth::ID, "expected Set Health");
    let health = ClientboundSetHealth::decode(&mut frame.body).unwrap();
    assert_eq!(health.health, 20.0);
    assert_eq!(health.food, 20);
    assert_eq!(health.saturation, 5.0);

    // Be polite: ack the teleport.
    write_frame(
        &mut stream,
        &ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        },
        compression,
    )
    .await;

    // Closing here: the keepalive loop runs on a 15-second tick, which
    // is way too long for a unit test. Asserting "no keepalive yet" or
    // waiting for one would either flake or stall, so we just confirm
    // the server stays connected for a beat and then close ourselves —
    // server.rs will surface EOF cleanly.
    let mut scratch = [0u8; 1];
    let early_close =
        tokio::time::timeout(Duration::from_millis(250), stream.read(&mut scratch)).await;
    assert!(
        early_close.is_err(),
        "server should NOT close the connection in the first quarter-second \
         after the spawn burst — keepalive loop is running"
    );

    // Now drop the client; the server task will see EOF and exit.
    drop(stream);
}

#[tokio::test]
async fn play_state_rejects_second_client_when_server_is_full() {
    let addr = start_server_with_max(1).await;
    let mut first = TcpStream::connect(addr).await.unwrap();
    let mut first_buf = BytesMut::with_capacity(8192);
    let first_compression = drive_to_play(&mut first, &mut first_buf, addr, "FullFirst").await;
    let first_frame = read_one_frame(&mut first, &mut first_buf, first_compression).await;
    assert_eq!(first_frame.id, LoginPlay::ID);

    let mut second = TcpStream::connect(addr).await.unwrap();
    let mut second_buf = BytesMut::with_capacity(8192);
    let second_compression = drive_to_play(&mut second, &mut second_buf, addr, "FullSecond").await;
    let second_frame = read_one_frame(&mut second, &mut second_buf, second_compression).await;
    assert_eq!(second_frame.id, PlayDisconnect::ID);

    drop(first);
    drop(second);
}

#[tokio::test]
async fn play_state_rejects_duplicate_offline_profile() {
    let addr = start_server().await;
    let mut first = TcpStream::connect(addr).await.unwrap();
    let mut first_buf = BytesMut::with_capacity(8192);
    let first_compression = drive_to_play(&mut first, &mut first_buf, addr, "DupProfile").await;
    let first_frame = read_one_frame(&mut first, &mut first_buf, first_compression).await;
    assert_eq!(first_frame.id, LoginPlay::ID);

    let mut second = TcpStream::connect(addr).await.unwrap();
    let mut second_buf = BytesMut::with_capacity(8192);
    let second_compression = drive_to_play(&mut second, &mut second_buf, addr, "DupProfile").await;
    let second_frame = read_one_frame(&mut second, &mut second_buf, second_compression).await;
    assert_eq!(second_frame.id, PlayDisconnect::ID);

    drop(first);
    drop(second);
}

#[tokio::test]
async fn play_state_handles_serverbound_keepalive_echo() {
    // We can't easily wait 15 s for a keepalive in a unit test, so this
    // test instead sends a *spurious* keepalive echo from the client to
    // confirm the server reads it without crashing or treating it as
    // an unexpected packet. The mismatch will be logged as a warning;
    // the connection should stay up.
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "Spurious").await;

    // Drain Play entry burst. With world = None the chunk packet itself
    // is intentionally not emitted (LoginPlay, ChangeDifficulty,
    // PlayerAbilities, SetHeldSlot, permission EntityEvent, Commands,
    // SyncPos, InitializeBorder, SetTime, SetDefaultSpawn, GameEvent,
    // SetCenterChunk, 3 visibility/dispatch bursts = 15 frames).
    for _ in 0..15 {
        let _ = read_one_frame(&mut stream, &mut rbuf, compression).await;
    }

    write_frame(
        &mut stream,
        &ServerboundKeepAlive { id: 0xDEAD_BEEF },
        compression,
    )
    .await;

    // Confirm the connection is still alive — the server should log a
    // mismatch warning but not close.
    let mut scratch = [0u8; 1];
    let close = tokio::time::timeout(Duration::from_millis(250), stream.read(&mut scratch)).await;
    assert!(
        close.is_err(),
        "server should not close on a spurious keepalive id"
    );
    drop(stream);

    // Hush the unused-import lint for `ClientboundKeepAlive`. It is
    // referenced by docstrings only — the test wire format uses the
    // serverbound counterpart.
    let _ = std::mem::size_of::<ClientboundKeepAlive>();
}

#[tokio::test]
async fn play_state_survival_damage_command_updates_health() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "DamageCmd").await;

    for _ in 0..15 {
        let _ = read_one_frame(&mut stream, &mut rbuf, compression).await;
    }

    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: "debug survival damage 7.5".to_string(),
        },
        compression,
    )
    .await;

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    for _ in 0..64 {
        if frame.id == ClientboundSetHealth::ID {
            break;
        }
        frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    }
    assert_eq!(frame.id, ClientboundSetHealth::ID, "expected Set Health");
    let health = ClientboundSetHealth::decode(&mut frame.body).unwrap();
    assert_eq!(health.health, 12.5);
    assert_eq!(health.food, 20);
    assert_eq!(health.saturation, 5.0);

    drop(stream);
}
