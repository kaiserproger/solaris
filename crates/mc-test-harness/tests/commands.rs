use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundSetTime, ClientboundSystemChat, CommandNodeKind,
    ConfirmTeleportation, GameEvent, GameMode, ServerboundChat, ServerboundChatCommand,
    SetCenterChunk, SynchronizePlayerPosition,
};
use mc_test_harness::client::{Client, FrameWaitLimits};

const LATENCY_SENSITIVE_FRAME_WAIT_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(128),
    max_skipped_bytes: Some(128 * 1024),
};

async fn start_server() -> SocketAddr {
    start_server_with_shutdown(mc_net::ShutdownHandle::default()).await
}

async fn start_server_with_shutdown(shutdown: mc_net::ShutdownHandle) -> SocketAddr {
    start_server_with_shutdown_and_chunk_pipeline(shutdown, mc_net::ChunkPipelinePolicy::default())
        .await
}

async fn start_server_with_shutdown_and_chunk_pipeline(
    shutdown: mc_net::ShutdownHandle,
    chunk_pipeline: mc_net::ChunkPipelinePolicy,
) -> SocketAddr {
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M35 commands".into(),
        max_players: 4,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline,
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown,
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    addr
}

async fn start_server_with_runtime_control() -> (SocketAddr, mc_net::RuntimeControlHandle) {
    start_server_with_runtime_control_and_shutdown(mc_net::ShutdownHandle::default()).await
}

async fn start_server_with_runtime_control_and_shutdown(
    shutdown: mc_net::ShutdownHandle,
) -> (SocketAddr, mc_net::RuntimeControlHandle) {
    let chunk_pipeline = mc_net::ChunkPipelinePolicy {
        runtime_control: Some(mc_net::RuntimeControlConfig {
            policy: mc_net::AutoscalePolicy {
                min_view_distance: 2,
                max_view_distance: 8,
                min_chunk_send_rate: 1,
                max_chunk_send_rate: 16,
                min_chunk_load_rate: 2,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 3,
                max_chunk_generate_rate: 32,
                ..mc_net::AutoscalePolicy::for_profile(mc_net::AutoscaleProfile::Balanced)
            },
            initial_limits: mc_net::RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 64,
                chunk_generate_rate: 32,
            },
        }),
        ..mc_net::ChunkPipelinePolicy::default()
    };
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 runtime status".into(),
        max_players: 4,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline,
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown,
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let runtime_control = bound
        .runtime_control_handle()
        .expect("runtime control handle");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    (addr, runtime_control)
}

#[tokio::test]
async fn command_tree_gamemode_and_feedback_round_trip() {
    let addr = start_server().await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M35Command").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let commands: ClientboundCommands = client.read_typed().await.expect("Commands");
    let root = &commands.nodes[commands.root_index as usize];
    assert!(
        !root.children.is_empty(),
        "command tree must advertise commands"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("send gamemode command");

    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                GameEvent::ID,
                Duration::from_secs(5),
                LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("gamemode event frame");
        let frame = outcome.frame;
        let event = GameEvent::decode(&mut frame.body.clone()).expect("decode GameEvent");
        if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
            && event.value == GameMode::Creative.id() as f32
        {
            break;
        }
    }
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSystemChat::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("command feedback frame");
    assert!(
        outcome.skipped.frames <= 16,
        "command feedback skipped too many frames: {:?}",
        outcome.skipped
    );
    let frame = outcome.frame;
    let feedback =
        ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
    assert!(!feedback.content_nbt.is_empty());

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamerule players_sleeping_percentage 50".to_string(),
        })
        .await
        .expect("set sleeping percentage");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "players_sleeping_percentage = 50"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamerule players_sleeping_percentage".to_string(),
        })
        .await
        .expect("query sleeping percentage");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "players_sleeping_percentage = 50"
    );
}

#[tokio::test]
async fn lua_player_command_is_exposed_and_routed_for_non_operator() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("greetings");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "greetings"
            name = "Greetings"
            version = "0.1.0"
            api = "0.3.0"
            player_commands = ["hello"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_command(event)
                solaris.send_message(
                    event.player_id,
                    event.root .. ":" .. event.username .. ":" .. event.arguments
                )
            end
        "#,
    )
    .expect("write plugin source");
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);

    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua player command wire test".into(),
        max_players: 1,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local_addr");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "LuaCommandPlayer")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let commands: ClientboundCommands = client.read_typed().await.expect("Commands");
    let roots = commands.nodes[commands.root_index as usize]
        .children
        .iter()
        .filter_map(|index| match &commands.nodes[*index as usize].kind {
            CommandNodeKind::Literal(root) => Some(root.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(roots.contains(&"hello"));
    assert!(!roots.contains(&"gamemode"));
    assert!(!roots.contains(&"stop"));

    client
        .write_packet(&ServerboundChatCommand {
            command: "hello one  two".to_owned(),
        })
        .await
        .expect("send plugin command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "hello:LuaCommandPlayer:one  two"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "missing".to_owned(),
        })
        .await
        .expect("send unknown command");
    assert_eq!(next_system_chat_text(&mut client).await, "Unknown command");

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_owned(),
        })
        .await
        .expect("send denied built-in command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "You do not have permission to use that command"
    );

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn lua_operator_command_is_hidden_from_non_operators_and_routes_for_operators() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("admin-day");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "admin-day"
            name = "Admin Day"
            version = "0.1.0"
            api = "0.4.0"
            operator_commands = ["adminday"]
            console_commands = ["time"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_command(event)
                solaris.run_console("time set day")
                solaris.send_message(event.player_id, "admin-day:" .. event.username)
            end
        "#,
    )
    .expect("write plugin source");
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);

    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua operator command wire test".into(),
        max_players: 2,
        view_distance: 2,
        data: Arc::new(mc_data::testing::stub()),
        blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
        world: None,
        tags: Arc::new(mc_data::tags::TagsData::default()),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(["LuaOperator"], false),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local_addr");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut non_operator = Client::connect(addr).await.expect("non-operator connect");
    let _ = non_operator
        .drive_login(addr, "LuaCommandPlayer")
        .await
        .expect("non-operator login");
    non_operator
        .drive_configuration()
        .await
        .expect("non-operator configuration");
    let _ = non_operator
        .read_play_login()
        .await
        .expect("non-operator play entry");
    let commands: ClientboundCommands = non_operator.read_typed().await.expect("Commands");
    let roots = commands.nodes[commands.root_index as usize]
        .children
        .iter()
        .filter_map(|index| match &commands.nodes[*index as usize].kind {
            CommandNodeKind::Literal(root) => Some(root.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!roots.contains(&"adminday"));

    non_operator
        .write_packet(&ServerboundChatCommand {
            command: "adminday".to_owned(),
        })
        .await
        .expect("send forged operator plugin command");
    assert_eq!(
        next_system_chat_text(&mut non_operator).await,
        "You do not have permission to use that command"
    );

    let mut operator = Client::connect(addr).await.expect("operator connect");
    let _ = operator
        .drive_login(addr, "LuaOperator")
        .await
        .expect("operator login");
    operator
        .drive_configuration()
        .await
        .expect("operator configuration");
    let _ = operator
        .read_play_login()
        .await
        .expect("operator play entry");
    let commands: ClientboundCommands = operator.read_typed().await.expect("operator Commands");
    let admin_day = commands
        .nodes
        .iter()
        .find(|node| matches!(&node.kind, CommandNodeKind::Literal(root) if root == "adminday"));
    assert!(admin_day.is_some_and(|node| node.restricted && node.executable));

    operator
        .write_packet(&ServerboundChatCommand {
            command: "adminday".to_owned(),
        })
        .await
        .expect("send operator plugin command");
    wait_for_admin_day_effects(&mut operator).await;

    drop(non_operator);
    drop(operator);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server shutdown timeout")
        .expect("server task")
        .expect("server result");
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

#[tokio::test]
async fn normal_chat_broadcasts_to_other_players() {
    let addr = start_server().await;
    let mut alice = Client::connect(addr).await.expect("alice connect");
    let _ = alice
        .drive_login(addr, "M34Alice")
        .await
        .expect("alice login");
    alice
        .drive_configuration()
        .await
        .expect("alice configuration");
    let _ = alice.read_play_login().await.expect("alice play entry");
    let _: ClientboundCommands = alice.read_typed().await.expect("alice Commands");

    let mut bob = Client::connect(addr).await.expect("bob connect");
    let _ = bob.drive_login(addr, "M34Bob").await.expect("bob login");
    bob.drive_configuration().await.expect("bob configuration");
    let _ = bob.read_play_login().await.expect("bob play entry");
    let _: ClientboundCommands = bob.read_typed().await.expect("bob Commands");

    alice
        .write_packet(&ServerboundChat {
            message: "p34 hello".to_string(),
            timestamp_millis: 0,
            salt: 0,
            signature: None,
            last_seen_offset: 0,
            last_seen_acknowledged: [0; 3],
            last_seen_checksum: 0,
        })
        .await
        .expect("send normal chat");

    assert_eq!(
        next_system_chat_text(&mut bob).await,
        "<M34Alice> p34 hello"
    );
}

#[tokio::test]
async fn save_all_and_stop_commands_report_feedback_and_signal_shutdown() {
    let shutdown = mc_net::ShutdownHandle::default();
    let addr = start_server_with_shutdown(shutdown.clone()).await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M35Ops").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");

    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".to_string(),
        })
        .await
        .expect("send save-all command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Saved 0 players, 0 entities, 0 chunks"
    );
    assert!(!shutdown.is_requested());

    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Saved all state; stopping server"
    );
    assert!(shutdown.is_requested());
}

#[tokio::test]
async fn status_command_reports_runtime_control_drain_snapshot() {
    let (addr, runtime_control) = start_server_with_runtime_control().await;
    runtime_control.request_drain();
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M100Status").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");

    client
        .write_packet(&ServerboundChatCommand {
            command: "status".to_string(),
        })
        .await
        .expect("send status command");

    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Runtime control: draining=true action=hold pressure=none limits=view_distance:2,send:1,load:2,generate:3 pressure_ticks=0 healthy_ticks=0 reason=drain active; holding minimum limits"
    );
}

#[tokio::test]
async fn stop_command_requests_runtime_control_drain() {
    let shutdown = mc_net::ShutdownHandle::default();
    let (addr, runtime_control) =
        start_server_with_runtime_control_and_shutdown(shutdown.clone()).await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M100StopDrain")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");

    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");

    assert_eq!(
        next_system_chat_text(&mut client).await,
        "Saved all state; stopping server"
    );
    assert!(shutdown.is_requested());
    assert!(runtime_control.snapshot().draining);
}

#[tokio::test]
async fn client_receives_continuing_world_time_updates() {
    let addr = start_server().await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M38Time").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: ClientboundSetTime = client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    let first = next_time_update(&mut client).await;
    let second = next_time_update(&mut client).await;

    assert!(
        second.game_time > first.game_time,
        "world time updates must advance"
    );
}

async fn next_system_chat_text(client: &mut Client) -> String {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSystemChat::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("system chat frame");
    let packet =
        ClientboundSystemChat::decode(&mut outcome.frame.body.clone()).expect("decode SystemChat");
    text_component_text(&packet)
}

fn text_component_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = Bytes::copy_from_slice(&packet.content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("system chat component must contain text")
}

async fn next_time_update(client: &mut Client) -> ClientboundSetTime {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSetTime::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("world time frame");
    assert!(
        outcome.skipped.bytes <= 32 * 1024,
        "time update skipped too many bytes: {:?}",
        outcome.skipped
    );
    let frame = outcome.frame;
    ClientboundSetTime::decode(&mut frame.body.clone()).expect("decode SetTime")
}

async fn wait_for_admin_day_effects(client: &mut Client) {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut saw_day = false;
        let mut saw_reply = false;
        while !saw_day || !saw_reply {
            let frame = client.read_frame().await.expect("admin-day packet");
            if frame.id == ClientboundSetTime::ID {
                let packet = ClientboundSetTime::decode(&mut frame.body.clone())
                    .expect("decode admin-day SetTime");
                saw_day |= packet.game_time == 1000;
            } else if frame.id == ClientboundSystemChat::ID {
                let packet = ClientboundSystemChat::decode(&mut frame.body.clone())
                    .expect("decode admin-day SystemChat");
                saw_reply |= text_component_text(&packet) == "admin-day:LuaOperator";
            }
        }
    })
    .await
    .expect("admin-day Lua effects timeout");
}
