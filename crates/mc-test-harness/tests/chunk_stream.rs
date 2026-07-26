//! M3.g / M4.f — raw-TCP integration test for the chunk-streaming
//! spawn burst.
//!
//! Boots `mc_net::run` on an ephemeral port with a real
//! in-memory generated `WorldStorage`, walks
//! Handshake → Login → Configuration → Play exactly as a vanilla
//! client does, and asserts that the M3.e view-distance ring around
//! spawn arrives as `LevelChunkWithLight` packets — each with the
//! expected coordinates, non-empty client-usage heightmaps, and a
//! non-empty chunk-data blob.
//!
//! M4.f extends the assertions to the bundled `LightData` payload:
//! every chunk's sky / block masks must cover all 26 wire slots
//! between the present and empty channels, the layer counts must match
//! the mask popcount, and the spawn chunk's above-world slot must ship
//! 0xFF nibbles (open sky).
//!
//! The spawn-window guard is ignored by default because it requires
//! gitignored local vanilla sidecar reports. Run it explicitly when
//! claiming generated-world or light-path coverage.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockUpdate, ClientboundKeepAlive, ConfirmTeleportation, ForgetLevelChunk, GameEvent,
    LevelChunkWithLight, MovePlayerFlags, SectionBlocksUpdate, ServerboundKeepAlive,
    ServerboundMovePlayerPos, SetCenterChunk, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

/// Matches the default `[server].view_distance` in `example.toml`.
/// Hard-coded here on purpose: a regression that quietly raises the
/// default should fail this test by overshooting the bound, not silently pass.
const VIEW_DISTANCE: i32 = 8;
const MOVEMENT_VIEW_DISTANCE: i32 = 2;

#[tokio::test]
#[ignore]
async fn vanilla_client_receives_spawn_view_distance_window() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    assert!(
        blocks_json.exists(),
        "M97 generated-world blocker guard requires {}; run tools/extract-vanilla-data.sh --reports",
        blocks_json.display()
    );
    assert!(
        block_light_path.exists(),
        "M97 generated-world light-path guard requires {}; run tools/extract-block-light.sh",
        block_light_path.display()
    );

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    let block_light = Some(Arc::new(
        mc_data::block_light::load(&block_light_path).expect("block light report loads"),
    ));

    let policy = mc_net::ChunkPipelinePolicy::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M3.g chunk stream".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: policy,
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let chunk_pipeline_metrics = bound.chunk_pipeline_metrics();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M3gTester")
        .await
        .expect("drive login");
    let lock_pressure_before = mc_net::lock_pressure_snapshot();
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    // Spawn burst, in the order `mc_net::play::handle` emits it.
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let event: GameEvent = client.read_typed().await.expect("GameEvent");
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);
    let center: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    assert_eq!(
        (center.chunk_x, center.chunk_z),
        (0, 0),
        "spawn (0.5, 0.5) anchors at chunk (0, 0)"
    );

    // Ack the teleport so the server's keepalive scheduler doesn't see
    // a desync warning. The chunk stream is independent of this ack —
    // the M3.d/M3.e path emits chunks before the keepalive loop — but
    // doing it keeps the test honest about the M1.g handshake.
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    // 24 sections × minimum 8 bytes (i16 non_air + i16 fluid + u8 bpe +
    // VarInt(0) block id + u8 bpe + VarInt biome) = 192. Any chunk
    // emitted by `encode_chunk_data` must clear this floor regardless of
    // whether vanilla's generator filled the chunk in or wrote it as a
    // Status: empty placeholder.
    const MIN_CHUNK_DATA_BYTES: usize = 24 * 8;

    let expected_count = (2 * VIEW_DISTANCE + 1).pow(2) as usize; // 17×17 = 289
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    // Cap per-chunk client-usage heightmaps so the spawn chunk remains
    // the explicit canary for end-to-end heightmap encoding.
    let mut chunks_with_heightmaps = 0usize;
    let mut spawn_heightmaps_seen = 0usize;
    // 180 s gives the M4.f light-bearing burst comfortable margin in
    // debug builds; the M3.g shape-only burst finishes in ~5 s, M4.f's
    // BFS-per-chunk run brings that up to ~30-40 s in isolation and
    // ~50-70 s when other workspace tests run in parallel.
    let timeout = Duration::from_secs(180);
    let deadline = tokio::time::Instant::now() + timeout;
    while seen.len() < expected_count {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(f) => f,
            Err(err) => panic!(
                "expected {} chunks within {:?}, only saw {}: {err}",
                expected_count,
                timeout,
                seen.len(),
            ),
        };
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        assert!(
            (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_x)
                && (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_z),
            "chunk ({}, {}) outside the expected ring ±{VIEW_DISTANCE}",
            pkt.chunk_x,
            pkt.chunk_z
        );
        assert!(
            pkt.data.len() >= MIN_CHUNK_DATA_BYTES,
            "chunk ({}, {}) payload {} bytes < {MIN_CHUNK_DATA_BYTES} byte floor",
            pkt.chunk_x,
            pkt.chunk_z,
            pkt.data.len()
        );
        for hm in &pkt.heightmaps {
            assert!(
                !hm.data.is_empty(),
                "heightmap type_id={} long[] must be populated for chunk ({}, {})",
                hm.type_id,
                pkt.chunk_x,
                pkt.chunk_z
            );
        }
        if !pkt.heightmaps.is_empty() {
            chunks_with_heightmaps += 1;
        }
        if (pkt.chunk_x, pkt.chunk_z) == (0, 0) {
            spawn_heightmaps_seen = pkt.heightmaps.len();
        }
        assert_light_invariants(&pkt);
        let fresh = seen.insert((pkt.chunk_x, pkt.chunk_z));
        assert!(
            fresh,
            "duplicate chunk ({}, {}) on the wire",
            pkt.chunk_x, pkt.chunk_z
        );
    }

    assert_eq!(
        seen.len(),
        expected_count,
        "every chunk in the spawn view-distance ring must be on the wire"
    );

    // Lattice spot-check: the (cx, cz) set is exactly [-VD..=VD]^2.
    for cz in -VIEW_DISTANCE..=VIEW_DISTANCE {
        for cx in -VIEW_DISTANCE..=VIEW_DISTANCE {
            assert!(
                seen.contains(&(cx, cz)),
                "missing chunk ({cx}, {cz}) from the view-distance ring"
            );
        }
    }

    // Generated chunks carry heightmaps end-to-end; the spawn chunk is
    // the strongest canary because every client must receive it first.
    assert!(
        spawn_heightmaps_seen > 0,
        "spawn chunk (0, 0) must carry client-usage heightmaps"
    );
    assert!(
        chunks_with_heightmaps >= 1,
        "no chunk in the ring carried client-usage heightmaps — \
         encode_chunk_data is probably dropping them"
    );

    let lock_pressure_after = mc_net::lock_pressure_snapshot();
    assert_generated_world_chunk_stream_lock_pressure(lock_pressure_before, lock_pressure_after);

    let resource_metrics = chunk_pipeline_metrics.snapshot();
    assert!(
        resource_metrics.max_cpu_active > 0,
        "M97 generated-world blocker guard must exercise chunk CPU pipeline metrics: {resource_metrics:?}"
    );
    assert!(
        resource_metrics.max_io_active <= policy.chunk_io_threads,
        "M97 generated-world blocker regression: chunk IO concurrency exceeded policy: {resource_metrics:?}"
    );
    assert!(
        resource_metrics.max_cpu_active <= policy.chunk_worker_threads,
        "M97 generated-world blocker regression: chunk CPU concurrency exceeded policy: {resource_metrics:?}"
    );
}

#[tokio::test]
async fn movement_across_chunk_boundary_replans_view_subscription() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!(
            "skipping: {} missing — run tools/extract-vanilla-data.sh --reports",
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * MOVEMENT_VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);
    let policy = mc_net::ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..mc_net::ChunkPipelinePolicy::default()
    };

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M14 movement chunk stream".into(),
        max_players: 8,
        view_distance: MOVEMENT_VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: policy,
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M14MoveTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let center: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    assert_eq!((center.chunk_x, center.chunk_z), (0, 0));
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    let initial_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                initial_deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("initial spawn chunk before movement");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        if (pkt.chunk_x, pkt.chunk_z) == (0, 0) {
            break;
        }
    }

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 48.5,
            y: sync.y,
            z: 0.5,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("send movement");

    let timeout = Duration::from_secs(180);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut saw_new_center = false;
    let mut saw_unload = false;
    let mut saw_new_strip_chunk = false;
    let mut chunks_after_move = 0usize;
    while !(saw_unload && saw_new_strip_chunk) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(frame) => frame,
            Err(err) => panic!(
                "movement replan frames: {err}; center={saw_new_center} unload={saw_unload} new_strip={saw_new_strip_chunk} chunks_after_move={chunks_after_move}"
            ),
        };
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let center = SetCenterChunk::decode(&mut body).expect("decode SetCenterChunk");
            if (center.chunk_x, center.chunk_z) == (3, 0) {
                saw_new_center = true;
            }
            continue;
        }
        if frame.id == ForgetLevelChunk::ID {
            let mut body = frame.body;
            let unload = ForgetLevelChunk::decode(&mut body).expect("decode ForgetLevelChunk");
            if (unload.chunk_x, unload.chunk_z) == (0, 0) {
                saw_unload = true;
            }
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        chunks_after_move += 1;
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        if (pkt.chunk_x, pkt.chunk_z) == (3 + MOVEMENT_VIEW_DISTANCE, 0) {
            saw_new_strip_chunk = true;
        }
    }

    assert!(saw_new_center, "movement must send SetCenterChunk(3, 0)");
    assert!(saw_unload, "movement must unload a chunk that left view");
}

#[tokio::test]
async fn reconnect_during_chunk_prepare_receives_only_the_new_exact_view() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!(
            "skipping: {} missing — run tools/extract-vanilla-data.sh --reports",
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * MOVEMENT_VIEW_DISTANCE + 7) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let policy = mc_net::ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..mc_net::ChunkPipelinePolicy::default()
    };
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "T02 chunk prepare reconnect".into(),
        max_players: 8,
        view_distance: MOVEMENT_VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: policy,
        random_tick: mc_net::RandomTickPolicy {
            spawn_monsters: false,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg)
        .await
        .expect("bind reconnect chunk server");
    let addr = bound.local_addr().expect("reconnect chunk local_addr");
    let runtime = bound.runtime_telemetry_handle();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut first, sync, center) = connect_chunk_stream_client(addr, "T02ChunkRejoin").await;
    assert_eq!((center.chunk_x, center.chunk_z), (0, 0));
    wait_for_chunk(&mut first, (0, 0)).await;
    first
        .write_packet(&ServerboundMovePlayerPos {
            x: 48.5,
            y: sync.y,
            z: 0.5,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("start chunk prepare before disconnect");
    wait_for_center(&mut first, (3, 0)).await;
    drop(first);
    wait_for_active_session_count(&runtime, 0).await;

    let (mut rejoined, rejoin_sync, center) =
        connect_chunk_stream_client(addr, "T02ChunkRejoin").await;
    assert_eq!(
        rejoin_sync.x, 48.5,
        "rejoin must restore the authoritative position that selected the new view"
    );
    assert_eq!((center.chunk_x, center.chunk_z), (3, 0));

    let expected_count = (2 * MOVEMENT_VIEW_DISTANCE + 1).pow(2) as usize;
    let expected_x = 3 - MOVEMENT_VIEW_DISTANCE..=3 + MOVEMENT_VIEW_DISTANCE;
    let expected_z = -MOVEMENT_VIEW_DISTANCE..=MOVEMENT_VIEW_DISTANCE;
    let mut seen = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while seen.len() < expected_count {
        let frame = rejoined
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                let missing = expected_z
                    .clone()
                    .flat_map(|z| expected_x.clone().map(move |x| (x, z)))
                    .filter(|position| !seen.contains(position))
                    .collect::<Vec<_>>();
                panic!(
                    "rejoin exact chunk view stalled after {}/{} chunks; missing={missing:?}: {error}",
                    seen.len(),
                    expected_count
                )
            });
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            rejoined
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo rejoin KeepAlive");
            continue;
        }
        assert_ne!(
            frame.id,
            ForgetLevelChunk::ID,
            "a fresh rejoin must not inherit unloads from the disconnected view"
        );
        assert!(
            frame.id != BlockUpdate::ID && frame.id != SectionBlocksUpdate::ID,
            "a fresh rejoin must not inherit stale block deltas"
        );
        if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let repeated = SetCenterChunk::decode(&mut body).expect("decode repeated center");
            assert_eq!((repeated.chunk_x, repeated.chunk_z), (3, 0));
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let chunk = LevelChunkWithLight::decode(&mut body).expect("decode rejoin chunk");
        assert!(
            expected_x.contains(&chunk.chunk_x) && expected_z.contains(&chunk.chunk_z),
            "rejoin received stale/out-of-view chunk ({}, {})",
            chunk.chunk_x,
            chunk.chunk_z
        );
        assert!(
            seen.insert((chunk.chunk_x, chunk.chunk_z)),
            "rejoin received duplicate chunk ({}, {})",
            chunk.chunk_x,
            chunk.chunk_z
        );
    }

    for z in expected_z {
        for x in expected_x.clone() {
            assert!(
                seen.contains(&(x, z)),
                "rejoin missing required chunk ({x}, {z})"
            );
        }
    }
}

async fn connect_chunk_stream_client(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition, SetCenterChunk) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let center: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync, center)
}

async fn wait_for_chunk(client: &mut Client, expected: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("required chunk");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let chunk = LevelChunkWithLight::decode(&mut body).expect("decode required chunk");
            if (chunk.chunk_x, chunk.chunk_z) == expected {
                return;
            }
        }
    }
}

async fn wait_for_center(client: &mut Client, expected: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("required center");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let center = SetCenterChunk::decode(&mut body).expect("decode required center");
            if (center.chunk_x, center.chunk_z) == expected {
                return;
            }
        }
    }
}

async fn wait_for_active_session_count(runtime: &mc_net::RuntimeTelemetryHandle, expected: usize) {
    let mut sessions = runtime.subscribe_active_sessions();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if *sessions.borrow_and_update() == expected {
                return;
            }
            sessions
                .changed()
                .await
                .expect("active-session publisher remains available");
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "active sessions did not reach {expected}; current={}",
            runtime.snapshot().active_sessions
        )
    });
}

fn assert_generated_world_chunk_stream_lock_pressure(
    before: mc_net::LockMetricsSnapshot,
    after: mc_net::LockMetricsSnapshot,
) {
    eprintln!(
        "generated-world chunk stream lock pressure: world_storage={:?}->{:?} \
         session_registry={:?}->{:?} chunk_prepare={:?}->{:?}",
        before.world_storage,
        after.world_storage,
        before.session_registry,
        after.session_registry,
        before.chunk_prepare,
        after.chunk_prepare
    );
    assert!(
        after.chunk_prepare.wait_count > before.chunk_prepare.wait_count,
        "generated-world full-window stream must exercise ChunkPrepare wait metrics: before={before:?} after={after:?}"
    );
    assert!(
        after.chunk_prepare.hold_count > before.chunk_prepare.hold_count,
        "generated-world full-window stream must exercise ChunkPrepare hold metrics: before={before:?} after={after:?}"
    );
    assert_eq!(
        after.save_all_flush, before.save_all_flush,
        "in-memory generated-world stream must not enter SaveAllFlush lock path"
    );
    assert_eq!(
        after.player_persistence, before.player_persistence,
        "in-memory generated-world stream must not enter PlayerPersistence lock path"
    );
}

/// M4.f: every streamed chunk must carry a wire LightData whose
/// present + empty masks cover all 26 sections, with the present
/// layer count matching the mask popcount. The above-world slot
/// (bit 25) must always be present in the sky mask and carry an
/// all-0xFF nibble layer (open sky).
fn assert_light_invariants(pkt: &LevelChunkWithLight) {
    let light = &pkt.light;
    let sky_mask = mask_to_u64(&light.sky_y_mask);
    let empty_sky_mask = mask_to_u64(&light.empty_sky_y_mask);
    let block_mask = mask_to_u64(&light.block_y_mask);
    let empty_block_mask = mask_to_u64(&light.empty_block_y_mask);

    const ALL_26: u64 = (1 << 26) - 1;
    assert_eq!(
        sky_mask | empty_sky_mask,
        ALL_26,
        "sky present+empty must cover slots 0..=25 for chunk ({}, {}); \
         got present={sky_mask:#X} empty={empty_sky_mask:#X}",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        sky_mask & empty_sky_mask,
        0,
        "sky present/empty masks must be disjoint for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        light.sky_updates.len(),
        sky_mask.count_ones() as usize,
        "sky_updates count must match popcount of sky mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    assert_eq!(
        block_mask | empty_block_mask,
        ALL_26,
        "block present+empty must cover slots 0..=25 for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        block_mask & empty_block_mask,
        0,
        "block present/empty masks must be disjoint for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        light.block_updates.len(),
        block_mask.count_ones() as usize,
        "block_updates count must match popcount of block mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    // Slot 25 (Y=20, the above-world slab) is open sky — always
    // present in the sky channel as all-0xFF.
    assert!(
        sky_mask & (1 << 25) != 0,
        "slot 25 (above-world) must be present in sky mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    // The above-world layer is the *last* one we emit (M4.d emits in
    // ascending slot order, slot 25 last).
    let last_sky = light
        .sky_updates
        .last()
        .expect("sky_updates non-empty when slot 25 is present");
    assert!(
        last_sky.iter().all(|&b| b == 0xFF),
        "above-world sky slab must be all-0xFF nibbles for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    // Every present layer must be exactly 2048 bytes (one section of
    // 4-bit nibbles).
    for (slot_idx, layer) in light.sky_updates.iter().enumerate() {
        assert_eq!(
            layer.len(),
            2048,
            "sky layer {slot_idx} has {} bytes (expected 2048) on chunk ({}, {})",
            layer.len(),
            pkt.chunk_x,
            pkt.chunk_z,
        );
    }
    for (slot_idx, layer) in light.block_updates.iter().enumerate() {
        assert_eq!(
            layer.len(),
            2048,
            "block layer {slot_idx} has {} bytes (expected 2048) on chunk ({}, {})",
            layer.len(),
            pkt.chunk_x,
            pkt.chunk_z,
        );
    }
}

fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
