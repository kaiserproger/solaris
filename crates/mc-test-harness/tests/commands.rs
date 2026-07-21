use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundOpenScreen, ClientboundSetTime, ClientboundSystemChat,
    CommandNodeKind, ConfirmTeleportation, ContainerInput, Direction, GameEvent, GameMode,
    HashedStack, HashedStackComponentHashes, InteractionHand, LevelChunkWithLight, MovePlayerFlags,
    PlayerActionKind, ServerboundChat, ServerboundChatCommand, ServerboundContainerClick,
    ServerboundMovePlayerPos, ServerboundMovePlayerStatusOnly, ServerboundPlaceRecipe,
    ServerboundPlayerAction, ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition,
    pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::{Client, FrameWaitLimits};

const LATENCY_SENSITIVE_FRAME_WAIT_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(128),
    max_skipped_bytes: Some(128 * 1024),
};
const LUA_TRANSACTION_FRAME_WAIT_LIMITS: FrameWaitLimits = FrameWaitLimits {
    max_skipped_frames: Some(4096),
    max_skipped_bytes: Some(32 * 1024 * 1024),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
async fn lua_0_6_player_command_reaches_the_server_chat_adapter() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("greetings");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "greetings"
            name = "Greetings"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined"]
            player_commands = ["hello"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            joined_player_id = 0

            function on_player_joined(event)
                joined_player_id = event.player_id
            end

            function on_player_command(event)
                solaris.send_message(
                    event.player_id,
                    "joined:" .. joined_player_id .. ":" .. event.root .. ":" .. event.username .. ":" .. event.arguments
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
        "joined:1:hello:LuaCommandPlayer:one  two"
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
async fn lua_gameplay_events_follow_authoritative_commits() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("block-jobs");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "block-jobs"
            name = "Block Jobs"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.block_broken", "player.block_placed", "player.item_crafted"]
            player_commands = ["block-fence"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_block_broken(event)
                solaris.send_message(
                    event.player_id,
                    "block-broken:" .. event.block_id
                        .. ":" .. event.dimension
                        .. ":" .. event.x
                        .. ":" .. event.y
                        .. ":" .. event.z
                        .. ":" .. event.game_mode
                        .. ":" .. event.username
                )
            end

            function on_player_block_placed(event)
                solaris.send_message(
                    event.player_id,
                    "block-placed:" .. event.block_id
                        .. ":" .. event.dimension
                        .. ":" .. event.x
                        .. ":" .. event.y
                        .. ":" .. event.z
                        .. ":" .. event.game_mode
                        .. ":" .. event.username
                )
            end

            function on_player_item_crafted(event)
                solaris.send_message(
                    event.player_id,
                    "item-crafted:" .. event.item_id
                        .. ":" .. event.count
                        .. ":" .. event.craft_count
                        .. ":" .. event.source
                        .. ":" .. event.game_mode
                        .. ":" .. event.dimension
                        .. ":" .. event.username
                )
            end

            function on_player_command(event)
                solaris.send_message(event.player_id, "block-fence:" .. event.arguments)
            end
        "#,
    )
    .expect("write plugin source");
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 1);

    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default
        .0 as i32;
    let items = Arc::new(mc_data::items::solaris_required_items());
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let oak_log_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak log item");
    let oak_planks_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak planks item");
    let recipes = Arc::new(mc_data::recipes::solaris_required_recipes());
    let oak_planks_recipe = recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        .and_then(|index| i32::try_from(index).ok())
        .expect("embedded oak planks recipe display id");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 49)
        .with_item_registry(Arc::clone(&items))
        .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua block break event wire test".into(),
        max_players: 2,
        view_distance: 2,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes,
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "BreakEvents")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    client
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("report grounded spawn pose");

    let mut chunks = HashSet::new();
    while chunks.len() < 25 {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                LevelChunkWithLight::ID,
                Duration::from_secs(30),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("initial chunk stream");
        let chunk = LevelChunkWithLight::decode(&mut outcome.frame.body.clone())
            .expect("decode initial chunk");
        chunks.insert((chunk.chunk_x, chunk.chunk_z));
    }

    let mut peer = Client::connect(addr).await.expect("peer connect");
    let _ = peer
        .drive_login(addr, "BreakPeer")
        .await
        .expect("peer login");
    peer.drive_configuration()
        .await
        .expect("peer configuration");
    peer.wait_for_frame_id_with_timeout_and_limits(
        ClientboundCommands::ID,
        Duration::from_secs(5),
        LUA_TRANSACTION_FRAME_WAIT_LIMITS,
    )
    .await
    .expect("peer Commands");
    let outcome = peer
        .wait_for_frame_id_with_timeout_and_limits(
            SynchronizePlayerPosition::ID,
            Duration::from_secs(5),
            LUA_TRANSACTION_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("peer SyncPlayerPos");
    let peer_sync = SynchronizePlayerPosition::decode(&mut outcome.frame.body.clone())
        .expect("decode peer SyncPlayerPos");
    peer.write_packet(&ConfirmTeleportation {
        teleport_id: peer_sync.teleport_id,
    })
    .await
    .expect("ack peer teleport");
    peer.write_packet(&ServerboundMovePlayerStatusOnly {
        flags: MovePlayerFlags::new(true, false),
    })
    .await
    .expect("report grounded peer pose");
    let mut peer_chunks = HashSet::new();
    while peer_chunks.len() < 25 {
        let outcome = peer
            .wait_for_frame_id_with_timeout_and_limits(
                LevelChunkWithLight::ID,
                Duration::from_secs(30),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("peer initial chunk stream");
        let chunk = LevelChunkWithLight::decode(&mut outcome.frame.body.clone())
            .expect("decode peer initial chunk");
        peer_chunks.insert((chunk.chunk_x, chunk.chunk_z));
    }
    peer.write_packet(&ServerboundChatCommand {
        command: "gamemode creative".to_owned(),
    })
    .await
    .expect("switch peer to creative");
    loop {
        let outcome = peer
            .wait_for_frame_id_with_timeout_and_limits(
                GameEvent::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("peer creative game mode event");
        let event = GameEvent::decode(&mut outcome.frame.body.clone()).expect("decode GameEvent");
        if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
            && event.value == GameMode::Creative.id() as f32
        {
            break;
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_owned(),
        })
        .await
        .expect("switch to creative");
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                GameEvent::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("creative game mode event");
        let event = GameEvent::decode(&mut outcome.frame.body.clone()).expect("decode GameEvent");
        if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
            && event.value == GameMode::Creative.id() as f32
        {
            break;
        }
    }

    let target = (0, sync.y.floor() as i32 - 2, 0);
    let creative_placement_target = (target.0, target.1 + 1, target.2);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::AbortDestroyBlock,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            sequence: 0,
        })
        .await
        .expect("abort block break");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence abort".to_owned(),
        })
        .await
        .expect("send abort fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-broken:"),
            "abort published a committed block-break event: {message}"
        );
        if message == "block-fence:abort" {
            break;
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".to_owned(),
        })
        .await
        .expect("give creative placement block");
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundContainerSetSlot::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("creative placement item");
        let slot = ClientboundContainerSetSlot::decode(&mut outcome.frame.body.clone())
            .expect("decode creative placement item");
        if slot.item_stack.item_id == dirt_item_id && slot.item_stack.count == 1 {
            break;
        }
    }
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1,
        })
        .await
        .expect("place creative block");
    let expected_placement_message = format!(
        "block-placed:minecraft:dirt:minecraft:overworld:0:{}:0:creative:BreakEvents",
        creative_placement_target.1
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_placement_update = false;
    let mut saw_placement_event = false;
    while !(saw_placement_update && saw_placement_event) {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("creative block placement event wire response");
        if frame.id == BlockUpdate::ID {
            let update =
                BlockUpdate::decode(&mut frame.body).expect("decode placement BlockUpdate");
            if unpack_block_pos(update.position) == creative_placement_target
                && update.state_id != air_state
            {
                saw_placement_update = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if text_component_text(&chat) == expected_placement_message {
                saw_placement_event = true;
            }
        }
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 2,
        })
        .await
        .expect("repeat creative placement into occupied target");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence creative-place-reject".to_owned(),
        })
        .await
        .expect("send creative placement reject fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-placed:"),
            "rejected creative placement published another event: {message}"
        );
        if message == "block-fence:creative-place-reject" {
            break;
        }
    }

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            sequence: 3,
        })
        .await
        .expect("break generated surface block");

    let expected_message = format!(
        "block-broken:minecraft:grass_block:minecraft:overworld:0:{}:0:creative:BreakEvents",
        target.1
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_committed_update = false;
    let mut saw_plugin_event = false;
    let mut observed_messages = Vec::new();
    while !(saw_committed_update && saw_plugin_event) {
        let result = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await;
        let Ok(mut frame) = result else {
            panic!(
                "block break event wire response failed: committed_update={saw_committed_update}, \
                 plugin_event={saw_plugin_event}, messages={observed_messages:?}, \
                 target={target:?}, sync_y={}, error={result:?}",
                sync.y
            );
        };
        if frame.id == BlockUpdate::ID {
            let update = BlockUpdate::decode(&mut frame.body).expect("decode BlockUpdate");
            if unpack_block_pos(update.position) == target && update.state_id == air_state {
                saw_committed_update = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            let message = text_component_text(&chat);
            if message == expected_message {
                saw_plugin_event = true;
            }
            observed_messages.push(message);
        }
    }

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            sequence: 4,
        })
        .await
        .expect("repeat break against air");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence stale".to_owned(),
        })
        .await
        .expect("send stale-break fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-broken:"),
            "rejected repeated break published another event: {message}"
        );
        if message == "block-fence:stale" {
            break;
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode survival".to_owned(),
        })
        .await
        .expect("switch to survival");
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                GameEvent::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("survival game mode event");
        let event = GameEvent::decode(&mut outcome.frame.body.clone()).expect("decode GameEvent");
        if event.event == GameEvent::EVENT_CHANGE_GAME_MODE
            && event.value == GameMode::Survival.id() as f32
        {
            break;
        }
    }

    let survival_target = (1, target.1, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(survival_target.0, survival_target.1, survival_target.2),
            direction: Direction::Up,
            sequence: 5,
        })
        .await
        .expect("start survival break");
    let baseline = next_lua_time_update(&mut client).await.game_time;
    loop {
        let current = next_lua_time_update(&mut client).await.game_time;
        if current.saturating_sub(baseline) >= 40 {
            break;
        }
    }
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: pack_block_pos(survival_target.0, survival_target.1, survival_target.2),
            direction: Direction::Up,
            sequence: 6,
        })
        .await
        .expect("finish survival break");

    let expected_survival_message = format!(
        "block-broken:minecraft:grass_block:minecraft:overworld:1:{}:0:survival:BreakEvents",
        survival_target.1
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_survival_update = false;
    let mut saw_survival_event = false;
    while !(saw_survival_update && saw_survival_event) {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("survival block break event wire response");
        if frame.id == BlockUpdate::ID {
            let update = BlockUpdate::decode(&mut frame.body).expect("decode BlockUpdate");
            if unpack_block_pos(update.position) == survival_target && update.state_id == air_state
            {
                saw_survival_update = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if text_component_text(&chat) == expected_survival_message {
                saw_survival_event = true;
            }
        }
    }

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(survival_target.0, survival_target.1, survival_target.2),
            direction: Direction::Up,
            sequence: 7,
        })
        .await
        .expect("repeat survival break against air");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence survival-repeat".to_owned(),
        })
        .await
        .expect("send survival repeat fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-broken:"),
            "repeated survival break published another event: {message}"
        );
        if message == "block-fence:survival-repeat" {
            break;
        }
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(survival_target.0, survival_target.1 - 1, survival_target.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 8,
        })
        .await
        .expect("place survival block");
    let expected_survival_placement = format!(
        "block-placed:minecraft:dirt:minecraft:overworld:1:{}:0:survival:BreakEvents",
        survival_target.1
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_survival_placement_update = false;
    let mut saw_survival_placement_event = false;
    while !(saw_survival_placement_update && saw_survival_placement_event) {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("survival block placement event wire response");
        if frame.id == BlockUpdate::ID {
            let update =
                BlockUpdate::decode(&mut frame.body).expect("decode survival placement update");
            if unpack_block_pos(update.position) == survival_target && update.state_id != air_state
            {
                saw_survival_placement_update = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if text_component_text(&chat) == expected_survival_placement {
                saw_survival_placement_event = true;
            }
        }
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(survival_target.0, survival_target.1 - 1, survival_target.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 9,
        })
        .await
        .expect("repeat survival placement with empty hand");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence survival-place-reject".to_owned(),
        })
        .await
        .expect("send survival placement reject fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-placed:"),
            "rejected survival placement published another event: {message}"
        );
        if message == "block-fence:survival-place-reject" {
            break;
        }
    }

    let stale_target = (2, target.1, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(stale_target.0, stale_target.1, stale_target.2),
            direction: Direction::Up,
            sequence: 10,
        })
        .await
        .expect("start stale survival break");
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                BlockChangedAck::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("stale break start acknowledgement");
        let ack = BlockChangedAck::decode(&mut outcome.frame.body.clone())
            .expect("decode stale start acknowledgement");
        if ack.sequence == 10 {
            break;
        }
    }
    let baseline = next_lua_time_update(&mut client).await.game_time;
    loop {
        let current = next_lua_time_update(&mut client).await.game_time;
        if current.saturating_sub(baseline) >= 40 {
            break;
        }
    }

    peer.write_packet(&ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: pack_block_pos(stale_target.0, stale_target.1, stale_target.2),
        direction: Direction::Up,
        sequence: 1,
    })
    .await
    .expect("peer invalidates survival break snapshot");
    let expected_peer_message = format!(
        "block-broken:minecraft:grass_block:minecraft:overworld:2:{}:0:creative:BreakPeer",
        stale_target.1
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_peer_update = false;
    let mut saw_peer_event = false;
    while !(saw_peer_update && saw_peer_event) {
        let mut frame = peer
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("peer block-break event wire response");
        if frame.id == BlockUpdate::ID {
            let update = BlockUpdate::decode(&mut frame.body).expect("decode peer BlockUpdate");
            if unpack_block_pos(update.position) == stale_target && update.state_id == air_state {
                saw_peer_update = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if text_component_text(&chat) == expected_peer_message {
                saw_peer_event = true;
            }
        }
    }

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: pack_block_pos(stale_target.0, stale_target.1, stale_target.2),
            direction: Direction::Up,
            sequence: 11,
        })
        .await
        .expect("finish stale survival break");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence owner-stale".to_owned(),
        })
        .await
        .expect("send owner-stale fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("block-broken:"),
            "owner-rejected stale break published an event: {message}"
        );
        if message == "block-fence:owner-stale" {
            break;
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 0".to_owned(),
        })
        .await
        .expect("give exact recipe-book ingredients");
    loop {
        let outcome = client
            .wait_for_frame_id_with_timeout_and_limits(
                ClientboundContainerSetSlot::ID,
                Duration::from_secs(5),
                LUA_TRANSACTION_FRAME_WAIT_LIMITS,
            )
            .await
            .expect("oak log inventory commit");
        let slot = ClientboundContainerSetSlot::decode(&mut outcome.frame.body.clone())
            .expect("decode oak log inventory commit");
        if slot.container_id == 0
            && slot.slot == 36
            && slot.item_stack.item_id == oak_log_item_id
            && slot.item_stack.count == 2
        {
            break;
        }
    }

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("craft maximum oak planks from player inventory");
    let expected_craft_message =
        "item-crafted:minecraft:oak_planks:8:2:inventory:survival:minecraft:overworld:BreakEvents";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut saw_input_commit = false;
    let mut saw_output_commit = false;
    let mut saw_craft_event = false;
    while !(saw_input_commit && saw_output_commit && saw_craft_event) {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("item craft inventory and Lua wire response");
        if frame.id == ClientboundContainerSetSlot::ID {
            let slot = ClientboundContainerSetSlot::decode(&mut frame.body)
                .expect("decode item craft inventory commit");
            assert_eq!(slot.container_id, 0, "craft updated a non-player container");
            if slot.slot == 36 && slot.item_stack.is_empty() {
                saw_input_commit = true;
            } else if slot.item_stack.item_id == oak_planks_item_id && slot.item_stack.count == 8 {
                saw_output_commit = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let chat = ClientboundSystemChat::decode(&mut frame.body).expect("decode SystemChat");
            if text_component_text(&chat) == expected_craft_message {
                saw_craft_event = true;
            }
        }
    }

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("repeat recipe request without inputs");
    client
        .write_packet(&ServerboundChatCommand {
            command: "block-fence craft-repeat".to_owned(),
        })
        .await
        .expect("send missing-input craft fence");
    loop {
        let message = next_lua_transaction_system_chat_text(&mut client).await;
        assert!(
            !message.starts_with("item-crafted:"),
            "missing-input recipe request published another event: {message}"
        );
        if message == "block-fence:craft-repeat" {
            break;
        }
    }

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
async fn lua_zone_entry_reaches_the_owning_plugin_from_normal_player_movement() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("spawn-zone");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "spawn-zone"
            name = "Spawn Zone"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined", "player.zone_entered"]
            capabilities = ["zones"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_joined(event)
                solaris.upsert_zone("market", "minecraft:alpha", 2, -60, 0, 4, -58, 2)
                solaris.send_message(event.player_id, "zone-ready")
            end

            function on_player_zone_entered(event)
                solaris.send_message(event.player_id, "entered:" .. event.zone_id .. ":" .. event.username)
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
        motd: "Lua zone wire test".into(),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
    let _ = client.drive_login(addr, "ZonePlayer").await.expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    assert_eq!(next_system_chat_text(&mut client).await, "zone-ready");
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 3.0,
            y: -59.0,
            z: 1.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("enter plugin zone");

    assert_eq!(
        next_system_chat_text(&mut client).await,
        "entered:market:ZonePlayer"
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
async fn lua_colony_upsert_reaches_the_owning_plugin_with_correlated_result() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("colony-owner");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "colony-owner"
            name = "Colony Owner"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined"]
            capabilities = ["colonies"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            local joined_player = nil

            function on_player_joined(event)
                joined_player = event.player_id
                solaris.upsert_colony(
                    "register-starter",
                    "starter",
                    "Starter Colony",
                    "minecraft:alpha",
                    3,
                    -59,
                    1
                )
            end

            function on_colony_record_result(event)
                solaris.send_message(
                    joined_player,
                    "colony-result:" .. event.request_id .. ":" .. event.colony_id .. ":" .. tostring(event.accepted)
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
        motd: "Lua colony wire test".into(),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
        .drive_login(addr, "ColonyPlayer")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let _: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "colony-result:register-starter:starter:true"
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
async fn lua_villager_order_reaches_the_regional_owner_and_returns_targeted_result() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("colony-orders");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "colony-orders"
            name = "Colony Orders"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined"]
            capabilities = ["colonies"]
            spawn_entities = ["minecraft:villager"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            local joined_player = nil

            function on_player_joined(event)
                joined_player = event.player_id
                solaris.upsert_colony(
                    "register",
                    "starter",
                    "Starter Colony",
                    "minecraft:overworld",
                    8,
                    -59,
                    2
                )
                solaris.spawn_entity(event.player_id, "minecraft:villager", 1, -59, 1)
            end

            function on_colony_record_result(event)
                if event.request_id == "register" and event.accepted then
                    solaris.bind_nearest_villager("bind", "starter", 0, -59, 0, 16)
                end
            end

            function on_colony_villager_binding_result(event)
                if event.binding_token == nil then
                    solaris.send_message(joined_player, "villager-binding:false")
                    return
                end
                solaris.set_villager_order(
                    "home",
                    "starter",
                    event.binding_token,
                    "home"
                )
            end

            function on_colony_villager_order_result(event)
                solaris.send_message(
                    joined_player,
                    "villager-order:" .. event.request_id .. ":" .. event.order .. ":" .. tostring(event.accepted)
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
        motd: "Lua villager order wire test".into(),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
        .drive_login(addr, "ColonyOrder")
        .await
        .expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let _: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "villager-order:home:home:true"
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
async fn lua_inventory_menu_opens_on_the_client_and_routes_click_to_its_owner() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("catalog");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "catalog"
            name = "Catalog"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined", "inventory.menu.clicked"]
            capabilities = ["inventory_menus"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_joined(event)
                solaris.send_message(event.player_id, "menu-command-ready")
                solaris.open_inventory_menu(event.player_id, "market", "Market", {
                    {slot = 0, resource = "minecraft:apple", count = 1}
                })
            end

            function on_inventory_menu_clicked(event)
                solaris.send_message(
                    event.player_id,
                    "clicked:" .. event.menu_id .. ":" .. event.slot .. ":" .. event.click
                )
            end
        "#,
    )
    .expect("write plugin source");
    let observer = plugins.path().join("observer");
    std::fs::create_dir(&observer).expect("create observer plugin directory");
    std::fs::write(
        observer.join("plugin.toml"),
        r#"
            id = "observer"
            name = "Observer"
            version = "0.1.0"
            api = "0.6.0"
            events = ["inventory.menu.clicked"]
            player_commands = ["menu-fence"]
        "#,
    )
    .expect("write observer manifest");
    std::fs::write(
        observer.join("main.lua"),
        r#"
            function on_inventory_menu_clicked(event)
                solaris.send_message(event.player_id, "leaked-menu-click")
            end

            function on_player_command(event)
                solaris.send_message(event.player_id, "observer-fence")
            end
        "#,
    )
    .expect("write observer source");
    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 2);

    let shutdown = mc_net::ShutdownHandle::default();
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("embedded apple item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 49)
        .with_item_registry(Arc::clone(&items))
        .with_generator(generator);
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua inventory menu wire test".into(),
        max_players: 1,
        view_distance: 2,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
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
    let _ = client.drive_login(addr, "MenuPlayer").await.expect("login");
    client.drive_configuration().await.expect("configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "menu-command-ready"
    );

    let open = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundOpenScreen::ID,
            Duration::from_secs(5),
            LATENCY_SENSITIVE_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("script menu open frame");
    let screen = ClientboundOpenScreen::decode(&mut open.frame.body.clone())
        .expect("decode script OpenScreen");
    assert_eq!(screen.menu_type, 0);
    assert_eq!(literal_text_component_text(&screen.title_nbt), "Market");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let content = loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("script menu content frame");
        if frame.id != ClientboundContainerSetContent::ID {
            continue;
        }
        let content = ClientboundContainerSetContent::decode(&mut frame.body.clone())
            .expect("decode script menu content");
        if content.container_id == screen.container_id {
            break content;
        }
    };
    assert_eq!(content.items.len(), 45);
    assert_eq!(content.items[0].item_id, apple_id);
    assert_eq!(content.items[0].count, 1);
    assert_eq!(content.items[0].custom_name, None);

    client
        .write_packet(&ServerboundContainerClick {
            container_id: content.container_id,
            state_id: content.state_id + 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: apple_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send stale script menu click");
    client
        .write_packet(&ServerboundChatCommand {
            command: "menu-fence".to_owned(),
        })
        .await
        .expect("fence stale menu click delivery");
    assert_eq!(next_system_chat_text(&mut client).await, "observer-fence");

    client
        .write_packet(&ServerboundContainerClick {
            container_id: content.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: apple_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("click script menu item");
    assert_eq!(
        next_system_chat_text(&mut client).await,
        "clicked:market:0:primary"
    );
    client
        .write_packet(&ServerboundChatCommand {
            command: "menu-fence".to_owned(),
        })
        .await
        .expect("fence targeted menu click delivery");
    assert_eq!(next_system_chat_text(&mut client).await, "observer-fence");

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
async fn lua_inventory_storage_transaction_commits_and_rejects_stale_storage_atomically() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("currency-shop");
    std::fs::create_dir(&plugin).expect("create currency plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "currency-shop"
            name = "Currency Shop"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.joined", "plugin.storage.cas_result", "inventory.storage_transaction.result"]
            capabilities = ["storage", "inventory_storage_transactions"]
            player_commands = ["purchase"]
        "#,
    )
    .expect("write currency plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            player_id = nil
            currency_key = nil

            function on_player_joined(event)
                player_id = event.player_id
                currency_key = "currency:" .. event.player_id
            end

            function on_plugin_storage_cas_result(event)
                if event.request_id == "seed" then
                    solaris.inventory_storage_transaction(
                        player_id,
                        "purchase",
                        {
                            { resource = "minecraft:gold_nugget", delta = -1 },
                            { resource = "minecraft:apple", delta = 1 }
                        },
                        {
                            { operation = "cas", key = currency_key, expected_version = event.version, value = "1" }
                        }
                    )
                end
            end

            function on_player_command(event)
                if event.root == "purchase" then
                    player_id = event.player_id
                    currency_key = "currency:" .. event.player_id
                    solaris.send_message(event.player_id, "purchase-received")
                    solaris.storage_cas("seed", currency_key, nil, "2")
                end
            end

            function on_inventory_storage_transaction_result(event)
                solaris.send_message(
                    player_id,
                    "tx-result:" .. event.request_id .. ":" .. tostring(event.committed)
                )
                if event.request_id == "purchase" then
                    solaris.inventory_storage_transaction(
                        player_id,
                        "stale",
                        {
                            { resource = "minecraft:gold_nugget", delta = -1 },
                            { resource = "minecraft:apple", delta = 1 }
                        },
                        {
                            { operation = "cas", key = currency_key, expected_version = 1, value = "0" }
                        }
                    )
                end
            end
        "#,
    )
    .expect("write currency plugin source");

    let observer = plugins.path().join("transaction-observer");
    std::fs::create_dir(&observer).expect("create transaction observer directory");
    std::fs::write(
        observer.join("plugin.toml"),
        r#"
            id = "transaction-observer"
            name = "Transaction Observer"
            version = "0.1.0"
            api = "0.6.0"
            events = ["inventory.storage_transaction.result"]
            player_commands = ["transaction-fence"]
        "#,
    )
    .expect("write transaction observer manifest");
    std::fs::write(
        observer.join("main.lua"),
        r#"
            function on_inventory_storage_transaction_result(event)
                solaris.send_message(1, "leaked-result:" .. event.request_id)
            end

            function on_player_command(event)
                if event.root == "transaction-fence" then
                    solaris.send_message(event.player_id, "transaction-fence")
                end
            end
        "#,
    )
    .expect("write transaction observer source");

    let (boundary, host) = mc_script::start_lua_host(mc_script::LuaHostConfig::new(plugins.path()))
        .expect("start Lua host");
    assert_eq!(host.loaded_plugins(), 2);

    let world_dir = tempfile::tempdir().expect("disk-backed world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("create world region");
    let block_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&block_report).expect("embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let gold_nugget_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:gold_nugget").unwrap())
        .expect("embedded gold nugget item");
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("embedded apple item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 49)
            .expect("open disk-backed world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua inventory storage transaction wire test".into(),
        max_players: 1,
        view_distance: 1,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &block_report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local address");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "CurrencyPlayer")
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
    assert!(roots.contains(&"give"));
    assert!(roots.contains(&"purchase"));
    assert!(roots.contains(&"transaction-fence"));

    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    let initial = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundContainerSetContent::ID,
            Duration::from_secs(5),
            LUA_TRANSACTION_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("initial inventory content frame");
    let initial = ClientboundContainerSetContent::decode(&mut initial.frame.body.clone())
        .expect("decode initial inventory content");
    assert_eq!(initial.container_id, 0);
    assert_eq!(initial.state_id, 1);
    assert_eq!(initial.items.len(), 46);
    assert!(initial.items.iter().all(|stack| stack.is_empty()));

    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:gold_nugget 2".to_owned(),
        })
        .await
        .expect("give currency through server command");
    let given = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundContainerSetSlot::ID,
            Duration::from_secs(5),
            LUA_TRANSACTION_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("give inventory slot frame");
    let given = ClientboundContainerSetSlot::decode(&mut given.frame.body.clone())
        .expect("decode give inventory slot");
    assert_eq!(given.container_id, 0);
    assert_eq!(given.slot, 9);
    assert_eq!(given.state_id, 2);
    assert_eq!(given.item_stack.item_id, gold_nugget_id);
    assert_eq!(given.item_stack.count, 2);

    client
        .write_packet(&ServerboundChatCommand {
            command: "purchase".to_owned(),
        })
        .await
        .expect("issue purchase transaction command");
    wait_for_lua_transaction_result(&mut client, "purchase-received").await;
    let committed = wait_for_lua_transaction_inventory_snapshot(&mut client).await;
    wait_for_lua_transaction_result(&mut client, "tx-result:purchase:true").await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "transaction-fence".to_owned(),
        })
        .await
        .expect("fence targeted purchase result");
    let purchase_fence_messages = wait_for_lua_transaction_fence(&mut client).await;
    let stale_result_already_seen = purchase_fence_messages
        .iter()
        .any(|message| message == "tx-result:stale:false");
    assert_eq!(committed.container_id, 0);
    assert_eq!(committed.state_id, given.state_id.wrapping_add(1));
    assert_eq!(committed.items[9].item_id, gold_nugget_id);
    assert_eq!(committed.items[9].count, 1);
    assert_eq!(committed.items[10].item_id, apple_id);
    assert_eq!(committed.items[10].count, 1);
    assert!(committed.items[11..45].iter().all(|stack| stack.is_empty()));
    assert!(committed.items[0..9].iter().all(|stack| stack.is_empty()));
    assert!(committed.items[45].is_empty());

    if !stale_result_already_seen {
        wait_for_lua_transaction_result(&mut client, "tx-result:stale:false").await;
    }
    client
        .write_packet(&ServerboundChatCommand {
            command: "transaction-fence".to_owned(),
        })
        .await
        .expect("fence targeted stale result");
    wait_for_lua_transaction_fence(&mut client).await;
    let rejected = request_player_inventory_resync(&mut client, committed.state_id).await;
    assert_eq!(rejected.container_id, 0);
    assert_eq!(rejected.state_id, committed.state_id);
    assert_eq!(rejected.items, committed.items);

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

async fn request_player_inventory_resync(
    client: &mut Client,
    state_id: i32,
) -> ClientboundContainerSetContent {
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("request player inventory resync");
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundContainerSetContent::ID,
            Duration::from_secs(5),
            LUA_TRANSACTION_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("player inventory resync frame");
    ClientboundContainerSetContent::decode(&mut outcome.frame.body.clone())
        .expect("decode player inventory resync")
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
            api = "0.6.0"
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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
    assert_eq!(next_system_chat_text(&mut client).await, "Stopping server");
    assert!(shutdown.is_requested());
}

#[tokio::test]
async fn stop_command_publishes_exact_runtime_control_drain_snapshot() {
    let shutdown = mc_net::ShutdownHandle::default();
    let (addr, runtime_control) =
        start_server_with_runtime_control_and_shutdown(shutdown.clone()).await;
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, "M100Status").await.expect("login");
    client.drive_configuration().await.expect("configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");

    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    assert_eq!(next_system_chat_text(&mut client).await, "Stopping server");
    assert!(shutdown.is_requested());
    let snapshot = runtime_control.snapshot();
    assert!(snapshot.draining);
    assert_eq!(
        snapshot.last_decision.action,
        mc_net::AutoscaleAction::ScaleDown
    );
    assert_eq!(snapshot.last_decision.pressure, None);
    assert_eq!(
        snapshot.limits,
        mc_net::RuntimeControlLimits {
            view_distance: 2,
            chunk_send_rate: 1,
            chunk_load_rate: 2,
            chunk_generate_rate: 3,
        }
    );
    assert_eq!(
        snapshot.last_decision.reason,
        "drain requested; clamped to minimum chunk throughput"
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

    assert_eq!(next_system_chat_text(&mut client).await, "Stopping server");
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
    next_system_chat_text_with_limits(client, LATENCY_SENSITIVE_FRAME_WAIT_LIMITS).await
}

async fn next_lua_transaction_system_chat_text(client: &mut Client) -> String {
    next_system_chat_text_with_limits(client, LUA_TRANSACTION_FRAME_WAIT_LIMITS).await
}

async fn wait_for_lua_transaction_result(client: &mut Client, expected: &str) {
    loop {
        let message = next_lua_transaction_system_chat_text(client).await;
        assert_no_lua_transaction_leak(&message);
        if message == expected {
            return;
        }
    }
}

async fn wait_for_lua_transaction_inventory_snapshot(
    client: &mut Client,
) -> ClientboundContainerSetContent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut frame = client.read_frame().await.expect("transaction wire frame");
            if frame.id == ClientboundContainerSetContent::ID {
                return ClientboundContainerSetContent::decode(&mut frame.body)
                    .expect("decode transaction inventory snapshot");
            }
            if frame.id == ClientboundSystemChat::ID {
                let packet = ClientboundSystemChat::decode(&mut frame.body)
                    .expect("decode transaction system chat");
                let message = text_component_text(&packet);
                assert_no_lua_transaction_leak(&message);
                assert_ne!(
                    message, "tx-result:purchase:true",
                    "transaction result reached the client before its authoritative inventory snapshot"
                );
            }
        }
    })
    .await
    .expect("authoritative transaction inventory snapshot timeout")
}

async fn wait_for_lua_transaction_fence(client: &mut Client) -> Vec<String> {
    let mut messages = Vec::new();
    loop {
        let message = next_lua_transaction_system_chat_text(client).await;
        assert_no_lua_transaction_leak(&message);
        if message == "transaction-fence" {
            return messages;
        }
        messages.push(message);
    }
}

fn assert_no_lua_transaction_leak(message: &str) {
    assert!(
        !message.starts_with("leaked-result:"),
        "targeted inventory transaction result leaked to observer plugin: {message}"
    );
}

async fn next_system_chat_text_with_limits(client: &mut Client, limits: FrameWaitLimits) -> String {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSystemChat::ID,
            Duration::from_secs(5),
            limits,
        )
        .await
        .expect("system chat frame");
    let packet =
        ClientboundSystemChat::decode(&mut outcome.frame.body.clone()).expect("decode SystemChat");
    text_component_text(&packet)
}

fn text_component_text(packet: &ClientboundSystemChat) -> String {
    literal_text_component_text(&packet.content_nbt)
}

fn literal_text_component_text(component: &[u8]) -> String {
    let mut bytes = Bytes::copy_from_slice(component);
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

async fn next_lua_time_update(client: &mut Client) -> ClientboundSetTime {
    let outcome = client
        .wait_for_frame_id_with_timeout_and_limits(
            ClientboundSetTime::ID,
            Duration::from_secs(5),
            LUA_TRANSACTION_FRAME_WAIT_LIMITS,
        )
        .await
        .expect("Lua gameplay world time frame");
    ClientboundSetTime::decode(&mut outcome.frame.body.clone()).expect("decode SetTime")
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
