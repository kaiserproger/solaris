use super::super::test_support::*;
use super::super::*;

#[test]
fn region_of_handles_negative_coordinates() {
    assert_eq!(region_of(ChunkPos { x: 0, z: 0 }), (0, 0));
    assert_eq!(region_of(ChunkPos { x: 31, z: 31 }), (0, 0));
    assert_eq!(region_of(ChunkPos { x: 32, z: 0 }), (1, 0));
    assert_eq!(region_of(ChunkPos { x: -1, z: -1 }), (-1, -1));
    assert_eq!(region_of(ChunkPos { x: -32, z: 0 }), (-1, 0));
    assert_eq!(region_of(ChunkPos { x: -33, z: 0 }), (-2, 0));
}

#[test]
fn chunk_pos_of_handles_negative_coordinates() {
    assert_eq!(
        chunk_pos_of(BlockPos { x: 0, y: 0, z: 0 }),
        ChunkPos { x: 0, z: 0 }
    );
    assert_eq!(
        chunk_pos_of(BlockPos { x: 15, y: 0, z: 15 }),
        ChunkPos { x: 0, z: 0 }
    );
    assert_eq!(
        chunk_pos_of(BlockPos { x: 16, y: 0, z: 0 }),
        ChunkPos { x: 1, z: 0 }
    );
    assert_eq!(
        chunk_pos_of(BlockPos { x: -1, y: 0, z: -1 }),
        ChunkPos { x: -1, z: -1 }
    );
    assert_eq!(
        chunk_pos_of(BlockPos { x: -16, y: 0, z: 0 }),
        ChunkPos { x: -1, z: 0 }
    );
}

#[test]
fn disk_load_rejects_same_and_cross_region_position_mismatch_without_caching() {
    let registry = single_air_registry();
    for (slot_pos, embedded_pos) in [
        (ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }),
        (ChunkPos { x: 31, z: -1 }, ChunkPos { x: 32, z: -1 }),
    ] {
        let tmp_world = tempfile::tempdir().unwrap();
        write_chunk_payload_at_slot(tmp_world.path(), slot_pos, embedded_pos, &registry, &[]);
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();

        assert!(matches!(
            world.get_chunk_without_generation(slot_pos),
            Err(WorldError::ChunkNbt(ChunkNbtError::PositionMismatch {
                expected_x,
                expected_z,
                actual_x,
                actual_z,
            })) if expected_x == slot_pos.x
                && expected_z == slot_pos.z
                && actual_x == embedded_pos.x
                && actual_z == embedded_pos.z
        ));
        assert!(world.cached_chunk_snapshot(slot_pos).is_none());
        assert_eq!(world.cache_len(), 0);
    }
}

#[test]
fn disk_load_rejects_trailing_chunk_nbt_without_caching() {
    let registry = single_air_registry();
    let pos = ChunkPos { x: -2, z: 3 };
    let tmp_world = tempfile::tempdir().unwrap();
    write_chunk_payload_at_slot(tmp_world.path(), pos, pos, &registry, &[0x7F]);
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();

    assert!(matches!(
        world.get_chunk_without_generation(pos),
        Err(WorldError::ChunkNbt(ChunkNbtError::TrailingNbtBytes {
            trailing: 1
        }))
    ));
    assert!(world.cached_chunk_snapshot(pos).is_none());
}

#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 blocks report"]
fn opens_real_test_world_and_queries_blocks() {
    let world_dir = workspace_path(".analysis/test-world");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        world_dir.is_dir() && blocks_path.is_file(),
        "need {} and {}",
        world_dir.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let mut world = WorldStorage::open_with_capacity(&world_dir, Arc::clone(&registry), 4).unwrap();

    let resolve = |w: &WorldStorage, id: BlockStateId| {
        w.registry()
            .by_id(id)
            .unwrap()
            .block
            .id
            .as_str()
            .to_string()
    };

    let air_id = air_state_id(&registry);
    let top_y = top_non_air_y(&mut world, 0, 0, air_id).expect("origin column has terrain");
    let top = world
        .get_block(BlockPos {
            x: 0,
            y: top_y,
            z: 0,
        })
        .unwrap()
        .unwrap();
    let air_above = world
        .get_block(BlockPos {
            x: 0,
            y: top_y + 1,
            z: 0,
        })
        .unwrap()
        .unwrap();
    assert_ne!(top, air_id, "top terrain block must not be air");
    assert_eq!(resolve(&world, air_above), "minecraft:air");

    // Out-of-range Y returns None gracefully.
    assert_eq!(
        world
            .get_block(BlockPos {
                x: 0,
                y: 1000,
                z: 0
            })
            .unwrap(),
        None
    );
    // A chunk in a region that doesn't exist on disk returns
    // None, not an error.
    assert_eq!(
        world
            .get_block(BlockPos {
                x: 100_000,
                y: 0,
                z: 0,
            })
            .unwrap(),
        None
    );

    // LRU stays bounded across many lookups.
    for x in 0..50 {
        let _ = world.get_block(BlockPos { x, y: -64, z: 0 }).unwrap();
    }
    assert!(world.cache_len() <= 4);
}

/// Walking 121 chunks of one region must leave exactly one entry
/// in the region cache regardless of chunk-LRU thrash. This is
/// the structural M3.f assertion: without the region cache the
/// equivalent path re-opened `r.0.0.mca` 121 times.

#[test]
fn worldgen_fallback_fills_missing_chunks() {
    use crate::chunk::ChunkGenerator;

    let tmp = tempfile::tempdir().unwrap();
    // Create the expected directory layout without populating it.
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();

    // Stub generator: every chunk is a single grass block at the
    // origin column. Enough to assert "we hit the generator".
    struct StubGen;
    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let mut c = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            c.set_block(0, 0, 0, BlockStateId(42));
            c.dirty = true;
            c
        }
    }

    // The stub registry has only "air" but generator emits
    // BlockStateId(42) directly; the registry isn't consulted on
    // the read path for raw state ids.
    let report = vec![mc_data::blocks::BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    }];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
        .unwrap()
        .with_generator(Arc::new(StubGen));

    let cpos = ChunkPos { x: 999, z: -999 };
    let chunk = world.get_chunk(cpos).unwrap().expect("generator ran");
    assert_eq!(chunk.get_block(0, 0, 0), Some(BlockStateId(42)));
    assert!(chunk.dirty);
}

#[test]
fn chunk_lookup_without_generation_does_not_run_generator() {
    use crate::chunk::ChunkGenerator;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingGen {
        calls: Arc<AtomicUsize>,
    }

    impl ChunkGenerator for CountingGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            )
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = Arc::new(
        BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }])
        .unwrap(),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let mut world = WorldStorage::open_with_capacity(tmp.path(), registry, 4)
        .unwrap()
        .with_generator(Arc::new(CountingGen {
            calls: Arc::clone(&calls),
        }));

    assert!(
        world
            .get_chunk_without_generation(ChunkPos { x: 4, z: 4 })
            .unwrap()
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    assert!(world.get_chunk(ChunkPos { x: 4, z: 4 }).unwrap().is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// Bench-style coverage of the M3.e load pattern: stream the
/// bottom-right quadrant of the view-distance ring (chunks 0..=10
/// in both axes — the slice of vd=10 around spawn that exists in
/// the test world's only region file). The point is to measure
/// the time-to-stream so the M3.f region-cache lands with a
/// before/after number rather than a guess.

#[test]
#[ignore = "explicit local vd=10 chunk-stream performance probe"]
fn streams_view_distance_quadrant_within_budget() {
    let world_dir = workspace_path(".analysis/test-world");
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    assert!(
        world_dir.is_dir() && blocks_path.is_file(),
        "need {} and {}",
        world_dir.display(),
        blocks_path.display()
    );
    let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    // Match the production chunk-LRU default. Revisiting 121 chunks
    // thrashes its 16 slots; the region cache retains only the index,
    // so each missed chunk is read and decompressed again.
    let mut world = WorldStorage::open(&world_dir, registry).unwrap();

    for cz in 0..=10 {
        for cx in 0..=10 {
            assert!(
                world
                    .get_chunk_without_generation(ChunkPos { x: cx, z: cz })
                    .unwrap()
                    .is_some(),
                "{} does not contain required vd=10 chunk ({cx}, {cz})",
                world_dir.display()
            );
        }
    }

    let started = std::time::Instant::now();
    let mut hit = 0usize;
    for cz in 0..=10 {
        for cx in 0..=10 {
            let chunk = world.get_chunk(ChunkPos { x: cx, z: cz }).unwrap();
            assert!(chunk.is_some(), "chunk ({cx}, {cz}) missing from r.0.0.mca");
            hit += 1;
        }
    }
    let elapsed = started.elapsed();
    eprintln!(
        "vd-quadrant stream: {hit} chunks in {ms} ms ({per_chunk_us} us/chunk)",
        ms = elapsed.as_millis(),
        per_chunk_us = elapsed.as_micros() as f64 / hit as f64,
    );
    // Generous ceiling for a contended runner; report the elapsed time
    // above so local probes can compare indexed per-slot loading.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "vd-quadrant stream took {elapsed:?} — suspicious regression",
    );
}
