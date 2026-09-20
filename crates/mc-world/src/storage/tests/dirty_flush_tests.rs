use super::super::test_support::*;
use super::super::*;
use mc_nbt::Tag;

#[test]
fn cache_insert_and_commit_reject_foreign_chunk_positions() {
    let registry = single_air_registry();
    let expected = ChunkPos { x: 0, z: 0 };
    let actual = ChunkPos { x: 1, z: 0 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let foreign = Chunk::empty(actual, BlockStateId(0), biome.clone());
    let mut empty_world = WorldStorage::in_memory(Arc::clone(&registry));
    assert!(matches!(
        empty_world.insert_generated_chunk(expected, foreign.clone()),
        Err(WorldError::ChunkPositionMismatch {
            expected_x: 0,
            expected_z: 0,
            actual_x: 1,
            actual_z: 0,
        })
    ));
    assert_eq!(empty_world.cache_len(), 0);

    let mut resident_world = WorldStorage::in_memory(registry);
    resident_world
        .insert_generated_chunk(expected, Chunk::empty(expected, BlockStateId(0), biome))
        .unwrap();
    assert!(matches!(
        resident_world.commit_chunk_snapshot(expected, foreign),
        Err(WorldError::ChunkPositionMismatch {
            expected_x: 0,
            expected_z: 0,
            actual_x: 1,
            actual_z: 0,
        })
    ));
    assert_eq!(
        resident_world.cached_chunk_snapshot(expected).unwrap().pos,
        expected
    );
}

#[test]
fn storage_stats_report_cache_and_dirty_pressure() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 2);
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();

    let stats = world.stats();

    assert_eq!(stats.chunk_cache_len, 1);
    assert_eq!(stats.chunk_cache_capacity, 2);
    assert_eq!(stats.region_cache_len, 0);
    assert_eq!(stats.region_cache_capacity, 4);
    assert_eq!(stats.dirty_chunks, 1);
    assert!(!stats.dirty_chunk_cache_saturated);
}

#[test]
fn saturated_dirty_state_changes_notify_without_a_new_high_water_edge() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
    let position = ChunkPos { x: 0, z: 0 };
    let block = BlockPos { x: 1, y: 64, z: 1 };
    let notifications = Arc::new(AtomicUsize::new(0));
    world.set_dirty_high_water_notifier({
        let notifications = Arc::clone(&notifications);
        Arc::new(move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        })
    });
    world
        .insert_generated_chunk(
            position,
            Chunk::empty(
                position,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(notifications.load(Ordering::SeqCst), 1);

    assert_eq!(
        world
            .mutation_view()
            .schedule_fluid_ticks(&[ScheduledFluidTick::new(
                block,
                Identifier::parse("minecraft:water").unwrap(),
                20,
                0,
            )]),
        1
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 2);

    assert_eq!(
        world
            .mutation_view()
            .schedule_fluid_ticks(&[ScheduledFluidTick::new(
                block,
                Identifier::parse("minecraft:water").unwrap(),
                21,
                0,
            ),]),
        1
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 3);
}

#[test]
fn unchanged_resident_mutation_does_not_notify_flush_consumer() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(registry);
    let position = ChunkPos { x: 0, z: 0 };
    let block = BlockPos { x: 1, y: 64, z: 1 };
    let tick = ScheduledFluidTick::new(block, Identifier::parse("minecraft:water").unwrap(), 20, 0);
    world
        .insert_generated_chunk(
            position,
            Chunk::empty(
                position,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(
        world
            .mutation_view()
            .schedule_fluid_ticks(std::slice::from_ref(&tick)),
        1
    );
    let notifications = Arc::new(AtomicUsize::new(0));
    world.set_dirty_high_water_notifier({
        let notifications = Arc::clone(&notifications);
        Arc::new(move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        })
    });

    assert_eq!(world.mutation_view().schedule_fluid_ticks(&[tick]), 0);
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
}

#[test]
fn retained_read_chunk_is_not_evicted_until_last_release() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
    let position = ChunkPos { x: 0, z: 0 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(position, Chunk::empty(position, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(position).unwrap().unwrap().dirty = false;
    let read_view = world.read_view();
    read_view.retain_chunk(position);
    read_view.retain_chunk(position);
    assert_eq!(read_view.retained_chunk_count(position), 2);

    assert!(!world.evict_clean_chunk());
    assert!(world.cached_chunk_snapshot(position).is_some());
    assert!(read_view.release_chunk(position));
    assert_eq!(read_view.retained_chunk_count(position), 1);
    assert!(!world.evict_clean_chunk());

    assert!(read_view.release_chunk(position));
    assert_eq!(read_view.retained_chunk_count(position), 0);
    assert!(world.evict_clean_chunk());
    assert!(world.cached_chunk_snapshot(position).is_none());
    assert!(!read_view.release_chunk(position));
}

#[test]
fn dirty_pressure_try_insert_defers_without_growth() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();

    world
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome.clone()),
        )
        .unwrap();
    assert!(world.stats().dirty_chunk_cache_saturated);

    let inserted = world
        .try_insert_generated_chunk(
            ChunkPos { x: 1, z: 0 },
            Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();

    assert!(!inserted);
    assert_eq!(world.cache_len(), 1);
    assert!(world.cached_chunk(ChunkPos { x: 1, z: 0 }).is_none());
}

#[test]
fn dirty_pressure_try_commit_defers_loaded_chunk_without_growth() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();

    world
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome.clone()),
        )
        .unwrap();

    let committed = world
        .try_commit_chunk_snapshot(
            ChunkPos { x: 1, z: 0 },
            Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();

    assert!(committed.is_none());
    assert_eq!(world.cache_len(), 1);
    assert!(world.cached_chunk(ChunkPos { x: 1, z: 0 }).is_none());
}

#[test]
fn cached_due_tick_drain_does_not_invalidate_dirty_shared_chunk_without_due_ticks() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
    let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    assert!(
        world
            .schedule_block_tick(ScheduledBlockTick::new(pos, block, 20, 0))
            .unwrap()
    );
    assert!(
        world
            .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 20, 0))
            .unwrap()
    );
    let _shared = world.cached_chunk_snapshot(cpos).unwrap();
    let before = world.resident.snapshot(cpos).unwrap().dirty_generation;

    assert!(
        world
            .drain_due_cached_block_ticks(cpos, 19, usize::MAX)
            .is_empty()
    );
    assert!(
        world
            .drain_due_cached_fluid_ticks(cpos, 19, usize::MAX)
            .is_empty()
    );

    assert_eq!(
        world.resident.snapshot(cpos).unwrap().dirty_generation,
        before
    );
}

#[test]
fn furnace_block_entity_survives_flush_and_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let air = mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    };
    let registry = Arc::new(BlockRegistry::from_report(&[air]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:raw_iron").unwrap(),
            protocol_id: 10,
        },
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:coal").unwrap(),
            protocol_id: 11,
        },
    ]));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    let mut furnace = FurnaceBlockEntity {
        burn_remaining: 1200,
        burn_total: 1600,
        cook_progress: 37,
        cook_total: 200,
        ..FurnaceBlockEntity::default()
    };
    furnace.slots[0] = crate::chunk::FurnaceSlot {
        count: 1,
        item_id: 10,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    furnace.slots[1] = crate::chunk::FurnaceSlot {
        count: 3,
        item_id: 11,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    world
        .set_furnace_block_entity(pos, furnace.clone())
        .unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);

    let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
        .unwrap()
        .with_item_registry(items);
    assert_eq!(fresh.furnace_block_entity(pos).unwrap(), Some(furnace));
}

#[test]
fn chest_block_entity_survives_flush_and_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let air = mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    };
    let registry = Arc::new(BlockRegistry::from_report(&[air]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 10,
        },
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 11,
        },
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:suspicious_stew").unwrap(),
            protocol_id: 12,
        },
    ]));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = crate::chunk::FurnaceSlot {
        count: 64,
        item_id: 10,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    chest.slots[26] = crate::chunk::FurnaceSlot {
        count: 3,
        item_id: 11,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    chest.slots[0].custom_name = Some("Reserved supplies".to_owned());
    chest.slots[0].item_model = Some(Arc::new(
        mc_data::Identifier::parse("solaris:stored_supplies").unwrap(),
    ));
    chest.slots[1] = crate::chunk::FurnaceSlot {
        item_id: 12,
        count: 1,
        stew_effects: vec![mc_data::item_stack::StewEffect {
            id: mc_data::Identifier::parse("minecraft:poison").unwrap(),
            duration: 220,
        }],
        ..crate::chunk::FurnaceSlot::EMPTY
    };
    world.set_chest_block_entity(pos, chest.clone()).unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);

    let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
        .unwrap()
        .with_item_registry(items);
    assert_eq!(fresh.chest_block_entity(pos).unwrap(), Some(chest));
}

#[test]
fn hopper_block_entity_survives_flush_and_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let registry = air_stone_hopper_registry();
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 10,
        },
        mc_data::items::ItemReport {
            id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 11,
        },
    ]));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    let mut hopper = crate::chunk::HopperBlockEntity {
        transfer_cooldown: 6,
        ..Default::default()
    };
    hopper.slots[0] = crate::chunk::FurnaceSlot {
        count: 64,
        item_id: 10,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    hopper.slots[4] = crate::chunk::FurnaceSlot {
        count: 3,
        item_id: 11,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    world.set_hopper_block_entity(pos, hopper.clone()).unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);

    let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
        .unwrap()
        .with_item_registry(items);
    assert_eq!(fresh.hopper_block_entity(pos).unwrap(), Some(hopper));
}

#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 blocks report"]
fn region_cache_holds_one_region_across_quadrant_walk() {
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
    let mut world = WorldStorage::open_with_capacity(&world_dir, registry, 4).unwrap();

    for cz in 0..=10 {
        for cx in 0..=10 {
            assert!(
                world
                    .get_chunk(ChunkPos { x: cx, z: cz })
                    .unwrap()
                    .is_some(),
                "test world must contain required chunk ({cx}, {cz})"
            );
        }
    }
    // Chunk LRU still capped at 4. Region LRU now holds exactly
    // the one region those chunks live in.
    assert!(world.cache_len() <= 4);
    assert_eq!(world.region_cache_len(), 1);
}

/// M6.b: a dirty chunk in the cache is flushed to its `.mca`
/// when `flush_dirty` is called, and the flush survives a fresh
/// `WorldStorage::open` (i.e. the next read picks it up from
/// disk, not from the in-memory cache).

#[test]
fn flush_dirty_writes_modified_chunks_to_disk() {
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

    let stone_id = registry
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .unwrap();
    let dirt_id = registry
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .unwrap();
    let edit_pos = BlockPos { x: 3, y: 0, z: 5 };
    let current = world.get_block(edit_pos).unwrap().unwrap();
    let new_state = if current == stone_id {
        dirt_id
    } else {
        stone_id
    };
    let prev = world.set_block_at(edit_pos, new_state).unwrap().unwrap();
    assert_ne!(prev, new_state, "test world cell must change state");
    assert_eq!(world.dirty_count(), 1);

    let n_flushed = world.flush_dirty().unwrap();
    assert_eq!(n_flushed, 1);
    assert_eq!(world.dirty_count(), 0);

    // Drop the in-memory world and re-open fresh — proves the
    // edit landed on disk, not just in the LRU.
    drop(world);
    let mut world2 =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
    let after = world2.get_block(edit_pos).unwrap().unwrap();
    assert_eq!(after, new_state);
}

#[test]
fn section_light_arrays_survive_flush_and_reopen() {
    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let block_light = (0..crate::chunk::LIGHT_LAYER_BYTES)
        .map(|index| (index & 0xFF) as u8)
        .collect::<Vec<_>>();
    let sky_light = (0..crate::chunk::LIGHT_LAYER_BYTES)
        .map(|index| 255u8.wrapping_sub((index & 0xFF) as u8))
        .collect::<Vec<_>>();

    let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
    chunk.section_lights[0].block = Some(crate::chunk::LightSection::from_bytes(
        block_light.clone().try_into().unwrap(),
    ));
    chunk.section_lights[0].sky = Some(crate::chunk::LightSection::from_bytes(
        sky_light.clone().try_into().unwrap(),
    ));
    chunk.section_lights[4].sky = Some(crate::chunk::LightSection::from_bytes(
        block_light.clone().try_into().unwrap(),
    ));
    chunk.mark_dirty();

    let mut world =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(cpos, chunk).unwrap();
    assert_eq!(world.dirty_count(), 1);

    assert_eq!(world.flush_dirty().unwrap(), 1);
    assert_eq!(world.dirty_count(), 0);
    drop(world);

    let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
    let chunk = reopened.get_chunk(cpos).unwrap().unwrap();

    assert_eq!(
        chunk.section_lights[0]
            .block
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(block_light.clone())
    );
    assert_eq!(
        chunk.section_lights[0]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(sky_light)
    );
    assert_eq!(chunk.section_lights[1].block, None);
    assert_eq!(chunk.section_lights[1].sky, None);
    assert_eq!(chunk.section_lights[4].block, None);
    assert_eq!(
        chunk.section_lights[4]
            .sky
            .as_ref()
            .map(|layer| layer.to_vec()),
        Some(block_light)
    );
}

#[test]
fn unknown_root_extras_survive_world_storage_edit_flush_reopen() {
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

    let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
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

#[test]
fn get_chunk_mut_does_not_mark_read_like_access_dirty() {
    let registry = single_air_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    world
        .insert_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();

    let chunk = world.get_chunk_mut(cpos).unwrap().unwrap();

    assert_eq!(chunk.dirty_generation, 0);
    assert_eq!(world.dirty_count(), 0);
}

#[test]
fn dirty_lru_pressure_rejects_growth_without_flushing_under_insert() {
    use crate::chunk::ChunkGenerator;
    use mc_data::Identifier;

    struct StubGen;

    impl ChunkGenerator for StubGen {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            let air = BlockStateId(0);
            let biome = Identifier::parse("minecraft:plains").unwrap();
            let mut chunk = Chunk::empty(pos, air, biome);
            chunk.set_block(0, 0, 0, BlockStateId(1));
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
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let mut world = WorldStorage::open_with_capacity(tmp_world.path(), registry, 1)
        .unwrap()
        .with_generator(Arc::new(StubGen));

    assert_eq!(
        world.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
        Some(BlockStateId(1))
    );
    assert_eq!(world.dirty_count(), 1);

    assert!(matches!(
        world.get_block(BlockPos { x: 16, y: 0, z: 0 }),
        Err(WorldError::ChunkCachePressure {
            save_healthy: true,
            ..
        })
    ));

    assert_eq!(world.cache_len(), 1);
    assert_eq!(world.dirty_count(), 1);
    assert!(!tmp_world.path().join("region/r.0.0.mca").exists());
}

#[test]
fn fluid_state_and_scheduled_tick_survive_flush_and_reopen() {
    use mc_data::Identifier;
    use std::collections::BTreeMap;

    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let mut water_properties = BTreeMap::new();
    water_properties.insert("level".to_string(), vec!["0".to_string(), "1".to_string()]);
    let report = vec![
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:water").unwrap(),
            properties: water_properties,
            states: vec![
                mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::from([("level".to_string(), "0".to_string())]),
                },
                mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: false,
                    properties: BTreeMap::from([("level".to_string(), "1".to_string())]),
                },
            ],
        },
    ];
    let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let water = Identifier::parse("minecraft:water").unwrap();
    let mut world =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    assert!(
        world
            .schedule_fluid_tick(ScheduledFluidTick::new(pos, water.clone(), 12, 0))
            .unwrap()
    );

    assert_eq!(world.flush_dirty().unwrap(), 1);
    drop(world);

    let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
    assert_eq!(reopened.get_block(pos).unwrap(), Some(BlockStateId(1)));
    let ticks = reopened.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].fluid, water);
    assert_eq!(ticks[0].trigger_tick, 12);
}

/// M6.b: the spawn-burst load path (read 121 chunks) must not
/// produce any dirty chunks (chunks decoded from disk start
/// clean). This guards against an accidental `dirty = true`
/// default that would turn the burst into an I/O storm.

#[test]
#[ignore = "requires local .analysis/test-world and 26.1.2 blocks report"]
fn spawn_burst_load_does_not_dirty_chunks() {
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
    let mut world = WorldStorage::open_with_capacity(&world_dir, registry, 4).unwrap();
    for cz in 0..=10 {
        for cx in 0..=10 {
            assert!(
                world
                    .get_chunk(ChunkPos { x: cx, z: cz })
                    .unwrap()
                    .is_some(),
                "test world must contain required chunk ({cx}, {cz})"
            );
        }
    }
    assert_eq!(world.dirty_count(), 0);
}

/// M7.c: a `WorldStorage` opened on a path *without* region
/// files but with a generator attached resolves every chunk
/// position to a non-empty `Chunk`.
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
