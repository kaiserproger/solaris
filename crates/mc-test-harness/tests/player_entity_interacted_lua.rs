use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundKeepAlive, ClientboundSystemChat,
    ConfirmTeleportation, EntityVec3, InteractionHand, MovePlayerFlags, ServerboundChatCommand,
    ServerboundInteract, ServerboundKeepAlive, ServerboundMovePlayerPos, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const WIRE_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn accepted_entity_interactions_reach_lua_with_exact_authoritative_snapshots() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let plugin = plugins.path().join("entity-interaction-wire");
    std::fs::create_dir(&plugin).expect("create plugin directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"
            id = "entity-interaction-wire"
            name = "Entity Interaction Wire"
            version = "0.1.0"
            api = "0.6.0"
            events = ["player.entity_interacted"]
            player_commands = ["spawn-interaction-target", "interaction-position-fence"]
            spawn_entities = ["minecraft:villager"]
        "#,
    )
    .expect("write plugin manifest");
    std::fs::write(
        plugin.join("main.lua"),
        r#"
            local spawn_count = 0
            local accepted_x = nil
            local accepted_y = nil
            local accepted_z = nil

            function on_player_entity_interacted(event)
                local expected = {
                    name = true, player_id = true, context_verified = true,
                    uuid = true, username = true, operator = true,
                    x = true, y = true, z = true, dimension = true,
                    entity_id = true, entity_type = true, hand = true,
                    secondary_action = true, game_mode = true,
                }
                local field_count = 0
                for field in pairs(event) do
                    assert(expected[field] == true, "unexpected field: " .. field)
                    field_count = field_count + 1
                end
                assert(field_count == 15)
                solaris.send_message(event.player_id, string.format(
                    "entity-interacted|name=%s|player_id=%d|context_verified=%s|uuid=%s|username=%s|operator=%s|x=%.3f|y=%.3f|z=%.3f|dimension=%s|entity_id=%d|entity_type=%s|hand=%s|secondary_action=%s|game_mode=%s",
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
                    event.hand,
                    tostring(event.secondary_action),
                    event.game_mode
                ))
            end

            function on_player_command(event)
                if event.root == "spawn-interaction-target" then
                    spawn_count = spawn_count + 1
                    if spawn_count == 1 then
                        solaris.spawn_entity(
                            event.player_id,
                            "minecraft:villager",
                            event.x + 16.0,
                            event.y,
                            event.z
                        )
                        solaris.send_message(event.player_id, "far-target-ready")
                    elseif spawn_count == 2 then
                        accepted_x = event.x + 3.0
                        accepted_y = event.y
                        accepted_z = event.z
                        solaris.spawn_entity(
                            event.player_id,
                            "minecraft:villager",
                            event.x + 4.0,
                            event.y,
                            event.z
                        )
                        solaris.send_message(event.player_id, "near-target-ready")
                    else
                        error("unexpected target spawn request")
                    end
                    return
                end

                if event.root == "interaction-position-fence" then
                    assert(event.x == accepted_x)
                    assert(event.y == accepted_y)
                    assert(event.z == accepted_z)
                    solaris.send_message(event.player_id, string.format(
                        "interaction-position-accepted|x=%.3f|y=%.3f|z=%.3f",
                        event.x,
                        event.y,
                        event.z
                    ))
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
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let villager_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("embedded villager entity type");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 49)
        .with_item_registry(Arc::clone(&items))
        .with_generator(generator);
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Lua entity interaction wire test".into(),
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
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind_with_scripts(cfg, boundary)
        .await
        .expect("bind scripted server");
    let addr = bound.local_addr().expect("local_addr");
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let login = client
        .drive_login(addr, "InteractWire")
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
            command: "spawn-interaction-target".into(),
        })
        .await
        .expect("request far plugin-spawned villager");
    let far_villager =
        wait_for_target_and_fence(&mut client, villager_type_id, "far-target-ready").await;

    client
        .write_packet(&ServerboundInteract {
            entity_id: far_villager.entity_id,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("interact with unreachable villager");
    client
        .write_packet(&ServerboundInteract {
            entity_id: i32::MAX,
            hand: InteractionHand::OffHand,
            location: EntityVec3::ZERO,
            using_secondary_action: true,
        })
        .await
        .expect("interact with missing entity id");
    client
        .write_packet(&ServerboundChatCommand {
            command: "spawn-interaction-target".into(),
        })
        .await
        .expect("request near plugin-spawned villager");
    let near_villager =
        wait_for_target_and_fence(&mut client, villager_type_id, "near-target-ready").await;

    let accepted_x = near_villager.x - 1.0;
    let accepted_y = near_villager.y;
    let accepted_z = near_villager.z;
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: accepted_x,
            y: accepted_y,
            z: accepted_z,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move beside near villager");
    client
        .write_packet(&ServerboundChatCommand {
            command: "interaction-position-fence".into(),
        })
        .await
        .expect("fence accepted interaction position");
    wait_for_exact_system_chat(
        &mut client,
        &format!(
            "interaction-position-accepted|x={accepted_x:.3}|y={accepted_y:.3}|z={accepted_z:.3}"
        ),
        "accepted movement Lua fence",
    )
    .await;

    let first_expected = format!(
        "entity-interacted|name=player.entity_interacted|player_id={}|context_verified=true|uuid={}|username=InteractWire|operator=true|x={accepted_x:.3}|y={accepted_y:.3}|z={accepted_z:.3}|dimension=minecraft:overworld|entity_id={}|entity_type=minecraft:villager|hand=off_hand|secondary_action=true|game_mode=survival",
        play.entity_id, login.uuid, near_villager.entity_id,
    );
    client
        .write_packet(&ServerboundInteract {
            entity_id: near_villager.entity_id,
            hand: InteractionHand::OffHand,
            location: EntityVec3::ZERO,
            using_secondary_action: true,
        })
        .await
        .expect("interact with near villager using off hand");
    wait_for_exact_system_chat(
        &mut client,
        &first_expected,
        "accepted off-hand entity-interaction Lua event",
    )
    .await;

    let second_expected = format!(
        "entity-interacted|name=player.entity_interacted|player_id={}|context_verified=true|uuid={}|username=InteractWire|operator=true|x={accepted_x:.3}|y={accepted_y:.3}|z={accepted_z:.3}|dimension=minecraft:overworld|entity_id={}|entity_type=minecraft:villager|hand=main_hand|secondary_action=false|game_mode=survival",
        play.entity_id, login.uuid, near_villager.entity_id,
    );
    client
        .write_packet(&ServerboundInteract {
            entity_id: near_villager.entity_id,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("interact with same near villager using main hand");
    wait_for_exact_system_chat(
        &mut client,
        &second_expected,
        "accepted main-hand entity-interaction Lua event",
    )
    .await;

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

async fn wait_for_target_and_fence(
    client: &mut Client,
    entity_type_id: i32,
    expected_fence: &str,
) -> AddEntity {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    let mut target = None;
    let mut saw_fence = false;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| panic!("plugin-spawned villager and Lua fence: {error}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode AddEntity");
            if packet.entity_type_id == entity_type_id {
                target = Some(packet);
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let packet =
                ClientboundSystemChat::decode(&mut frame.body.clone()).expect("decode SystemChat");
            assert_eq!(system_chat_text(&packet), expected_fence);
            saw_fence = true;
        }
        if saw_fence && let Some(target) = target {
            return target;
        }
    }
}

async fn wait_for_exact_system_chat(client: &mut Client, expected: &str, reason: &str) {
    let deadline = tokio::time::Instant::now() + WIRE_TIMEOUT;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| panic!("{reason}: {error}"));
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
