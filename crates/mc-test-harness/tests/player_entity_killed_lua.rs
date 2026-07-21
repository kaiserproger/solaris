use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundContainerSetSlot, ClientboundKeepAlive,
    ClientboundSystemChat, ConfirmTeleportation, EntityEvent, MovePlayerFlags, ServerboundAttack,
    ServerboundChatCommand, ServerboundKeepAlive, ServerboundMovePlayerPos,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const WIRE_TIMEOUT: Duration = Duration::from_secs(10);
const NETHERITE_AXE_RECHARGE_TICKS: u64 = 20;

#[tokio::test]
async fn player_entity_killed_reaches_lua_once_after_the_lethal_melee_commit() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("entity-kill-wire");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "entity-kill-wire"
            name = "Entity Kill Wire"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.entity_killed"]
            player_commands = ["spawn-kill-target"]
            spawn_entities = ["minecraft:chicken"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            function on_player_entity_killed(event)
                local expected = {
                    name = true, player_id = true, context_verified = true,
                    uuid = true, username = true, operator = true,
                    x = true, y = true, z = true, dimension = true,
                    entity_id = true, entity_type = true, source = true,
                    game_mode = true,
                }
                local field_count = 0
                for field in pairs(event) do
                    assert(expected[field] == true, "unexpected field: " .. field)
                    field_count = field_count + 1
                end
                assert(field_count == 14)
                solaris.send_message(event.player_id, string.format(
                    "entity-killed|name=%s|player_id=%d|context_verified=%s|uuid=%s|username=%s|operator=%s|x=%.3f|y=%.3f|z=%.3f|dimension=%s|entity_id=%d|entity_type=%s|source=%s|game_mode=%s",
                    event.name,
                    event.player_id,
                    tostring(event.context_verified),
                    event.uuid,
                    event.username,
                    tostring(event.operator),
                    event.x,
                    event.y,
                    event.z,
                    event.dimension,
                    event.entity_id,
                    event.entity_type,
                    event.source,
                    event.game_mode
                ))
            end

            function on_player_command(event)
                if event.root == "spawn-kill-target" then
                    solaris.spawn_entity(
                        event.player_id,
                        "minecraft:chicken",
                        event.x + 1.0,
                        event.y,
                        event.z
                    )
                    return
                end
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
    let items = Arc::new(mc_data::items::solaris_required_items());
    let netherite_axe_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:netherite_axe").unwrap())
        .expect("embedded netherite axe");
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let chicken_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:chicken").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded chicken entity type");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 49)
        .with_item_registry(Arc::clone(&items))
        .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua entity kill wire test".into(),
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
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let login = client
        .drive_login(addr, "KillWire")
        .await
        .expect("drive login");
    client.drive_configuration().await.expect("configuration");
    let play = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    client
        .write_packet(&ServerboundChatCommand {
            command: "spawn-kill-target".into(),
        })
        .await
        .expect("request plugin-spawned kill target");
    let chicken = wait_for_entity_spawn(&mut client, chicken_type_id).await;
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: chicken.x,
            y: chicken.y,
            z: chicken.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move beside chicken");

    client
        .write_packet(&ServerboundAttack {
            entity_id: chicken.entity_id,
        })
        .await
        .expect("send nonlethal bare-hand attack");
    wait_for_entity_hurt_without_chat(&mut client, chicken.entity_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:netherite_axe 1 0".into(),
        })
        .await
        .expect("give lethal melee weapon");
    wait_for_slot_stack(&mut client, netherite_axe_id).await;
    wait_for_additional_simulation_ticks(&mut simulation_ticks, NETHERITE_AXE_RECHARGE_TICKS).await;

    let first_expected_payload = format!(
        "entity-killed|name=player.entity_killed|player_id={}|context_verified=true|uuid={}|username=KillWire|operator=true|x={:.3}|y={:.3}|z={:.3}|dimension=minecraft:overworld|entity_id={}|entity_type=minecraft:chicken|source=melee|game_mode=survival",
        play.entity_id, login.uuid, chicken.x, chicken.y, chicken.z, chicken.entity_id,
    );
    client
        .write_packet(&ServerboundAttack {
            entity_id: chicken.entity_id,
        })
        .await
        .expect("send lethal netherite-axe attack");
    wait_for_exact_kill_message(&mut client, &first_expected_payload).await;

    client
        .write_packet(&ServerboundAttack {
            entity_id: chicken.entity_id,
        })
        .await
        .expect("repeat attack against dying chicken");
    client
        .write_packet(&ServerboundChatCommand {
            command: "spawn-kill-target".into(),
        })
        .await
        .expect("request second plugin-spawned kill target");
    let second_chicken = wait_for_entity_spawn(&mut client, chicken_type_id).await;
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: second_chicken.x,
            y: second_chicken.y,
            z: second_chicken.z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move beside second chicken");
    wait_for_additional_simulation_ticks(&mut simulation_ticks, NETHERITE_AXE_RECHARGE_TICKS).await;

    let second_expected_payload = format!(
        "entity-killed|name=player.entity_killed|player_id={}|context_verified=true|uuid={}|username=KillWire|operator=true|x={:.3}|y={:.3}|z={:.3}|dimension=minecraft:overworld|entity_id={}|entity_type=minecraft:chicken|source=melee|game_mode=survival",
        play.entity_id,
        login.uuid,
        second_chicken.x,
        second_chicken.y,
        second_chicken.z,
        second_chicken.entity_id,
    );
    client
        .write_packet(&ServerboundAttack {
            entity_id: second_chicken.entity_id,
        })
        .await
        .expect("kill second chicken after exact recharge");
    wait_for_exact_kill_message(&mut client, &second_expected_payload).await;

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

async fn wait_for_entity_hurt_without_chat(client: &mut Client, entity_id: i32) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("nonlethal entity hurt event");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let packet =
                ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
            panic!(
                "kill event preceded the lethal commit: {}",
                system_chat_text(&packet)
            );
        } else if frame.id == EntityEvent::ID {
            let packet = EntityEvent::decode(&mut frame.body.clone()).expect("decode EntityEvent");
            if packet.entity_id == entity_id && packet.event_id == 2 {
                return;
            }
        }
    }
}

async fn wait_for_exact_kill_message(client: &mut Client, expected: &str) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("committed entity-kill Lua message");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let packet =
                ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
            assert_eq!(system_chat_text(&packet), expected);
            return;
        }
    }
}

async fn wait_for_entity_spawn(client: &mut Client, entity_type_id: i32) -> AddEntity {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("plugin-spawned chicken");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode AddEntity");
            if packet.entity_type_id == entity_type_id {
                return packet;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let packet =
                ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
            panic!(
                "unexpected kill event before second target publication: {}",
                system_chat_text(&packet)
            );
        }
    }
}

async fn wait_for_slot_stack(client: &mut Client, item_id: u32) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    let mut saw_stack = false;
    let mut saw_command_commit = false;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("netherite axe inventory commit");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let packet = ClientboundContainerSetSlot::decode(&mut frame.body.clone())
                .expect("decode ContainerSetSlot");
            if packet.item_stack.item_id == item_id && packet.item_stack.count == 1 {
                saw_stack = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let packet = ClientboundSystemChat::decode(&mut frame.body.clone())
                .expect("decode unexpected SystemChat");
            let message = system_chat_text(&packet);
            assert_eq!(message, "Debug command executed");
            saw_command_commit = true;
        }
        if saw_stack && saw_command_commit {
            return;
        }
    }
}

async fn wait_for_additional_simulation_ticks(
    ticks: &mut tokio::sync::watch::Receiver<u64>,
    additional: u64,
) {
    let target = (*ticks.borrow()).saturating_add(additional);
    tokio::time::timeout(WIRE_TIMEOUT, async {
        loop {
            let current = *ticks.borrow_and_update();
            if current >= target {
                return;
            }
            ticks
                .changed()
                .await
                .expect("simulation tick publisher remains active");
        }
    })
    .await
    .expect("simulation did not reach the exact attack recharge tick");
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let packet = ClientboundKeepAlive::decode(&mut body.clone()).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: packet.id })
        .await
        .expect("echo KeepAlive");
    true
}

fn system_chat_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = Bytes::copy_from_slice(&packet.content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).expect("read text component NBT");
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
