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

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use mc_nbt::Tag;
use mc_protocol::PROTOCOL_VERSION;
use mc_protocol::codec::{Identifier, WriteMc};
use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundKnownPacks, FinishConfiguration, RegistryData,
    ServerboundKnownPacks, UpdateEnabledFeatures, UpdateTags,
};
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_protocol::packets::login::{LoginAcknowledged, LoginStart, LoginSuccess, SetCompression};
use mc_protocol::packets::play::{
    ClientboundChangeDifficulty, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundInitializeBorder, ClientboundPlayerAbilities, ClientboundRecipeBookAdd,
    ClientboundRecipeBookSettings, ClientboundSetHealth, ClientboundSetHeldSlot,
    ClientboundSetTime, ClientboundSystemChat, ClientboundUpdateRecipes, CommandNodeKind,
    ConfirmTeleportation, EntityEvent, GameEvent, LoginPlay, PlayDisconnect, ServerboundChat,
    ServerboundChatCommand, ServerboundCustomPayload, ServerboundKeepAlive, SetCenterChunk,
    SetDefaultSpawnPosition, SynchronizePlayerPosition, unpack_block_pos,
};
use mc_protocol::packets::{CustomPayload, Packet};
use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES, ScriptCommand, ScriptEvent,
    ScriptEventKind, ScriptHostEndpoint, ScriptPlayerId, ScriptPluginManifest, ScriptProtocolPhase,
    ValidatedScriptPluginManifest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use uuid::Uuid;

const OVERSIZED_CUSTOM_PAYLOAD_BYTES: usize = MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES + 1;
const SCRIPT_CHANNEL: &str = "solaris:test";
#[path = "play/operator_file_tests.rs"]
mod operator_file_tests;

async fn start_server() -> SocketAddr {
    start_server_with_max(8).await
}

async fn start_server_with_max(max_players: u32) -> SocketAddr {
    let random_tick = mc_net::RandomTickPolicy {
        simulation_distance: 5,
        ..mc_net::RandomTickPolicy::default()
    };
    let cfg = mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::bounded(2),
        random_tick,
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

fn script_channel_manifest(plugin_id: &str) -> ValidatedScriptPluginManifest {
    ScriptPluginManifest::new(
        plugin_id,
        "Test Payload",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_custom_payload_channel(SCRIPT_CHANNEL)
    .validate()
    .expect("test payload channel manifest validates")
}

async fn start_server_with_script_payloads() -> (SocketAddr, ScriptHostEndpoint) {
    let cfg = mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 script payloads".into(),
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::bounded(2),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let (boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(8).unwrap(),
    );
    endpoint
        .register_plugin_routes(&script_channel_manifest("test-payload"))
        .expect("test payload channel registers");
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    (addr, endpoint)
}

async fn start_server_with_scripts() -> (SocketAddr, ScriptHostEndpoint) {
    let cfg = script_server_config(mc_net::ShutdownHandle::default());
    let (boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(32).unwrap(),
        NonZeroUsize::new(8).unwrap(),
    );
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    (addr, endpoint)
}

fn script_server_config(shutdown: mc_net::ShutdownHandle) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        tab_list: mc_net::TabListConfig::default(),
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Component plugin integration".into(),
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::bounded(2),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown,
    }
}

struct FirstConnectedSession;

impl mc_plugin_host::PlayerSessions for FirstConnectedSession {
    fn session_of(&self, _player: &str) -> Option<u64> {
        Some(1)
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn hello_component_bytes() -> &'static [u8] {
    static BYTES: std::sync::LazyLock<Vec<u8>> =
        std::sync::LazyLock::new(build_hello_component_bytes);
    BYTES.as_slice()
}

fn build_hello_component_bytes() -> Vec<u8> {
    let sdk = repository_root().join("sdk/rust");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            sdk.join("Cargo.toml").to_str().expect("utf-8 path"),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "-p",
            "solaris-hello-plugin",
        ])
        .status()
        .expect("the hello guest build starts");
    assert!(status.success(), "the hello guest fixture must build");
    let module =
        std::fs::read(sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"))
            .expect("the hello guest module exists");
    wit_component::ComponentEncoder::default()
        .module(&module)
        .expect("the hello guest module carries component types")
        .validate(true)
        .encode()
        .expect("the hello guest module encodes as a component")
}

fn start_hello_component_deployment(
    root: &Path,
    id: &str,
    greeting: &str,
) -> mc_plugin_host::PluginHost {
    let package = root.join(id);
    std::fs::create_dir_all(&package).expect("component package directory");
    std::fs::write(
        package.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\nplayer_commands = [\"hello\"]\n"
        ),
    )
    .expect("component manifest");
    std::fs::write(
        package.join("config.toml"),
        format!("greeting = \"{greeting}\"\n"),
    )
    .expect("component config");
    std::fs::write(package.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");

    let limits = mc_plugin_host::PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &mc_plugin_host::DeploymentConfig {
            root: root.to_path_buf(),
            mode: mc_plugin_host::DiscoveryMode::Strict,
            expected: vec![id.to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .expect("strict component discovery")
    .into_packages();
    mc_plugin_host::start_deployment(
        packages,
        limits,
        mc_plugin_host::HostQueues::default(),
        Arc::new(FirstConnectedSession),
    )
    .expect("component deployment starts")
}

fn access_file_server_config(path: &Path) -> mc_net::ServerConfig {
    let raw = std::fs::read_to_string(path).expect("read access-control server config");
    let mut config: mc_server::ServerConfig = toml::from_str(&raw).expect("parse server config");
    config
        .load_access_control_files(path)
        .expect("load file-backed access control");
    config
        .to_network(
            std::sync::Arc::new(mc_data::testing::stub()),
            std::sync::Arc::new(
                mc_world::BlockRegistry::from_report(&[]).expect("empty registry builds"),
            ),
            None,
            std::sync::Arc::new(mc_data::tags::TagsData::default()),
            std::sync::Arc::new(Vec::new()),
            std::sync::Arc::new(mc_data::loot::LootTables::default()),
            None,
            std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
            std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
            std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        )
        .expect("translate access-control server config")
}

async fn start_server_from_access_file_config(path: &Path) -> SocketAddr {
    let bound = mc_net::bind(access_file_server_config(path))
        .await
        .expect("bind access-control server");
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

async fn write_oversized_custom_payload_frame(
    stream: &mut TcpStream,
    packet_id: i32,
    compression: Compression,
) {
    let mut body = BytesMut::new();
    body.write_identifier(&Identifier::parse("other:channel").unwrap())
        .unwrap();
    body.resize(body.len() + OVERSIZED_CUSTOM_PAYLOAD_BYTES, 0);
    let framed = encode_frame(packet_id, &body, compression).unwrap();
    stream.write_all(&framed).await.unwrap();
}

async fn recv_script_event(
    endpoint: &mut ScriptHostEndpoint,
    matches: impl Fn(&ScriptEvent) -> bool,
) -> ScriptEvent {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = endpoint
                .recv_event()
                .await
                .expect("script event queue closed");
            if matches(&event) {
                return event;
            }
        }
    })
    .await
    .expect("script event was not delivered within 2s")
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

async fn read_play_disconnect(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) -> PlayDisconnect {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(stream, buf, compression).await;
            if frame.id == PlayDisconnect::ID {
                return PlayDisconnect::decode(&mut frame.body).unwrap();
            }
        }
    })
    .await
    .expect("play disconnect was not delivered within 2s")
}

async fn assert_damage_command_still_processed(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) {
    write_frame(
        stream,
        &ServerboundChatCommand {
            command: "debug survival damage 7.5".to_string(),
        },
        compression,
    )
    .await;

    let health = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(stream, buf, compression).await;
            if frame.id != ClientboundSetHealth::ID {
                continue;
            }
            let health = ClientboundSetHealth::decode(&mut frame.body).unwrap();
            if health.health == 12.5 {
                return health;
            }
        }
    })
    .await
    .expect("damage Set Health was not delivered within 2s");
    assert_eq!(health.health, 12.5);
    assert_eq!(health.food, 20);
    assert_eq!(health.saturation, 5.0);
}

async fn drain_initial_play_burst(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
) {
    let health = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(stream, buf, compression).await;
            if frame.id == ClientboundSetHealth::ID {
                return ClientboundSetHealth::decode(&mut frame.body).unwrap();
            }
        }
    })
    .await
    .expect("initial Set Health was not delivered within 2s");
    assert_eq!(health.health, 20.0);
    assert_eq!(health.food, 20);
    assert_eq!(health.saturation, 5.0);
}

fn disconnect_text(disconnect: &PlayDisconnect) -> String {
    let mut cursor: &[u8] = &disconnect.reason_nbt;
    let tag = mc_nbt::read_network(&mut cursor).expect("disconnect reason NBT decodes");
    let Tag::Compound(fields) = tag else {
        panic!("disconnect reason should be an NBT compound");
    };
    let Some((_, Tag::String(text))) = fields.into_iter().find(|(name, _)| name == "text") else {
        panic!("disconnect reason should include a string text field");
    };
    text
}

fn system_chat_text(chat: &ClientboundSystemChat) -> String {
    let mut cursor: &[u8] = &chat.content_nbt;
    let tag = mc_nbt::read_network(&mut cursor).expect("system chat NBT decodes");
    let Tag::Compound(fields) = tag else {
        panic!("system chat should be an NBT compound");
    };
    let Some((_, Tag::String(text))) = fields.into_iter().find(|(name, _)| name == "text") else {
        panic!("system chat should include a string text field");
    };
    text
}

async fn read_matching_system_chat(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
    expected: &str,
) -> ClientboundSystemChat {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(stream, buf, compression).await;
            if frame.id != ClientboundSystemChat::ID {
                continue;
            }
            let chat = ClientboundSystemChat::decode(&mut frame.body).unwrap();
            if system_chat_text(&chat) == expected {
                return chat;
            }
        }
    })
    .await
    .expect("matching system chat was not delivered within 2s")
}

async fn read_initial_position_and_matching_system_chat(
    stream: &mut TcpStream,
    buf: &mut BytesMut,
    compression: Compression,
    expected: &str,
) -> ClientboundSystemChat {
    let (teleport_id, chat) = tokio::time::timeout(Duration::from_secs(2), async {
        let mut teleport_id = None;
        let mut chat = None;
        loop {
            let mut frame = read_one_frame(stream, buf, compression).await;
            if frame.id == SynchronizePlayerPosition::ID {
                teleport_id = Some(
                    SynchronizePlayerPosition::decode(&mut frame.body)
                        .unwrap()
                        .teleport_id,
                );
            } else if frame.id == ClientboundSystemChat::ID {
                let candidate = ClientboundSystemChat::decode(&mut frame.body).unwrap();
                if system_chat_text(&candidate) == expected {
                    chat = Some(candidate);
                }
            }
            if let Some(teleport_id) = teleport_id
                && let Some(chat) = chat.take()
            {
                return (teleport_id, chat);
            }
        }
    })
    .await
    .expect("initial player position and matching system chat were not delivered within 2s");
    write_frame(stream, &ConfirmTeleportation { teleport_id }, compression).await;
    chat
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

    // Configuration: enabled features, KnownPacks round trip, registries, ack.
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, UpdateEnabledFeatures::ID);
    let _ = UpdateEnabledFeatures::decode(&mut frame.body).unwrap();
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
    assert_eq!(
        frame.id,
        mc_protocol::packets::configuration::ClientboundCustomPayload::ID
    );
    mc_protocol::packets::configuration::ClientboundCustomPayload::decode(&mut frame.body).unwrap();
    let mut frame = read_one_frame(stream, buf, compression).await;
    assert_eq!(frame.id, FinishConfiguration::ID);
    let _ = FinishConfiguration::decode(&mut frame.body).unwrap();
    write_frame(stream, &AcknowledgeFinishConfiguration, compression).await;
    compression
}

async fn command_roots_for(addr: SocketAddr, name: &str) -> Vec<String> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, name).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
            if frame.id != ClientboundCommands::ID {
                continue;
            }
            let commands = ClientboundCommands::decode(&mut frame.body).unwrap();
            let root = &commands.nodes[commands.root_index as usize];
            return root
                .children
                .iter()
                .filter_map(|index| match &commands.nodes[*index as usize].kind {
                    CommandNodeKind::Literal(root) => Some(root.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
        }
    })
    .await
    .expect("command tree was not delivered within 2s")
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
    assert_eq!(login.view_distance, 10);
    assert_eq!(login.simulation_distance, 5);
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
    let overworld = time
        .overworld_clock
        .expect("initial Set Time must carry the overworld clock");
    assert!((0..=20).contains(&overworld.total_ticks));
    assert_eq!(overworld.partial_tick, 0.0);
    assert_eq!(overworld.rate, 1.0);

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        SetDefaultSpawnPosition::ID,
        "expected Set Default Spawn Position"
    );
    let default_spawn = SetDefaultSpawnPosition::decode(&mut frame.body).unwrap();
    assert_eq!(default_spawn.dimension.as_str(), "minecraft:alpha");
    assert_eq!(default_spawn.yaw, 0.0);
    assert_eq!(default_spawn.pitch, 0.0);
    assert_eq!(
        unpack_block_pos(default_spawn.position),
        (
            sync.x.floor() as i32,
            sync.y.floor() as i32,
            sync.z.floor() as i32
        )
    );

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
        ClientboundUpdateRecipes::ID,
        "expected Update Recipes"
    );
    let recipes = ClientboundUpdateRecipes::decode(&mut frame.body).unwrap();
    assert!(recipes.item_sets.is_empty());
    assert!(recipes.stonecutter_recipes.is_empty());

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundRecipeBookSettings::ID,
        "expected Recipe Book Settings"
    );
    let settings = ClientboundRecipeBookSettings::decode(&mut frame.body).unwrap();
    assert_eq!(settings, ClientboundRecipeBookSettings::default());

    let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
    assert_eq!(
        frame.id,
        ClientboundRecipeBookAdd::ID,
        "expected initial Recipe Book Add"
    );
    let recipe_book = ClientboundRecipeBookAdd::decode(&mut frame.body).unwrap();
    assert!(recipe_book.replace);
    assert!(recipe_book.entries.is_empty());

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

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

    // Be polite: ack the teleport.
    write_frame(
        &mut stream,
        &ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        },
        compression,
    )
    .await;

    assert_damage_command_still_processed(&mut stream, &mut rbuf, compression).await;

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
    let mut second_frame = read_one_frame(&mut second, &mut second_buf, second_compression).await;
    assert_eq!(second_frame.id, PlayDisconnect::ID);
    let disconnect = PlayDisconnect::decode(&mut second_frame.body).unwrap();
    assert_eq!(disconnect_text(&disconnect), "Server is full");

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
    let mut second_frame = read_one_frame(&mut second, &mut second_buf, second_compression).await;
    assert_eq!(second_frame.id, PlayDisconnect::ID);
    let disconnect = PlayDisconnect::decode(&mut second_frame.body).unwrap();
    assert_eq!(
        disconnect_text(&disconnect),
        "This player is already connected"
    );

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

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

    write_frame(
        &mut stream,
        &ServerboundKeepAlive { id: 0xDEAD_BEEF },
        compression,
    )
    .await;

    assert_damage_command_still_processed(&mut stream, &mut rbuf, compression).await;
    drop(stream);
}

#[tokio::test]
async fn play_state_ignores_unknown_custom_payload() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "PlayPayload").await;

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

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

    assert_damage_command_still_processed(&mut stream, &mut rbuf, compression).await;
    drop(stream);
}

#[tokio::test]
async fn play_state_ignores_oversized_custom_payload() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "PlayPayloadBig").await;

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

    write_oversized_custom_payload_frame(&mut stream, ServerboundCustomPayload::ID, compression)
        .await;

    assert_damage_command_still_processed(&mut stream, &mut rbuf, compression).await;
    drop(stream);
}

#[tokio::test]
async fn play_script_boundary_receives_join_payload_brand_and_leave() {
    let (addr, mut endpoint) = start_server_with_script_payloads().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "ExtPlayer").await;

    let joined = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerJoined { .. })
    })
    .await;
    let ScriptEventKind::PlayerJoined {
        player_id,
        username,
        ..
    } = joined.kind()
    else {
        panic!("expected PlayerJoined event, got {joined:?}");
    };
    assert_eq!(*player_id, ScriptPlayerId::new(1));
    assert_eq!(username, "ExtPlayer");

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: Identifier::parse(SCRIPT_CHANNEL).unwrap(),
                payload: b"ok".to_vec(),
            },
        },
        compression,
    )
    .await;
    let payload = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::CustomPayload { .. })
    })
    .await;
    let ScriptEventKind::CustomPayload {
        player_id: payload_player,
        phase,
        channel,
        payload,
    } = payload.kind()
    else {
        panic!("expected CustomPayload event, got {payload:?}");
    };
    assert_eq!(*payload_player, *player_id);
    assert_eq!(*phase, ScriptProtocolPhase::Play);
    assert_eq!(channel, SCRIPT_CHANNEL);
    assert_eq!(payload, b"ok");

    write_frame(
        &mut stream,
        &ServerboundCustomPayload {
            payload: CustomPayload::Brand("solar-client".to_owned()),
        },
        compression,
    )
    .await;
    let brand = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::ClientBrand { .. })
    })
    .await;
    assert_eq!(
        brand.kind(),
        &ScriptEventKind::ClientBrand {
            player_id: *player_id,
            brand: "solar-client".to_owned(),
        }
    );

    drop(stream);
    let left = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerLeft { .. })
    })
    .await;
    assert_eq!(left, ScriptEvent::player_left(*player_id, "disconnected"));
}

#[test]
fn script_payload_channel_ownership_rejects_collisions_until_released() {
    let (_boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(8).unwrap(),
    );
    endpoint
        .register_plugin_routes(&script_channel_manifest("first-owner"))
        .expect("first channel owner registers");
    assert!(
        endpoint
            .register_plugin_routes(&script_channel_manifest("second-owner"))
            .is_err(),
        "a second plugin must not claim an owned payload channel"
    );
    endpoint.unregister_plugin_routes("first-owner");
    endpoint
        .register_plugin_routes(&script_channel_manifest("second-owner"))
        .expect("released channel re-registers to a new owner");
    endpoint.unregister_plugin_routes("second-owner");
}
#[tokio::test]
async fn play_script_boundary_carries_lifecycle_chat_tick_and_targeted_reply() {
    let (addr, mut endpoint) = start_server_with_scripts().await;
    let started = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::ServerStarted)
    })
    .await;
    assert_eq!(started, ScriptEvent::server_started());

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "ScriptPlayer").await;
    let joined = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerJoined { .. })
    })
    .await;
    assert!(matches!(
        joined.kind(),
        ScriptEventKind::PlayerJoined {
            player_id, username, ..
        } if *player_id == ScriptPlayerId::new(1) && username == "ScriptPlayer"
    ));

    write_frame(
        &mut stream,
        &ServerboundChat {
            message: "hello plugin".to_owned(),
            timestamp_millis: 0,
            salt: 0,
            signature: None,
            last_seen_offset: 0,
            last_seen_acknowledged: [0; 3],
            last_seen_checksum: 0,
        },
        compression,
    )
    .await;
    let chat = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerChat { .. })
    })
    .await;
    assert!(matches!(
        chat.kind(),
        ScriptEventKind::PlayerChat {
            player_id, message, ..
        } if *player_id == ScriptPlayerId::new(1) && message == "hello plugin"
    ));

    let tick = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::ServerTick { .. })
    })
    .await;
    assert!(matches!(
        tick.kind(),
        ScriptEventKind::ServerTick { tick } if *tick > 0
    ));

    endpoint
        .try_submit_command(ScriptCommand::SendChatMessage {
            player_id: ScriptPlayerId::new(1),
            message: "plugin reply".to_owned(),
        })
        .unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut frame = read_one_frame(&mut stream, &mut rbuf, compression).await;
            if frame.id == ClientboundSystemChat::ID {
                let chat = ClientboundSystemChat::decode(&mut frame.body).unwrap();
                if system_chat_text(&chat) == "plugin reply" {
                    return chat;
                }
            }
        }
    })
    .await
    .expect("script chat reply was not delivered within 2s");
    assert!(!reply.overlay);

    drop(stream);
    let left = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerLeft { .. })
    })
    .await;
    assert_eq!(
        left,
        ScriptEvent::player_left(ScriptPlayerId::new(1), "disconnected")
    );
}

#[tokio::test]
async fn plugin_owned_command_argument_limits_do_not_terminate_play_ingress() {
    const ROOT_PREFIX: &str = "owned ";
    const MAX_PROTOCOL_COMMAND_BYTES: usize = 32_767;

    let (addr, mut endpoint) = start_server_with_scripts().await;
    let manifest = mc_script::ScriptPluginManifest::new(
        "command-boundary",
        "Command Boundary",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_player_command_root("owned")
    .validate()
    .unwrap();
    endpoint.register_plugin_routes(&manifest).unwrap();
    recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::ServerStarted)
    })
    .await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "CommandPlayer").await;
    // Drain the login burst before waiting on server-side script events: the
    // session task blocks writing this socket, so an unread burst would starve
    // the command ingress this test is actually about.
    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;
    recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerJoined { .. })
    })
    .await;

    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: format!("{ROOT_PREFIX}{}", "a".repeat(4_096)),
        },
        compression,
    )
    .await;
    let accepted = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerCommand { .. })
    })
    .await;
    assert!(matches!(
        accepted.kind(),
        ScriptEventKind::PlayerCommand { root, arguments, .. }
            if root == "owned" && arguments.len() == 4_096
    ));

    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: format!("{ROOT_PREFIX}{}", "b".repeat(4_097)),
        },
        compression,
    )
    .await;
    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: "owned after-4097".to_owned(),
        },
        compression,
    )
    .await;
    let after_4097 = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerCommand { .. })
    })
    .await;
    assert!(matches!(
        after_4097.kind(),
        ScriptEventKind::PlayerCommand { arguments, .. } if arguments == "after-4097"
    ));

    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: format!(
                "{ROOT_PREFIX}{}",
                "c".repeat(MAX_PROTOCOL_COMMAND_BYTES - ROOT_PREFIX.len())
            ),
        },
        compression,
    )
    .await;
    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: "owned after-32767".to_owned(),
        },
        compression,
    )
    .await;
    let after_protocol_max = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerCommand { .. })
    })
    .await;
    assert!(matches!(
        after_protocol_max.kind(),
        ScriptEventKind::PlayerCommand { arguments, .. } if arguments == "after-32767"
    ));
}

#[tokio::test]
async fn component_plugin_loaded_from_disk_replies_to_join_and_command_over_the_wire() {
    let plugins = tempfile::tempdir().unwrap();
    let host = start_hello_component_deployment(plugins.path(), "welcome", "joined");
    let boundary = host.boundary().clone();

    let shutdown = mc_net::ShutdownHandle::default();
    let mut config = script_server_config(shutdown.clone());
    config.command_permissions = mc_net::CommandPermissionConfig::new(["ComponentPlayer"], false);
    let bound = mc_net::bind_with_scripts(config, boundary).await.unwrap();
    let addr = bound.local_addr().unwrap();
    let server = tokio::spawn(async move { bound.serve().await });

    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "ComponentPlayer").await;
    let joined = read_initial_position_and_matching_system_chat(
        &mut stream,
        &mut rbuf,
        compression,
        "joined ComponentPlayer",
    )
    .await;
    assert!(!joined.overlay);

    write_frame(
        &mut stream,
        &ServerboundChatCommand {
            command: "hello".to_owned(),
        },
        compression,
    )
    .await;
    let reply = read_matching_system_chat(
        &mut stream,
        &mut rbuf,
        compression,
        "Hello from a WASM plugin.",
    )
    .await;
    assert!(!reply.overlay);

    drop(stream);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("server did not stop within 2s")
        .expect("server task failed")
        .expect("server returned an error");
    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one component instance ran");
}

#[tokio::test]
async fn file_backed_operator_permissions_reload_across_server_instances() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    let operators_path = temp.path().join("ops.json");
    std::fs::write(
        &config_path,
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [admin]
            operators_file = "ops.json"
            allow_local_dev_operators = false
        "#,
    )
    .unwrap();
    std::fs::write(&operators_path, br#"[{"name":"FileOp","level":4}]"#).unwrap();

    let first_addr = start_server_from_access_file_config(&config_path).await;
    let operator_roots = command_roots_for(first_addr, "FileOp").await;
    assert!(operator_roots.iter().any(|root| root == "time"));
    let member_roots = command_roots_for(first_addr, "MemberOne").await;
    assert!(!member_roots.iter().any(|root| root == "time"));

    std::fs::write(&operators_path, br#"[{"name":"SecondOp","level":4}]"#).unwrap();
    let restarted_addr = start_server_from_access_file_config(&config_path).await;
    let former_operator_roots = command_roots_for(restarted_addr, "FileOp").await;
    assert!(!former_operator_roots.iter().any(|root| root == "time"));
    let new_operator_roots = command_roots_for(restarted_addr, "SecondOp").await;
    assert!(new_operator_roots.iter().any(|root| root == "time"));
}

#[tokio::test]
async fn play_script_disconnect_command_disconnects_player() {
    let (addr, mut endpoint) = start_server_with_script_payloads().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "ExtKick").await;

    let joined = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerJoined { .. })
    })
    .await;
    let ScriptEventKind::PlayerJoined { player_id, .. } = joined.kind() else {
        panic!("expected PlayerJoined event, got {joined:?}");
    };
    let player_id = *player_id;

    endpoint
        .try_submit_command(ScriptCommand::DisconnectPlayer {
            player_id,
            reason: "script requested disconnect".to_owned(),
        })
        .unwrap();

    let disconnect = read_play_disconnect(&mut stream, &mut rbuf, compression).await;
    assert_eq!(disconnect_text(&disconnect), "script requested disconnect");

    let left = recv_script_event(&mut endpoint, |event| {
        matches!(event.kind(), ScriptEventKind::PlayerLeft { .. })
    })
    .await;
    assert_eq!(left, ScriptEvent::player_left(player_id, "disconnected"));
}

#[tokio::test]
async fn play_state_survival_damage_command_updates_health() {
    let addr = start_server().await;
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut rbuf = BytesMut::with_capacity(8192);
    let compression = drive_to_play(&mut stream, &mut rbuf, addr, "DamageCmd").await;

    drain_initial_play_burst(&mut stream, &mut rbuf, compression).await;

    assert_damage_command_still_processed(&mut stream, &mut rbuf, compression).await;

    drop(stream);
}
