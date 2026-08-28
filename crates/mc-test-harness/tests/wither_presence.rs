use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundExplode, ClientboundInitializeBorder,
    ClientboundKeepAlive, ClientboundSetHealth, ClientboundSetTime, ConfirmTeleportation,
    GameEvent, LevelChunkWithLight, RemoveEntities, ServerboundChatCommand, ServerboundKeepAlive,
    SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn embedded_wither_fires_skull_that_damages_explodes_and_discards_over_tcp() {
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
    let wither_type_id = entity_type_id(&entity_types, "minecraft:wither");
    let skull_type_id = entity_type_id(&entity_types, "minecraft:wither_skull");
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 wither TCP".into(),
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
    let bound = mc_net::bind(cfg).await.expect("bind wither server");
    let addr = bound.local_addr().expect("wither server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let (mut client, spawn) = connect_to_play(addr, "WitherTcp").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let wither_position = (spawn.x, spawn.y, spawn.z + 8.0);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:wither {} {} {}",
                wither_position.0, wither_position.1, wither_position.2
            ),
        })
        .await
        .expect("summon wither");
    let wither = wait_for_entity_type_spawn_at(&mut client, wither_type_id, wither_position).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut skull_id = None;
    let mut saw_damage = false;
    let mut saw_explosion = false;
    let mut saw_remove = false;
    while skull_id.is_none() || !saw_damage || !saw_explosion || !saw_remove {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "wither skull lifecycle did not converge: wither={} skull={skull_id:?} damage={saw_damage} explosion={saw_explosion} removed={saw_remove}",
                wither.entity_id,
            );
        }
        let frame = client.read_frame_with_timeout(remaining).await.unwrap_or_else(|error| {
            panic!(
                "wither skull TCP lifecycle failed: {error}; wither={} skull={skull_id:?} damage={saw_damage} explosion={saw_explosion} removed={saw_remove}",
                wither.entity_id,
            )
        });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode wither skull");
            if packet.entity_type_id == skull_type_id {
                skull_id.get_or_insert(packet.entity_id);
            }
        } else if frame.id == ClientboundSetHealth::ID {
            let health = ClientboundSetHealth::decode(&mut frame.body.clone())
                .expect("decode wither skull player damage");
            saw_damage |= health.health <= 12.0;
        } else if frame.id == ClientboundExplode::ID {
            let explosion = ClientboundExplode::decode(&mut frame.body.clone())
                .expect("decode wither skull explosion");
            saw_explosion |= (explosion.radius - 1.0).abs() < f32::EPSILON;
        } else if frame.id == RemoveEntities::ID {
            let removed = RemoveEntities::decode(&mut frame.body.clone())
                .expect("decode wither skull removal");
            if let Some(skull_id) = skull_id {
                saw_remove |= removed.entity_ids.contains(&skull_id);
            }
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("wither server shutdown timeout")
        .expect("wither server task")
        .expect("wither server result");
}

fn entity_type_id(registry: &mc_data::entity_types::EntityTypeRegistry, name: &str) -> i32 {
    registry
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or_else(|| panic!("missing entity type {name}"))
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
            .expect("drain wither chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet =
                LevelChunkWithLight::decode(&mut frame.body.clone()).expect("decode wither chunk");
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
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for wither spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode wither spawn");
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
    let keepalive = ClientboundKeepAlive::decode(&mut body.clone()).expect("decode keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer keepalive");
    true
}
