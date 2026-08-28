use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundSetHealth, ClientboundSetTime, ConfirmTeleportation, EntityPositionSync, GameEvent,
    LevelChunkWithLight, MoveEntityPos, MoveEntityPosRot, RemoveEntities, ServerboundChatCommand,
    ServerboundKeepAlive, SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn embedded_dragon_air_combat_moves_fires_clouds_damages_and_discards_over_tcp() {
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
    let dragon_type_id = entity_type_id(&entity_types, "minecraft:ender_dragon");
    let fireball_type_id = entity_type_id(&entity_types, "minecraft:dragon_fireball");
    let cloud_type_id = entity_type_id(&entity_types, "minecraft:area_effect_cloud");
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 dragon D1 TCP".into(),
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
    let bound = mc_net::bind(cfg).await.expect("bind dragon server");
    let addr = bound.local_addr().expect("dragon server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let (mut client, spawn) = connect_to_play(addr, "DragonTcp").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let dragon_position = (spawn.x, spawn.y + 3.0, spawn.z + 24.0);
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:ender_dragon {} {} {}",
                dragon_position.0, dragon_position.1, dragon_position.2
            ),
        })
        .await
        .expect("summon dragon");
    let dragon = wait_for_entity_type_spawn_at(&mut client, dragon_type_id, dragon_position).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut saw_motion = false;
    let mut fireball_id = None;
    let mut cloud_id = None;
    let mut saw_damage = false;
    let mut saw_fireball_remove = false;
    while !saw_motion
        || fireball_id.is_none()
        || cloud_id.is_none()
        || !saw_damage
        || !saw_fireball_remove
    {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "dragon D1 TCP lifecycle did not converge: dragon={} motion={saw_motion} fireball={fireball_id:?} cloud={cloud_id:?} damage={saw_damage} fireball_removed={saw_fireball_remove}",
                dragon.entity_id,
            );
        }
        let frame = client.read_frame_with_timeout(remaining).await.unwrap_or_else(|error| {
            panic!(
                "dragon D1 TCP lifecycle failed: {error}; dragon={} motion={saw_motion} fireball={fireball_id:?} cloud={cloud_id:?} damage={saw_damage} fireball_removed={saw_fireball_remove}",
                dragon.entity_id,
            )
        });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet =
                AddEntity::decode(&mut frame.body.clone()).expect("decode dragon child entity");
            if packet.entity_type_id == fireball_type_id {
                fireball_id.get_or_insert(packet.entity_id);
            } else if packet.entity_type_id == cloud_type_id {
                cloud_id.get_or_insert(packet.entity_id);
            }
        } else if frame.id == MoveEntityPos::ID {
            let packet =
                MoveEntityPos::decode(&mut frame.body.clone()).expect("decode dragon move");
            saw_motion |= packet.entity_id == dragon.entity_id
                && (packet.delta_x != 0 || packet.delta_y != 0 || packet.delta_z != 0);
        } else if frame.id == MoveEntityPosRot::ID {
            let packet = MoveEntityPosRot::decode(&mut frame.body.clone())
                .expect("decode dragon move+rotation");
            saw_motion |= packet.entity_id == dragon.entity_id
                && (packet.delta_x != 0 || packet.delta_y != 0 || packet.delta_z != 0);
        } else if frame.id == EntityPositionSync::ID {
            let packet = EntityPositionSync::decode(&mut frame.body.clone())
                .expect("decode dragon position sync");
            saw_motion |= packet.entity_id == dragon.entity_id
                && ((packet.values.position.x - dragon_position.0).abs() > 0.01
                    || (packet.values.position.y - dragon_position.1).abs() > 0.01
                    || (packet.values.position.z - dragon_position.2).abs() > 0.01);
        } else if frame.id == ClientboundSetHealth::ID {
            let health = ClientboundSetHealth::decode(&mut frame.body.clone())
                .expect("decode dragon breath player damage");
            saw_damage |= health.health <= 14.0;
        } else if frame.id == RemoveEntities::ID {
            let removed = RemoveEntities::decode(&mut frame.body.clone())
                .expect("decode dragon fireball removal");
            if let Some(fireball_id) = fireball_id {
                saw_fireball_remove |= removed.entity_ids.contains(&fireball_id);
            }
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("dragon server shutdown timeout")
        .expect("dragon server task")
        .expect("dragon server result");
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
            .expect("drain dragon chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet =
                LevelChunkWithLight::decode(&mut frame.body.clone()).expect("decode dragon chunk");
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
            .expect("wait for dragon spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode dragon spawn");
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
