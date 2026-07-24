use super::super::test_support::{air_stone_registry, single_air_registry};
use super::*;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, Chunk, ChunkPos};
use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_nbt::Tag;

#[test]
fn dirty_flush_uses_unique_region_tmp_without_clobbering_stale_fixed_tmp() {
    use crate::chunk::ChunkGenerator;
    use mc_data::Identifier;

    struct StubGen;

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    let region_dir = tmp_world.path().join("region");
    std::fs::create_dir_all(&region_dir).unwrap();
    let region_path = region_dir.join("r.0.0.mca");
    let tmp_path = region_path.with_extension("mca.tmp");
    let stale_tmp = b"interrupted previous flush";
    std::fs::write(&tmp_path, stale_tmp).unwrap();

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
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen));

    assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
    assert_eq!(world.flush_dirty().unwrap(), 1);

    assert!(region_path.is_file());
    assert_eq!(std::fs::read(&tmp_path).unwrap(), stale_tmp);
    assert_eq!(read_region(&region_path).unwrap().len(), 1);
}

#[test]
fn region_replace_rejects_stale_expected_version() {
    use crate::chunk::ChunkGenerator;
    use mc_data::Identifier;

    struct StubGen;

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    let region_dir = tmp_world.path().join("region");
    std::fs::create_dir_all(&region_dir).unwrap();
    let region_path = region_dir.join("r.0.0.mca");
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
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16)
        .unwrap()
        .with_generator(Arc::new(StubGen));
    assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
    assert_eq!(world.flush_dirty().unwrap(), 1);

    let expected = region_file_version(&region_path).unwrap();
    let payloads = read_region(&region_path).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&region_path)
        .unwrap();
    use std::io::Write as _;
    file.write_all(&[0]).unwrap();

    let tmp_path = write_unique_region_tmp(&region_path, &payloads).unwrap();
    let Err(WorldError::StaleRegion(path)) =
        install_region_file(&region_path, &tmp_path, expected.as_ref())
    else {
        panic!("stale region version must reject replacement");
    };
    assert_eq!(path, region_path);
}

#[test]
fn existing_region_install_rechecks_stale_target_before_rename() {
    use std::io::Write as _;

    let tmp_world = tempfile::tempdir().unwrap();
    let region_path = tmp_world.path().join("r.0.0.mca");
    let tmp_path = tmp_world.path().join(".r.0.0.mca.tmp");
    std::fs::write(&region_path, b"old region").unwrap();
    std::fs::write(&tmp_path, b"planned replacement").unwrap();
    let expected = region_file_version(&region_path).unwrap();

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&region_path)
        .unwrap();
    file.write_all(b" changed").unwrap();

    let Err(WorldError::StaleRegion(path)) =
        install_existing_region_file(&region_path, &tmp_path, expected.as_ref())
    else {
        panic!("existing-region install must reject a stale target before rename");
    };

    assert_eq!(path, region_path);
    assert_eq!(std::fs::read(&region_path).unwrap(), b"old region changed");
    assert!(!tmp_path.exists());
}

#[test]
fn dirty_flush_write_rejects_region_changed_after_planning() {
    use crate::chunk::ChunkGenerator;
    use std::io::Write as _;

    struct StubGen;

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
            chunk.set_block(0, 0, 0, BlockStateId(1));
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    let region_dir = tmp_world.path().join("region");
    std::fs::create_dir_all(&region_dir).unwrap();
    let region_path = region_dir.join("r.0.0.mca");
    let registry = air_stone_registry();

    let mut initial = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen));
    assert!(
        initial
            .get_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .is_some()
    );
    assert_eq!(initial.flush_dirty().unwrap(), 1);

    let mut stale = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
    stale
        .set_block_at(BlockPos { x: 0, y: 0, z: 0 }, BlockStateId(0))
        .unwrap()
        .unwrap();
    let plan = stale.plan_dirty_flush().unwrap();
    assert_eq!(plan.chunk_count(), 1);

    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&region_path)
        .unwrap();
    file.write_all(&[0]).unwrap();

    let Err(WorldError::StaleRegion(path)) = plan.write() else {
        panic!("flush write must reject a region changed after planning");
    };
    assert_eq!(path, region_path);
    assert_eq!(stale.dirty_count(), 1);
}

#[test]
fn sync_dirty_flush_replans_when_competing_writer_creates_region() {
    use crate::chunk::ChunkGenerator;

    struct StubGen;

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
            chunk.set_block(0, 0, 0, BlockStateId(1));
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen));

    assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
    assert_eq!(world.dirty_count(), 1);
    let mut competing_plan = Some(world.plan_dirty_flush().unwrap());

    let flushed = world
        .flush_dirty_at_tick_with_pre_write_hook(0, |_| {
            if let Some(plan) = competing_plan.take() {
                let commit = plan.write().unwrap();
                assert_eq!(commit.regions.len(), 1);
            }
        })
        .unwrap();

    assert_eq!(flushed, 1);
    assert_eq!(world.dirty_count(), 0);

    let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
    assert_eq!(
        reopened.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
        Some(BlockStateId(1))
    );
}

#[test]
fn synchronous_dirty_flush_replans_a_resident_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let position = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        position,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), registry, 4).unwrap();
    world.insert_chunk(position, chunk).unwrap();
    let resident = world.resident.clone();
    let mut mutate_once = true;

    let flushed = world
        .flush_dirty_at_tick_with_pre_write_hook(0, |_| {
            if mutate_once {
                mutate_once = false;
                resident
                    .mutate(position, |chunk| chunk.mark_dirty())
                    .expect("planned resident chunk");
            }
        })
        .unwrap();

    assert_eq!(flushed, 1);
    assert_eq!(world.dirty_count(), 0);
}

#[test]
fn synchronous_dirty_flush_reports_continuous_resident_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let position = ChunkPos { x: 0, z: 0 };
    let mut chunk = Chunk::empty(
        position,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), registry, 4).unwrap();
    world.insert_chunk(position, chunk).unwrap();
    let resident = world.resident.clone();
    let mut attempts = 0usize;

    let error = world
        .flush_dirty_at_tick_with_pre_write_hook(0, |_| {
            attempts += 1;
            resident
                .mutate(position, |chunk| chunk.mark_dirty())
                .expect("planned resident chunk");
        })
        .unwrap_err();

    assert_eq!(attempts, 4);
    assert!(matches!(
        error,
        WorldError::ResidentChangedDuringFlush {
            attempts: 4,
            remaining_dirty: 1,
        }
    ));
    assert_eq!(world.dirty_count(), 1);
    assert!(!tmp.path().join("region/r.0.0.mca").exists());
}

#[test]
fn new_region_install_rejects_concurrent_create() {
    let tmp_world = tempfile::tempdir().unwrap();
    let region_path = tmp_world.path().join("r.0.0.mca");
    let tmp_path = tmp_world.path().join(".r.0.0.mca.tmp");
    std::fs::write(&tmp_path, b"planned replacement").unwrap();
    std::fs::write(&region_path, b"concurrent region").unwrap();

    let Err(WorldError::StaleRegion(path)) = install_new_region_file(&region_path, &tmp_path)
    else {
        panic!("new-region install must reject a concurrently created target");
    };

    assert_eq!(path, region_path);
    assert_eq!(std::fs::read(&region_path).unwrap(), b"concurrent region");
    assert!(!tmp_path.exists());
}

#[test]
fn dirty_flush_commit_skips_a_region_changed_after_planning() {
    use crate::chunk::ChunkGenerator;
    use mc_data::Identifier;

    struct StubGen {
        stone: BlockStateId,
    }

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let air = BlockStateId(0);
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, air, biome);
            chunk.set_block(3, 0, 5, self.stone);
            chunk.status = "minecraft:full".into();
            chunk.mark_dirty();
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let report = vec![
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 2,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen {
            stone: BlockStateId(1),
        }));

    let edit_pos = BlockPos { x: 3, y: 0, z: 5 };
    world.get_block(edit_pos).unwrap().unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);
    world
        .set_block_at(edit_pos, BlockStateId(2))
        .unwrap()
        .unwrap();
    let plan = world.plan_dirty_flush().unwrap();
    assert_eq!(plan.chunk_count(), 1);

    world
        .set_block_at(edit_pos, BlockStateId(1))
        .unwrap()
        .unwrap();
    let commit = plan.write().unwrap();
    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
    assert_eq!(world.dirty_count(), 1);
    assert!(
        std::fs::read_dir(tmp_world.path().join("region"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")),
        "a skipped region must release its temporary image"
    );

    let mut fresh =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
    assert_eq!(fresh.get_block(edit_pos).unwrap(), Some(BlockStateId(1)));

    assert_eq!(world.flush_dirty().unwrap(), 1);
    assert_eq!(world.dirty_count(), 0);
    let mut fresh = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
    assert_eq!(fresh.get_block(edit_pos).unwrap(), Some(BlockStateId(1)));
}

#[test]
fn multi_region_commit_installs_stable_region_and_skips_changed_region() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 2).unwrap();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let changed_region_first = ChunkPos { x: 0, z: 0 };
    let changed_region_second = ChunkPos { x: 1, z: 0 };
    let stable_region = ChunkPos { x: 32, z: 0 };
    for position in [changed_region_first, changed_region_second, stable_region] {
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }

    let commit = world.plan_dirty_flush().unwrap().write().unwrap();
    world
        .set_block_at(
            BlockPos {
                x: changed_region_second.x * 16,
                y: 0,
                z: 0,
            },
            BlockStateId(1),
        )
        .unwrap();

    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
    assert_eq!(world.dirty_count(), 2);
    assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 2);
    assert!(!tmp.path().join("region/r.0.0.mca").exists());
    assert!(tmp.path().join("region/r.1.0.mca").is_file());

    let mut reopened = WorldStorage::open_with_capacity(tmp.path(), registry, 2).unwrap();
    assert!(reopened.get_chunk(stable_region).unwrap().is_some());
    assert!(reopened.get_chunk(changed_region_first).unwrap().is_none());
}

#[test]
fn dirty_flush_does_not_overwrite_newer_region_with_stale_cached_snapshot() {
    use crate::chunk::ChunkGenerator;
    use mc_data::Identifier;

    struct StubGen {
        state: BlockStateId,
    }

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let air = BlockStateId(0);
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, air, biome);
            chunk.set_block(0, 0, 0, self.state);
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let report = vec![
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 2,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        },
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());

    let mut initial = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen {
            state: BlockStateId(1),
        }));
    assert!(
        initial
            .get_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .is_some()
    );
    assert_eq!(initial.flush_dirty().unwrap(), 1);

    let mut stale_cached =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
    assert_eq!(
        stale_cached
            .get_block(BlockPos { x: 0, y: 0, z: 0 })
            .unwrap(),
        Some(BlockStateId(1))
    );

    let mut newer = WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
        .unwrap()
        .with_generator(Arc::new(StubGen {
            state: BlockStateId(1),
        }));
    assert!(newer.get_chunk(ChunkPos { x: 1, z: 0 }).unwrap().is_some());
    assert_eq!(newer.flush_dirty().unwrap(), 1);

    stale_cached
        .set_block_at(BlockPos { x: 0, y: 0, z: 0 }, BlockStateId(2))
        .unwrap()
        .unwrap();
    assert_eq!(stale_cached.flush_dirty().unwrap(), 1);

    let mut fresh =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
    assert_eq!(
        fresh.get_block(BlockPos { x: 16, y: 0, z: 0 }).unwrap(),
        Some(BlockStateId(1))
    );
    assert_eq!(
        fresh.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
        Some(BlockStateId(2))
    );
}

#[test]
fn dirty_flush_plan_retains_the_live_snapshot_without_payload_encoding() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned = &plan.regions[0].dirty_payloads[0];
    let snapshot = world.resident.snapshot(cpos).unwrap();

    assert!(Arc::ptr_eq(&planned.snapshot, &snapshot));
}

#[test]
fn bounded_dirty_flush_plan_commits_one_batch_and_leaves_remainder() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let mut world = WorldStorage::open_with_capacity(temp.path(), registry, 3).unwrap();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    for x in 0..3 {
        let position = ChunkPos { x, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }

    let plan = world.plan_dirty_flush_at_tick_bounded(17, 2).unwrap();
    assert_eq!(plan.chunk_count(), 2);
    assert_eq!(world.commit_dirty_flush(plan.write().unwrap()).unwrap(), 2);
    assert_eq!(world.dirty_count(), 1);

    let remainder = world.plan_dirty_flush_at_tick_bounded(18, 2).unwrap();
    assert_eq!(remainder.chunk_count(), 1);
    assert_eq!(
        world
            .commit_dirty_flush(remainder.write().unwrap())
            .unwrap(),
        1
    );
    assert_eq!(world.dirty_count(), 0);
}

#[test]
fn dirty_flush_plan_clones_snapshots_without_encoding_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let payload_encode_count = plan.payload_encode_counter();

    assert_eq!(
        payload_encode_count.load(Ordering::Relaxed),
        0,
        "dirty flush planning should only clone snapshots while the world lock is held"
    );

    let commit = plan.write().unwrap();

    assert_eq!(payload_encode_count.load(Ordering::Relaxed), 1);
    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
    assert_eq!(world.dirty_count(), 0);
}

#[test]
fn dirty_flush_write_carries_retained_snapshot_fast_path_metadata_into_commit() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned = &plan.regions[0].dirty_payloads[0];
    let expected_snapshot = Arc::clone(&planned.snapshot);
    let commit = plan.write().unwrap();
    let committed = &commit.regions[0].chunks[0];

    assert!(Arc::ptr_eq(&committed.snapshot, &expected_snapshot));
}

#[test]
fn dirty_flush_mutable_fork_after_plan_bumps_generation() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned = &plan.regions[0].dirty_payloads[0];
    let planned_generation = planned.dirty_generation;
    let planned_snapshot = Arc::clone(&planned.snapshot);

    world
        .get_chunk_mut(cpos)
        .unwrap()
        .unwrap()
        .set_block(1, 0, 1, BlockStateId(1));

    let live_snapshot = world.resident.snapshot(cpos).unwrap();
    assert!(live_snapshot.dirty_generation > planned_generation);
    assert!(!Arc::ptr_eq(&live_snapshot, &planned_snapshot));
    assert_eq!(
        planned_snapshot.get_block(1, 0, 1).unwrap(),
        BlockStateId(0)
    );
    assert_eq!(live_snapshot.get_block(1, 0, 1).unwrap(), BlockStateId(1));
}

#[test]
fn dirty_flush_mutable_alias_after_plan_skips_install_and_stays_dirty() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned_generation = plan.regions[0].dirty_payloads[0].dirty_generation;

    let _chunk = world.get_chunk_mut(cpos).unwrap().unwrap();
    assert!(
        world.resident.snapshot(cpos).unwrap().dirty_generation > planned_generation,
        "mutable access after dirty flush planning must invalidate the planned generation"
    );

    let commit = plan.write().unwrap();

    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
    assert_eq!(world.dirty_count(), 1);
}

#[test]
fn dirty_flush_commit_cleans_unchanged_nonzero_generation_snapshot_fast_path() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned = &plan.regions[0].dirty_payloads[0];
    let planned_generation = planned.dirty_generation;
    let planned_snapshot = Arc::clone(&planned.snapshot);
    let commit = plan.write().unwrap();

    assert_ne!(planned_generation, 0);
    assert!(Arc::ptr_eq(
        &world.resident.snapshot(cpos).unwrap(),
        &planned_snapshot,
    ));
    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
    assert_eq!(world.dirty_count(), 0);
}

#[test]
fn dirty_flush_commit_fast_path_clears_without_copying_unchanged_chunk() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let before_token = chunk_snapshot_token(&world.resident.snapshot(cpos).unwrap());
    let commit = plan.write().unwrap();

    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);

    let live = world.resident.snapshot(cpos).unwrap();
    assert!(!live.dirty);
    assert_eq!(chunk_snapshot_token(&live), before_token);
}

#[test]
fn dirty_flush_commit_skips_a_defensive_snapshot_change() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let table = BlockLightTable::from_arrays("test", vec![0, 0], vec![0, 15], vec![true, false]);
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.set_block(1, 0, 1, BlockStateId(1));
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned = &plan.regions[0].dirty_payloads[0];
    let planned_generation = planned.dirty_generation;
    let planned_snapshot = Arc::clone(&planned.snapshot);

    let mut fork = (*world.resident.snapshot(cpos).unwrap()).clone();
    fork.update_highest_opaque_column(1, 1, &table);
    world.resident.replace_for_test(cpos, Arc::new(fork));

    let live_snapshot = world.resident.snapshot(cpos).unwrap();
    let commit = plan.write().unwrap();

    assert_eq!(live_snapshot.dirty_generation, planned_generation);
    assert!(!Arc::ptr_eq(&live_snapshot, &planned_snapshot));
    assert_eq!(planned_snapshot.highest_opaque_y(1, 1), None);
    assert_eq!(live_snapshot.highest_opaque_y(1, 1), Some(0));
    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
    assert_eq!(world.dirty_count(), 1);
}

#[test]
fn dirty_flush_commit_skips_post_plan_unmarked_chunk_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    let planned_generation = world.resident.snapshot(cpos).unwrap().dirty_generation;
    world
        .get_chunk_mut(cpos)
        .unwrap()
        .unwrap()
        .set_block(1, 0, 1, BlockStateId(1));
    assert!(world.resident.snapshot(cpos).unwrap().dirty_generation > planned_generation);

    let commit = plan.write().unwrap();

    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
    assert_eq!(world.dirty_count(), 1);
    assert_eq!(
        world.get_block(BlockPos { x: 1, y: 0, z: 1 }).unwrap(),
        Some(BlockStateId(1))
    );
}

#[test]
fn dirty_flush_commit_skips_nonzero_generation_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.mark_dirty();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();

    let plan = world.plan_dirty_flush().unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().mark_dirty();
    let commit = plan.write().unwrap();

    assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
    assert_eq!(world.dirty_count(), 1);
}

#[test]
fn dirty_flush_preserves_unknown_root_extras_after_edit_flush_reopen() {
    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let extras = vec![
        ("DataVersion".into(), Tag::Int(4444)),
        ("InhabitedTime".into(), Tag::Long(123_456)),
        ("structures".into(), Tag::Compound(Vec::new())),
    ];

    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.extras = extras.clone();
    chunk.mark_dirty();

    let mut world =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);
    drop(world);

    let mut edited =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    edited
        .set_block_at(BlockPos { x: 1, y: 0, z: 1 }, BlockStateId(1))
        .unwrap();
    assert_eq!(edited.flush_dirty().unwrap(), 1);
    drop(edited);

    let mut reopened =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    let chunk = reopened.get_chunk(cpos).unwrap().unwrap();
    assert_eq!(
        chunk.extras,
        vec![
            ("DataVersion".into(), Tag::Int(4444)),
            ("LastUpdate".into(), Tag::Long(0)),
            ("InhabitedTime".into(), Tag::Long(123_456)),
            ("structures".into(), Tag::Compound(Vec::new())),
        ]
    );
    assert_eq!(chunk.get_block(1, 0, 1).unwrap(), BlockStateId(1));
}
