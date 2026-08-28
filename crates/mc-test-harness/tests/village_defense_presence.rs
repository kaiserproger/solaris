use std::sync::Arc;
use std::time::Duration;

use mc_entity::villager_26_1_2::{VillagerBrainState, VillagerPoiSet};
use mc_entity::{GoalState, SpawnEntity, Vec3, VillagerData, VillagerKind, VillagerProfession};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundSetTime, ConfirmTeleportation, EntityEvent, GameEvent, LevelChunkWithLight,
    ServerboundKeepAlive, SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;
const GOLEM_ATTACK_EVENT_26_1_2: i8 = 4;

#[tokio::test]
async fn embedded_village_defense_spawns_golem_and_attacks_hostile_over_tcp() {
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
    let villager_type_id = entity_type_id(&entity_types, "minecraft:villager");
    let ravager_type_id = entity_type_id(&entity_types, "minecraft:ravager");
    let iron_golem_type_id = entity_type_id(&entity_types, "minecraft:iron_golem");
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 village defence TCP".into(),
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
    let bound = mc_net::bind(cfg)
        .await
        .expect("bind village defence server");
    let load_bench = bound.load_bench_handle();
    let addr = bound.local_addr().expect("village defence server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let (mut client, spawn) = connect_to_play(addr, "VillageDefTcp").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let village_origin = Vec3::new(spawn.x + 5.0, spawn.y, spawn.z + 5.0);
    let villagers = [
        village_origin,
        Vec3::new(village_origin.x + 1.5, village_origin.y, village_origin.z),
        Vec3::new(village_origin.x, village_origin.y, village_origin.z + 1.5),
    ];
    let threat_position = Vec3::new(village_origin.x, village_origin.y, village_origin.z + 4.0);
    let mut entities = villagers
        .into_iter()
        .map(|position| recently_slept_villager(villager_type_id, position))
        .collect::<Vec<_>>();
    let mut threat = SpawnEntity::new(ravager_type_id, "minecraft:ravager", threat_position);
    threat.goal = GoalState::Idle;
    entities.push(threat);
    let seeded = load_bench.seed_spawn_entities(entities);
    assert_eq!(seeded.entities, 4);
    assert_eq!(seeded.hostile_entities, 1);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut golem_id = None;
    let mut saw_attack = false;
    while golem_id.is_none() || !saw_attack {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "village defence TCP lifecycle did not converge: golem={golem_id:?} attack={saw_attack}"
            );
        }
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "village defence TCP lifecycle failed: {error}; golem={golem_id:?} attack={saw_attack}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode entity spawn");
            if packet.entity_type_id == iron_golem_type_id {
                golem_id = Some(packet.entity_id);
            }
        } else if frame.id == EntityEvent::ID {
            let event = EntityEvent::decode(&mut frame.body.clone()).expect("decode entity event");
            saw_attack |=
                golem_id == Some(event.entity_id) && event.event_id == GOLEM_ATTACK_EVENT_26_1_2;
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("village defence server shutdown timeout")
        .expect("village defence server task")
        .expect("village defence server result");
}

fn recently_slept_villager(type_id: i32, position: Vec3) -> SpawnEntity {
    let mut entity = SpawnEntity::new(type_id, "minecraft:villager", position);
    entity.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::None,
        1,
    ));
    let mut brain = VillagerBrainState::adult(VillagerPoiSet {
        home: Some(position),
        job_site: None,
        meeting_point: Some(position),
    });
    brain.last_slept_tick = Some(0);
    entity.retained.villager_brain = Some(brain);
    entity.goal = GoalState::Idle;
    entity
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
            .expect("drain village defence chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet = LevelChunkWithLight::decode(&mut frame.body.clone())
                .expect("decode village defence chunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let keepalive =
        ClientboundKeepAlive::decode(&mut body.clone()).expect("decode village defence keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer village defence keepalive");
    true
}
