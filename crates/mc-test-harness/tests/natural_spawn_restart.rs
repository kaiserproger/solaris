use std::collections::{HashMap, HashSet};
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
    ClientboundSetTime, ConfirmTeleportation, GameEvent, LevelChunkWithLight, SetCenterChunk,
    SetDefaultSpawnPosition, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn natural_entities_keep_identity_across_save_restart_rejoin() {
    let world_dir = tempfile::tempdir().expect("natural restart disk world");
    std::fs::create_dir_all(world_dir.path().join("region"))
        .expect("create natural restart region directory");
    let data = Arc::new(mc_data::solaris_required_data());
    let blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .expect("build embedded block registry"),
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let zombie_type_id = entity_type_id(&entity_types, "minecraft:zombie");
    let sheep_type_id = entity_type_id(&entity_types, "minecraft:sheep");

    let storage = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("open natural restart world")
    .with_item_registry(Arc::clone(&items))
    .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
        0,
        Arc::clone(&blocks),
    )));
    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = server_config(
        &data,
        &blocks_report,
        &blocks,
        &items,
        &entity_types,
        storage,
        shutdown.clone(),
    );
    let bound = mc_net::bind(cfg)
        .await
        .expect("bind natural restart server");
    let load_bench = bound.load_bench_handle();
    let addr = bound.local_addr().expect("natural restart server address");
    let server = tokio::spawn(async move { bound.serve_and_save().await });

    let (mut client, spawn) = connect_to_play(addr, "RestartSpawn").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let specs = [
        (
            zombie_type_id,
            "minecraft:zombie",
            Vec3::new(spawn.x + 4.0, spawn.y, spawn.z + 4.0),
        ),
        (
            zombie_type_id,
            "minecraft:zombie",
            Vec3::new(spawn.x - 4.0, spawn.y, spawn.z - 4.0),
        ),
        (
            sheep_type_id,
            "minecraft:sheep",
            Vec3::new(spawn.x + 4.0, spawn.y, spawn.z - 4.0),
        ),
    ];
    let seeded = load_bench.seed_natural_entities(
        specs
            .iter()
            .map(|(type_id, name, position)| SpawnEntity::new(*type_id, *name, *position))
            .collect(),
    );
    assert_eq!(seeded.entities, 3);
    let mut seeded_uuids = HashSet::new();
    let specs_key: Vec<(i32, Vec3)> = specs.iter().map(|(id, _, pos)| (*id, *pos)).collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while seeded_uuids.len() < specs_key.len() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "seeded natural spawns never arrived");
        let frame = client
            .read_frame_with_timeout(remaining.min(Duration::from_secs(5)))
            .await
            .expect("wait for natural restart spawns");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != AddEntity::ID {
            continue;
        }
        let packet =
            AddEntity::decode(&mut frame.body.clone()).expect("decode natural restart spawn");
        if specs_key.iter().any(|(type_id, position)| {
            packet.entity_type_id == *type_id
                && (packet.x - position.x).abs() < 0.01
                && (packet.y - position.y).abs() < 0.01
                && (packet.z - position.z).abs() < 0.01
        }) {
            assert!(
                seeded_uuids.insert(packet.uuid),
                "seeded natural entities must have distinct identities"
            );
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("natural restart server shutdown timeout")
        .expect("natural restart server task")
        .expect("natural restart serve/save result");
    drop(load_bench);
    let persisted_before = persisted_entity_uuids(world_dir.path());
    for uuid in &seeded_uuids {
        assert!(
            persisted_before.contains(uuid),
            "seeded natural entity {uuid} must survive the save"
        );
    }

    let second_storage = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("reopen natural restart world")
    .with_item_registry(Arc::clone(&items))
    .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
        0,
        Arc::clone(&blocks),
    )));
    let second_shutdown = mc_net::ShutdownHandle::default();
    let second_cfg = server_config(
        &data,
        &blocks_report,
        &blocks,
        &items,
        &entity_types,
        second_storage,
        second_shutdown.clone(),
    );
    let second_bound = mc_net::bind(second_cfg)
        .await
        .expect("bind restarted natural spawn server");
    let second_load_bench = second_bound.load_bench_handle();
    let second_addr = second_bound
        .local_addr()
        .expect("restarted natural spawn address");
    let second_server = tokio::spawn(async move { second_bound.serve_and_save().await });

    let (mut rejoined, _) = connect_to_play(second_addr, "RestartSpawn").await;
    drain_until_chunk(&mut rejoined, (0, 0)).await;
    let observed = collect_entity_uuids(&mut rejoined, &persisted_before).await;
    assert_eq!(
        observed, persisted_before,
        "restart must restore exactly the saved population: no loss, no duplicates, no extras"
    );
    assert_eq!(
        second_load_bench.readiness().owner_entities,
        persisted_before.len(),
        "owner population after restart must match the saved set"
    );

    drop(rejoined);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(30), second_server)
        .await
        .expect("restarted natural spawn shutdown timeout")
        .expect("restarted natural spawn server task")
        .expect("restarted natural spawn serve/save result");
    assert_eq!(
        persisted_entity_uuids(world_dir.path()),
        persisted_before,
        "second save must preserve the same identity set"
    );
}

fn server_config(
    data: &Arc<mc_data::VanillaData>,
    blocks_report: &[mc_data::blocks::BlockReport],
    blocks: &Arc<mc_world::BlockRegistry>,
    items: &Arc<mc_data::items::ItemRegistry>,
    entity_types: &Arc<mc_data::entity_types::EntityTypeRegistry>,
    storage: mc_world::WorldStorage,
    shutdown: mc_net::ShutdownHandle,
) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "natural spawn restart identity".into(),
        max_players: 4,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(data),
        blocks: Arc::clone(blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(storage))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items: Arc::clone(items),
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            blocks_report,
        )),
        entity_types: Arc::clone(entity_types),
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
        shutdown,
    }
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
            .expect("drain natural restart chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let packet = LevelChunkWithLight::decode(&mut frame.body.clone())
                .expect("decode natural restart chunk");
            if (packet.chunk_x, packet.chunk_z) == target {
                return;
            }
        }
    }
}

async fn collect_entity_uuids(
    client: &mut Client,
    expected: &HashSet<uuid::Uuid>,
) -> HashSet<uuid::Uuid> {
    let mut observed = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while observed != *expected {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "restart never published {expected:?}");
        let frame = client
            .read_frame_with_timeout(remaining.min(Duration::from_secs(5)))
            .await
            .expect("collect restarted natural entities");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != AddEntity::ID {
            continue;
        }
        let packet =
            AddEntity::decode(&mut frame.body.clone()).expect("decode restarted natural spawn");
        assert!(
            expected.contains(&packet.uuid),
            "restarted world published unexpected entity {}",
            packet.uuid
        );
        assert!(
            observed.insert(packet.uuid),
            "restart published a duplicate identity"
        );
    }
    let settle_until = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = settle_until.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(_) => break,
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != AddEntity::ID {
            continue;
        }
        let packet =
            AddEntity::decode(&mut frame.body.clone()).expect("decode settle natural spawn");
        assert!(
            expected.contains(&packet.uuid),
            "settle window published unexpected entity {}",
            packet.uuid
        );
        assert!(
            observed.insert(packet.uuid),
            "settle window published a duplicate identity"
        );
    }
    observed
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let keepalive =
        ClientboundKeepAlive::decode(&mut body.clone()).expect("decode natural restart keepalive");
    client
        .write_packet(&mc_protocol::packets::play::ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("answer natural restart keepalive");
    true
}

fn persisted_entity_uuids(world_root: &std::path::Path) -> HashSet<uuid::Uuid> {
    persisted_entity_records(world_root).into_keys().collect()
}

fn persisted_entity_records(world_root: &std::path::Path) -> HashMap<uuid::Uuid, String> {
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
    let mut records = HashMap::new();
    for (index, element) in entities.elements.iter().enumerate() {
        let Tag::Compound(fields) = element else {
            panic!("entity checkpoint element {index} must be a compound");
        };
        let Some(type_name) = fields.iter().find_map(|(name, value)| {
            (name == "id")
                .then_some(value)
                .and_then(|value| match value {
                    Tag::String(name) => Some(name.clone()),
                    _ => None,
                })
        }) else {
            panic!("entity checkpoint element {index} is missing its id");
        };
        let Some(uuid) = fields.iter().find_map(|(name, value)| {
            (name == "UUID")
                .then_some(value)
                .and_then(|value| match value {
                    Tag::IntArray(values) if values.len() == 4 => {
                        let mut bytes = [0_u8; 16];
                        for (idx, word) in values.iter().enumerate() {
                            bytes[idx * 4..idx * 4 + 4].copy_from_slice(&word.to_be_bytes());
                        }
                        Some(uuid::Uuid::from_u128(u128::from_be_bytes(bytes)))
                    }
                    _ => None,
                })
        }) else {
            panic!("entity checkpoint element {index} is missing its UUID");
        };
        records.insert(uuid, type_name);
    }
    records
}
