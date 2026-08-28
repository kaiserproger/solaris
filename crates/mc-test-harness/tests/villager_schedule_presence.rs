use std::sync::Arc;
use std::time::Duration;

use mc_entity::villager_26_1_2::{VillagerActivity, VillagerBrainState, VillagerPoiSet};
use mc_entity::{GoalState, SpawnEntity, Vec3, VillagerData, VillagerKind, VillagerProfession};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundSetTime, ConfirmTeleportation, EntityPositionSync, GameEvent, LevelChunkWithLight,
    MoveEntityPos, MoveEntityPosRot, ServerboundChatCommand, ServerboundKeepAlive, SetCenterChunk,
    SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn embedded_villager_schedule_switches_rest_to_work_over_tcp() {
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
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 villager schedule TCP".into(),
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
        .expect("bind villager schedule server");
    let load_bench = bound.load_bench_handle();
    let addr = bound
        .local_addr()
        .expect("villager schedule server address");
    let server = tokio::spawn(async move { bound.serve().await });

    let (mut client, spawn) = connect_to_play(addr, "VillScheduleTcp").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let position = Vec3::new(spawn.x + 4.0, spawn.y - 1.0, spawn.z + 4.0);
    let job_site = Vec3::new(position.x + 4.0, position.y, position.z);
    let mut villager = SpawnEntity::new(villager_type_id, "minecraft:villager", position);
    villager.retained.villager = Some(VillagerData::new(
        VillagerKind::Plains,
        VillagerProfession::None,
        1,
    ));
    let mut brain = VillagerBrainState::adult(VillagerPoiSet {
        home: Some(position),
        job_site: Some(job_site),
        meeting_point: Some(position),
    });
    brain.activity = VillagerActivity::Rest;
    villager.retained.villager_brain = Some(brain);
    villager.goal = GoalState::FollowPosition {
        target: position,
        speed: 0.3,
    };
    let seeded = load_bench.seed_spawn_entities(vec![villager]);
    assert_eq!(seeded.entities, 1);

    let villager = wait_for_entity_type_spawn_at(&mut client, villager_type_id, position).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "time set 2000".to_owned(),
        })
        .await
        .expect("set villager work time");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut saw_time_set = false;
    let mut saw_work_motion = false;
    while !saw_time_set || !saw_work_motion {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!(
                "villager schedule did not converge: time_set={saw_time_set} work_motion={saw_work_motion}"
            );
        }
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "villager schedule TCP lifecycle failed: {error}; time_set={saw_time_set} work_motion={saw_work_motion}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let packet = ClientboundSetTime::decode(&mut frame.body.clone())
                .expect("decode villager schedule time update");
            saw_time_set |= packet
                .overworld_clock
                .is_some_and(|clock| clock.total_ticks >= 2_000);
        } else if frame.id == MoveEntityPos::ID {
            let packet = MoveEntityPos::decode(&mut frame.body.clone())
                .expect("decode villager relative movement");
            saw_work_motion |= packet.entity_id == villager.entity_id && packet.delta_x > 0;
        } else if frame.id == MoveEntityPosRot::ID {
            let packet = MoveEntityPosRot::decode(&mut frame.body.clone())
                .expect("decode villager relative movement+rotation");
            saw_work_motion |= packet.entity_id == villager.entity_id && packet.delta_x > 0;
        } else if frame.id == EntityPositionSync::ID {
            let packet = EntityPositionSync::decode(&mut frame.body.clone())
                .expect("decode villager position sync");
            saw_work_motion |= packet.entity_id == villager.entity_id
                && packet.values.position.x > position.x + 0.01;
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("villager schedule server shutdown timeout")
        .expect("villager schedule server task")
        .expect("villager schedule server result");
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
            .expect("drain villager schedule chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet = LevelChunkWithLight::decode(&mut frame.body.clone())
                .expect("decode villager schedule chunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn wait_for_entity_type_spawn_at(
    client: &mut Client,
    entity_type_id: i32,
    position: Vec3,
) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for villager schedule spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet =
                AddEntity::decode(&mut frame.body.clone()).expect("decode villager schedule spawn");
            if packet.entity_type_id == entity_type_id
                && (packet.x - position.x).abs() < 0.01
                && (packet.y - position.y).abs() < 0.01
                && (packet.z - position.z).abs() < 0.01
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
    let keepalive = ClientboundKeepAlive::decode(&mut body.clone())
        .expect("decode villager schedule keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer villager schedule keepalive");
    true
}
