use super::super::*;
use serde_json::Value;

#[test]
fn pregenerate_refuses_an_unversioned_vanilla_import() {
    let error = ensure_pregenerate_target(WorldSource::ExistingVanilla).unwrap_err();
    assert!(
        error.to_string().contains("read-only"),
        "unexpected error: {error}"
    );
    ensure_pregenerate_target(WorldSource::SolarisGenerated).unwrap();
}

#[test]
fn parses_pregenerate_block_coordinates() {
    assert_eq!(parse_block_coords("12,-34").unwrap(), (12, -34));
    assert_eq!(parse_block_coords(" 0 , 0 ").unwrap(), (0, 0));
    assert!(parse_block_coords("12").is_err());
    assert!(parse_block_coords("12;34").is_err());
    assert!(parse_block_coords("x,34").is_err());
    assert!(parse_block_coords("12,99999999999").is_err());
}

#[test]
fn region_positions_normalise_corners_and_refuse_absurd_requests() {
    let forward = region_positions((-1, -1), (16, 16)).unwrap();
    let reversed = region_positions((16, 16), (-1, -1)).unwrap();
    assert_eq!(forward, reversed);
    assert_eq!(forward.len(), 9);
    for expected in [
        mc_world::ChunkPos { x: -1, z: -1 },
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::ChunkPos { x: 1, z: 1 },
    ] {
        assert!(forward.contains(&expected), "missing {expected:?}");
    }

    let error = region_positions((-1, -1), (i32::MAX, i32::MAX)).unwrap_err();
    assert!(
        error.to_string().contains("pre-generation cap"),
        "unexpected error: {error}"
    );
}

#[test]
fn region_pre_generation_stores_every_requested_chunk_and_skips_stored_ones() {
    use std::sync::atomic::AtomicUsize;

    struct CountingGen {
        calls: AtomicUsize,
    }

    impl mc_world::ChunkGenerator for CountingGen {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let air = mc_world::BlockStateId(0);
            let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = mc_world::Chunk::empty(pos, air, biome);
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            chunk
        }
    }

    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    let tmp = tempfile::tempdir().unwrap();
    ensure_world_region_root(tmp.path()).unwrap();
    let mut storage =
        mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
            .unwrap()
            .with_spawn(mc_world::WorldSpawn::new(0, 0));
    let generator = Arc::new(CountingGen {
        calls: AtomicUsize::new(0),
    });

    let positions = region_positions((-8, 40), (23, 55)).unwrap();
    let expected = positions.len();
    let generated = generate_chunk_positions(
        &mut storage,
        Arc::clone(&generator) as Arc<dyn mc_world::ChunkGenerator>,
        positions.clone(),
        2,
        "region",
    )
    .unwrap();
    assert_eq!(generated, expected);
    assert!(storage.flush_dirty().unwrap() >= expected);
    assert_eq!(generator.calls.load(Ordering::SeqCst), expected);

    // Repeating the same request finds every chunk stored, so nothing is
    // regenerated and played terrain or in-game edits are preserved.
    let (pending, skipped) = pending_region_positions(&mut storage, positions.clone()).unwrap();
    assert!(pending.is_empty(), "stored chunks must not be regenerated");
    assert_eq!(skipped, expected);
    let again = generate_chunk_positions(
        &mut storage,
        Arc::clone(&generator) as Arc<dyn mc_world::ChunkGenerator>,
        pending,
        2,
        "region",
    )
    .unwrap();
    assert_eq!(again, 0);
    assert_eq!(generator.calls.load(Ordering::SeqCst), expected);

    // A position that was never stored is still requested.
    let (mut pending, skipped) =
        pending_region_positions(&mut storage, region_positions((0, 0), (0, 0)).unwrap()).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending.pop(), Some(mc_world::ChunkPos { x: 0, z: 0 }));

    // Every requested chunk survives a reopen, so the region is on disk.
    drop(storage);
    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32).unwrap();
    for pos in &positions {
        assert!(
            reopened.get_chunk(*pos).unwrap().is_some(),
            "requested chunk ({}, {}) is not on disk",
            pos.x,
            pos.z
        );
    }
}

#[test]
fn generate_spawn_window_materializes_view_square_plus_light_border() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct StubGen {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        first_workers_ready: Arc<std::sync::Barrier>,
        calls: AtomicUsize,
    }

    impl mc_world::ChunkGenerator for StubGen {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            if self.calls.fetch_add(1, Ordering::SeqCst) < 4 {
                self.first_workers_ready.wait();
            }
            let air = mc_world::BlockStateId(0);
            let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = mc_world::Chunk::empty(pos, air, biome);
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            self.active.fetch_sub(1, Ordering::SeqCst);
            chunk
        }
    }

    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    let spawn = mc_world::WorldSpawn::new(160, -80);
    let center = spawn.chunk();
    let mut storage =
        mc_world::WorldStorage::in_memory_with_capacity(registry, 32).with_spawn(spawn);
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let generator = Arc::new(StubGen {
        active,
        max_active: Arc::clone(&max_active),
        first_workers_ready: Arc::new(std::sync::Barrier::new(4)),
        calls: AtomicUsize::new(0),
    });

    assert_eq!(
        generate_spawn_window(&mut storage, generator, 1, 4, 4, None).unwrap(),
        25
    );
    assert_eq!(storage.cache_len(), 25);
    assert_eq!(storage.dirty_count(), 25);
    for position in spawn_window_positions_at(center, 1) {
        assert!(
            storage.cached_chunk_snapshot(position).is_some(),
            "spawn-centred chunk {position:?} should be resident"
        );
    }
    assert!(
        storage
            .cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 })
            .is_none(),
        "nonzero spawn generation must not silently materialize origin"
    );
    assert!(
        max_active.load(Ordering::SeqCst) > 1,
        "startup pre-generation should use worker threads"
    );
}

#[test]
#[ignore = "release-host public-alpha 225-chunk Tellus worker-scaling gate"]
fn tellus_seed_712816_spawn_window_reports_worker_scaling() {
    const SEED: i64 = 712_816;
    const VIEW_DISTANCE: i32 = 6;

    if cfg!(debug_assertions) && std::env::var_os("SOLARIS_WORLDGEN_ALLOW_DEBUG_PROBE").is_none() {
        panic!("the public-alpha throughput gate must run with --release");
    }
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    let requested_workers = std::env::var("SOLARIS_WORLDGEN_WORKERS").ok().map(|raw| {
        raw.parse::<usize>()
            .unwrap_or_else(|error| panic!("invalid SOLARIS_WORLDGEN_WORKERS={raw:?}: {error}"))
            .max(1)
    });
    let minimum_chunks_per_second = std::env::var("SOLARIS_WORLDGEN_MIN_CHUNKS_PER_SECOND")
        .ok()
        .map(|raw| {
            raw.parse::<f64>().unwrap_or_else(|error| {
                panic!("invalid SOLARIS_WORLDGEN_MIN_CHUNKS_PER_SECOND={raw:?}: {error}")
            })
        });
    assert!(
        minimum_chunks_per_second.is_none() || requested_workers.is_some(),
        "a throughput minimum requires SOLARIS_WORLDGEN_WORKERS so the compared topology is explicit"
    );
    let mut worker_counts = vec![1, available];
    if let Some(workers) = requested_workers {
        worker_counts.push(workers);
    }
    worker_counts.sort_unstable();
    worker_counts.dedup();

    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("embedded block registry"));
    let generator = build_terrain_generator(
        SEED,
        mc_worldgen::WorldgenMode::TellusLike(mc_worldgen::TellusWorldgenSettings::default()),
        mc_world::OVERWORLD_GEOMETRY,
        Arc::clone(&blocks),
        mc_worldgen::StructureRules::none(),
        None,
        None,
        None,
    )
    .expect("build production Tellus generator");
    let located_spawn = generator
        .locate_safe_spawn()
        .expect("seed 712816 has a bounded natural spawn");
    let spawn = mc_world::WorldSpawn::new(located_spawn.block_x, located_spawn.block_z);
    const SAMPLES_PER_WORKER_COUNT: usize = 3;
    let mut requested_result = None;

    for workers in worker_counts {
        let mut samples = Vec::with_capacity(SAMPLES_PER_WORKER_COUNT);
        for sample in 1..=SAMPLES_PER_WORKER_COUNT {
            let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
                Arc::clone(&blocks),
                chunk_cache_size_for_view_distance(VIEW_DISTANCE),
            )
            .with_spawn(spawn);
            let started = Instant::now();
            let generated = generate_spawn_window(
                &mut storage,
                Arc::clone(&generator) as Arc<dyn mc_world::ChunkGenerator>,
                VIEW_DISTANCE,
                workers,
                startup_light_bake_worker_threads(workers),
                None,
            )
            .expect("generate exact public-alpha spawn window");
            let elapsed = started.elapsed();
            let chunks_per_second = generated as f64 / elapsed.as_secs_f64().max(0.001);

            assert_eq!(generated, 225);
            assert_eq!(storage.cache_len(), 225);
            eprintln!(
                "PUBLIC_ALPHA_WORLDGEN_SAMPLE seed={SEED} chunks={generated} requested_workers={workers} effective_workers={} available_parallelism={available} sample={sample}/{SAMPLES_PER_WORKER_COUNT} elapsed_ms={} chunks_per_second={chunks_per_second:.3}",
                workers.min(generated),
                elapsed.as_millis(),
            );
            samples.push(chunks_per_second);
        }
        samples.sort_by(f64::total_cmp);
        let minimum = samples[0];
        let median = samples[SAMPLES_PER_WORKER_COUNT / 2];
        let maximum = samples[SAMPLES_PER_WORKER_COUNT - 1];
        eprintln!(
            "PUBLIC_ALPHA_WORLDGEN_SUMMARY seed={SEED} chunks=225 requested_workers={workers} effective_workers={} available_parallelism={available} samples={SAMPLES_PER_WORKER_COUNT} min_chunks_per_second={minimum:.3} median_chunks_per_second={median:.3} max_chunks_per_second={maximum:.3}",
            workers.min(225),
        );
        if requested_workers == Some(workers) {
            requested_result = Some(median);
        }
    }

    if let Some(minimum) = minimum_chunks_per_second {
        let workers = requested_workers.expect("minimum requires explicit workers");
        let measured = requested_result.expect("requested worker count was measured");
        assert!(
            measured >= minimum,
            "revision-10 throughput for {workers} workers was {measured:.3} chunks/s, below explicit minimum {minimum:.3} chunks/s"
        );
    }
}

#[test]
fn generate_spawn_window_rejects_worker_panic() {
    struct PanicGen;

    impl mc_world::ChunkGenerator for PanicGen {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            assert_ne!(pos, mc_world::ChunkPos { x: 0, z: 0 }, "worker failure");
            let air = mc_world::BlockStateId(0);
            let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
            mc_world::Chunk::empty(pos, air, biome)
        }
    }

    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 32);

    let error = generate_spawn_window(&mut storage, Arc::new(PanicGen), 1, 4, 4, None)
        .expect_err("partial generation must fail startup");

    assert!(
        error
            .to_string()
            .contains("spawn pre-generation worker panicked"),
        "{error:#}"
    );
    assert!(storage.cache_len() < 25);
}

#[test]
fn generate_spawn_window_bakes_view_square_light() {
    struct StubGen;

    impl mc_world::ChunkGenerator for StubGen {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let air = mc_world::BlockStateId(0);
            let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = mc_world::Chunk::empty(pos, air, biome);
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            chunk
        }
    }

    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 32);
    let table =
        mc_data::block_light::BlockLightTable::from_arrays("test", vec![0], vec![0], vec![true]);

    assert_eq!(
        generate_spawn_window(&mut storage, Arc::new(StubGen), 1, 4, 8, Some(&table)).unwrap(),
        25
    );

    for z in -1..=1 {
        for x in -1..=1 {
            let chunk = storage
                .cached_chunk_snapshot(mc_world::ChunkPos { x, z })
                .expect("view-square chunk should be resident");
            assert!(
                mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some(),
                "view-square chunk ({x}, {z}) should carry baked startup light"
            );
        }
    }
}

#[test]
fn warm_spawn_window_loads_existing_chunks_without_dirtying_them() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                .unwrap();
        for pos in spawn_window_positions(1) {
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(0),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.mark_dirty();
            storage.insert_generated_chunk(pos, chunk).unwrap();
        }
        assert_eq!(storage.flush_dirty().unwrap(), 25);
    }
    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32).unwrap();

    assert_eq!(warm_spawn_window(&mut reopened, 1).unwrap(), 25);

    assert_eq!(reopened.cache_len(), 25);
    assert_eq!(reopened.dirty_count(), 0);
}

#[test]
fn existing_world_startup_bakes_missing_view_square_light() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                .unwrap();
        for pos in spawn_window_positions(1) {
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(0),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.mark_dirty();
            storage.insert_generated_chunk(pos, chunk).unwrap();
        }
        assert_eq!(storage.flush_dirty().unwrap(), 25);
    }
    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32).unwrap();
    let table =
        mc_data::block_light::BlockLightTable::from_arrays("test", vec![0], vec![0], vec![true]);

    assert_eq!(warm_spawn_window(&mut reopened, 1).unwrap(), 25);
    let read_view = reopened.read_view();
    assert_eq!(
        bake_missing_spawn_window_light(&mut reopened, &table, 1, 4).unwrap(),
        9
    );

    for pos in spawn_view_positions(1) {
        let chunk = reopened
            .cached_chunk_snapshot(pos)
            .expect("view-square chunk should remain cached");
        assert!(
            mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some(),
            "view-square chunk ({}, {}) should be backfilled with baked light",
            pos.x,
            pos.z
        );
        let published = read_view
            .snapshot_chunks(&[pos])
            .chunk(pos)
            .expect("view-square chunk should remain published");
        assert!(
            mc_world::light::ChunkLight::from_section_lights(&published.section_lights).is_some(),
            "view-square chunk ({}, {}) should publish baked light",
            pos.x,
            pos.z
        );
    }
    assert_eq!(reopened.dirty_count(), 9);
}

#[test]
fn existing_world_startup_defers_generated_light_border_flush_when_view_light_is_present() {
    struct StubGen;

    impl mc_world::ChunkGenerator for StubGen {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let air = mc_world::BlockStateId(0);
            let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = mc_world::Chunk::empty(pos, air, biome);
            chunk.status = "minecraft:full".into();
            chunk
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let report = [mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
    let baked = mc_world::light::ChunkLight::filled(15, 0);
    {
        let mut storage =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                .unwrap();
        for pos in spawn_view_positions(1) {
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(0),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.set_baked_light(&baked);
            chunk.mark_dirty();
            storage.insert_generated_chunk(pos, chunk).unwrap();
        }
        assert_eq!(storage.flush_dirty().unwrap(), 9);
    }
    let mut reopened = mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32)
        .unwrap()
        .with_generator(Arc::new(StubGen));
    let table =
        mc_data::block_light::BlockLightTable::from_arrays("test", vec![0], vec![0], vec![true]);

    let prep = prepare_existing_spawn_window(&mut reopened, &table, 1, 4).unwrap();

    assert_eq!(prep.warmed, 25);
    assert_eq!(prep.baked, 0);
    assert_eq!(
        prep.dirty, 16,
        "generated light-border chunks should remain dirty and resident before listener"
    );
    assert_eq!(
        reopened.dirty_count(),
        16,
        "existing-world startup should defer dirty warm-cache chunks to startup dirty checkpoint"
    );
}

#[test]
fn check_output_marks_autoscale_live_chunk_send_and_normalized_bounds() {
    let toml_src = r#"
        [server]
        name = "S"
        motd = "M"
        view_distance = 12

        [network]
        bind_address = "0.0.0.0"
        port = 25565

        [chunk_pipeline]
        chunk_send_rate = 3
        chunk_load_rate = 5
        chunk_generate_rate = 7

        [autoscale]
        enabled = true
        min_view_distance = 0
        max_view_distance = 1
        scale_down_after_seconds = 0
        scale_up_after_seconds = 0
    "#;
    let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
    let rendered = serde_json::to_value(EffectiveConfig::from(&cfg)).expect("serialize");
    let autoscale = &rendered["effective_autoscale"];
    let policy = &autoscale["policy"];
    let simulation = &rendered["simulation"];

    assert_eq!(simulation["friendly_spawn_chunk_budget"], 48);
    assert_eq!(simulation["hostile_spawn_chunk_budget"], 4);
    assert_eq!(autoscale["enabled"], Value::Bool(true));
    assert_eq!(autoscale["runtime_mode"], "live_adaptive_work_budgets");
    assert_eq!(policy["min_view_distance"], 2);
    assert_eq!(policy["max_view_distance"], 2);
    assert_eq!(policy["min_chunk_send_rate"], 3);
    assert_eq!(policy["max_chunk_send_rate"], 16);
    assert_eq!(policy["min_chunk_load_rate"], 5);
    assert_eq!(policy["max_chunk_load_rate"], 64);
    assert_eq!(policy["min_chunk_generate_rate"], 7);
    assert_eq!(policy["max_chunk_generate_rate"], 32);
    assert_eq!(policy["scale_down_after_seconds"], 60);
    assert_eq!(policy["scale_up_after_seconds"], 60);
    assert!(policy.get("worker_pressure_percent").is_none());
}
