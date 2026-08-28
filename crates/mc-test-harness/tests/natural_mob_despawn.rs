use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use flate2::read::GzDecoder;
use mc_entity::{SpawnEntity, Vec3};
use mc_nbt::Tag;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundCommands, ClientboundInitializeBorder, ClientboundKeepAlive,
    ClientboundSetTime, ConfirmTeleportation, GameEvent, LevelChunkWithLight, RemoveEntities,
    ServerboundChatCommand, ServerboundKeepAlive, SetCenterChunk, SetDefaultSpawnPosition,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn distant_natural_hostile_hard_despawns_for_nearby_spectator_and_stays_absent_on_disk() {
    let world_dir = tempfile::tempdir().expect("natural despawn disk world");
    std::fs::create_dir_all(world_dir.path().join("region"))
        .expect("create natural despawn region directory");
    let data = Arc::new(mc_data::solaris_required_data());
    let blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .expect("build embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let storage = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("open natural despawn world")
    .with_item_registry(Arc::clone(&items))
    .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
        0,
        Arc::clone(&blocks),
    )));
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let zombie_type_id = entity_type_id(&entity_types, "minecraft:zombie");
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Phase 4 natural mob despawn TCP".into(),
        max_players: 4,
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
        .expect("bind natural despawn server");
    let load_bench = bound.load_bench_handle();
    let addr = bound.local_addr().expect("natural despawn server address");
    let server = tokio::spawn(async move { bound.serve_and_save().await });

    let (mut survival, survival_spawn) = connect_to_play(addr, "DespawnSurvive").await;
    drain_until_chunk(&mut survival, (0, 0)).await;
    let (mut spectator, _) = connect_to_play(addr, "DespawnSpectate").await;
    drain_until_chunk(&mut spectator, (0, 0)).await;
    set_spectator_and_wait(&mut spectator).await;

    let zombie_position = Vec3::new(
        survival_spawn.x + 4.0,
        survival_spawn.y,
        survival_spawn.z + 4.0,
    );
    let seeded = load_bench.seed_natural_entities(vec![SpawnEntity::new(
        zombie_type_id,
        "minecraft:zombie",
        zombie_position,
    )]);
    assert_eq!(seeded.entities, 1);
    assert_eq!(seeded.hostile_entities, 1);
    let zombie_id = wait_for_entity_spawn(&mut spectator, zombie_type_id, zombie_position).await;
    assert_eq!(load_bench.readiness().owner_entities, 1);

    teleport_and_wait(
        &mut survival,
        survival_spawn.x + 200.0,
        survival_spawn.y,
        survival_spawn.z,
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_remove = false;
    while !saw_remove {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "spectator never observed hard despawn"
        );
        let frame = spectator
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for hard despawn wire removal");
        if handle_keepalive(&mut spectator, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == RemoveEntities::ID {
            let packet = RemoveEntities::decode(&mut frame.body.clone())
                .expect("decode hard despawn RemoveEntities");
            saw_remove = packet.entity_ids.contains(&zombie_id);
        }
    }
    assert_eq!(
        load_bench.readiness().owner_entities,
        0,
        "wire removal must follow the hard-despawn regional-owner commit"
    );

    drop(survival);
    drop(spectator);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("natural despawn server shutdown timeout")
        .expect("natural despawn server task")
        .expect("natural despawn serve/save result");
    assert_eq!(persisted_entity_count(world_dir.path()), 0);
}

fn entity_type_id(registry: &mc_data::entity_types::EntityTypeRegistry, name: &str) -> i32 {
    registry
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .unwrap_or_else(|| panic!("missing entity type {name}"))
}

async fn set_spectator_and_wait(client: &mut Client) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode spectator".to_owned(),
        })
        .await
        .expect("switch observer to spectator");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for spectator game mode");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == GameEvent::ID {
            let event = GameEvent::decode(&mut frame.body.clone()).expect("decode game mode event");
            if event.event == GameEvent::EVENT_CHANGE_GAME_MODE && event.value == 3.0 {
                return;
            }
        }
    }
}

async fn teleport_and_wait(client: &mut Client, x: f64, y: f64, z: f64) {
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("tp {x} {y} {z}"),
        })
        .await
        .expect("teleport survival despawn anchor");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for survival teleport");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let sync = SynchronizePlayerPosition::decode(&mut frame.body.clone())
                .expect("decode survival teleport");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await
                .expect("confirm survival teleport");
            return;
        }
    }
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
            .expect("drain natural despawn chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet = LevelChunkWithLight::decode(&mut frame.body.clone())
                .expect("decode natural despawn chunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn wait_for_entity_spawn(client: &mut Client, type_id: i32, position: Vec3) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for natural zombie spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let packet = AddEntity::decode(&mut frame.body.clone()).expect("decode natural zombie");
            if packet.entity_type_id == type_id
                && (packet.x - position.x).abs() < 0.01
                && (packet.y - position.y).abs() < 0.01
                && (packet.z - position.z).abs() < 0.01
            {
                return packet.entity_id;
            }
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let keepalive =
        ClientboundKeepAlive::decode(&mut body.clone()).expect("decode natural despawn keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer natural despawn keepalive");
    true
}

fn persisted_entity_count(world_root: &std::path::Path) -> usize {
    let path = world_root.join("solaris").join("entities.dat");
    let file = File::open(&path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("decompress {}: {error}", path.display()));
    let mut input = bytes.as_slice();
    let (_, root) = mc_nbt::read_named(&mut input)
        .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()));
    let Tag::Compound(fields) = root else {
        panic!("entity checkpoint root must be a compound")
    };
    let entities = fields
        .iter()
        .find_map(|(name, value)| (name == "Entities").then_some(value))
        .expect("entity checkpoint Entities field");
    let Tag::List(entities) = entities else {
        panic!("entity checkpoint Entities must be a list")
    };
    entities.elements.len()
}
