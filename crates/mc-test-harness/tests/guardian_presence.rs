use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundSetEntityData, ClientboundSetHealth, ClientboundSetTime, ConfirmTeleportation,
    EntityDataValue, EntityEvent, GameEvent, LevelChunkWithLight, ServerboundChatCommand,
    ServerboundKeepAlive, SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;
const GUARDIAN_ATTACK_TARGET_INDEX_26_1_2: u8 = 17;
const GUARDIAN_ATTACK_EVENT_26_1_2: i8 = 21;

#[tokio::test]
async fn embedded_guardian_beam_publishes_target_damages_player_and_resets() {
    let data = Arc::new(mc_data::solaris_required_data());
    let blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .expect("build embedded block registry"),
    );
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let items = Arc::new(mc_data::items::solaris_required_items());
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let guardian_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:guardian").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("guardian entity type");
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 guardian TCP".into(),
        max_players: 2,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(storage))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &blocks_report,
        )),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            simulation_distance: VIEW_DISTANCE,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind guardian server");
    let addr = bound.local_addr().expect("guardian server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let (mut client, spawn) = connect_to_play(addr, "GuardianTcp").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let guardian_position = (spawn.x, spawn.y, spawn.z + 6.0);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:guardian {} {} {}",
                guardian_position.0, guardian_position.1, guardian_position.2
            ),
        })
        .await
        .expect("summon guardian");
    let guardian =
        wait_for_entity_type_spawn_at(&mut client, guardian_type_id, guardian_position).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut saw_target = false;
    let mut saw_attack_event = false;
    let mut saw_damage = false;
    let mut saw_reset = false;
    while !(saw_target && saw_attack_event && saw_damage && saw_reset) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "guardian beam did not converge: target={saw_target} event={saw_attack_event} damage={saw_damage} reset={saw_reset}"
            );
        }
        let frame = client.read_frame_with_timeout(remaining).await.unwrap_or_else(|error| {
            panic!(
                "guardian beam TCP lifecycle failed: {error}; target={saw_target} event={saw_attack_event} damage={saw_damage} reset={saw_reset}"
            )
        });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetEntityData::ID {
            let packet = ClientboundSetEntityData::decode(&mut frame.body.clone())
                .expect("decode guardian metadata");
            if packet.entity_id == guardian.entity_id {
                for value in packet.values {
                    if let EntityDataValue::Int { index, value } = value
                        && index == GUARDIAN_ATTACK_TARGET_INDEX_26_1_2
                    {
                        saw_target |= value > 0;
                        saw_reset |= saw_target && value == 0;
                    }
                }
            }
        } else if frame.id == EntityEvent::ID {
            let event =
                EntityEvent::decode(&mut frame.body.clone()).expect("decode guardian event");
            saw_attack_event |= event.entity_id == guardian.entity_id
                && event.event_id == GUARDIAN_ATTACK_EVENT_26_1_2;
        } else if frame.id == ClientboundSetHealth::ID {
            let health = ClientboundSetHealth::decode(&mut frame.body.clone())
                .expect("decode guardian player damage");
            saw_damage |= health.health < 20.0;
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("guardian server shutdown timeout")
        .expect("guardian server task")
        .expect("guardian server result");
}

async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: ClientboundInitializeBorder = client.read_typed().await.expect("InitializeBorder");
    let _: ClientboundSetTime = client.read_typed().await.expect("SetTime");
    let _: SetDefaultSpawnPosition = client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync)
}

async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain guardian chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet = LevelChunkWithLight::decode(&mut frame.body.clone())
                .expect("decode guardian chunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn wait_for_entity_type_spawn_at(
    client: &mut Client,
    entity_type_id: i32,
    position: (f64, f64, f64),
) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for guardian spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode guardian spawn");
            if packet.entity_type_id == entity_type_id
                && (packet.x - position.0).abs() < 0.01
                && (packet.y - position.1).abs() < 0.01
                && (packet.z - position.2).abs() < 0.01
            {
                return packet;
            }
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let keepalive =
        ClientboundKeepAlive::decode(&mut body.clone()).expect("decode guardian keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer guardian keepalive");
    true
}
