use super::super::test_support::*;
use super::super::*;

#[test]
fn block_mutation_version_advances_on_changes_and_detects_aba() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
        )
        .unwrap();
    let pos = BlockPos { x: 1, y: 0, z: 1 };

    let initial = world.block_mutation_token(pos).expect("initial token");
    assert_eq!(initial.version, 0);
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    let first = world.block_mutation_token(pos).expect("first token");
    assert_eq!(first.chunk_instance_id, initial.chunk_instance_id);
    assert_eq!(first.version, 1);
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    assert_eq!(world.block_mutation_token(pos), Some(first));
    world.set_block_at(pos, BlockStateId(0)).unwrap();
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(1)));
    assert_eq!(
        world.block_mutation_token(pos).expect("ABA token").version,
        3
    );
}

#[test]
fn resident_mutation_is_canonical_and_independent_between_regions() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let held_region = ChunkPos { x: 0, z: 0 };
    let target_chunk = ChunkPos { x: 8, z: 0 };
    for position in [held_region, target_chunk] {
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let target = BlockPos {
        x: target_chunk.x * SECTION_DIM as i32,
        y: 0,
        z: target_chunk.z * SECTION_DIM as i32,
    };
    let expected_token = world.block_mutation_token(target).expect("target token");
    let read_view = world.read_view();
    let mutation = world.mutation_view();
    let held = read_view.lock_chunk_shard_for_test(held_region);
    let (completed, observed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = mutation.set_block_if_current(
            target,
            BlockStateId(0),
            expected_token,
            BlockStateId(1),
            false,
        );
        completed.send(result).expect("mutation completion");
    });

    let result = observed.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    worker.join().expect("regional mutation worker");

    assert_eq!(
        result,
        Ok(crate::ResidentBlockMutation::Applied(BlockStateId(0)))
    );
    assert_eq!(
        world
            .cached_chunk_snapshot(target_chunk)
            .unwrap()
            .get_block(0, 0, 0),
        Some(BlockStateId(1))
    );
}

#[test]
fn resident_batch_rejects_stale_precondition_without_partial_mutation() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let first = BlockPos { x: 1, y: 0, z: 1 };
    let stale = BlockPos { x: 2, y: 0, z: 1 };
    let first_token = world.block_mutation_token(first).unwrap();
    let mut stale_token = world.block_mutation_token(stale).unwrap();
    stale_token.version += 1;

    let result = world.mutation_view().apply_block_edits_conditionally(
        &[
            crate::ResidentBlockEdit {
                pos: first,
                new_state: BlockStateId(1),
                preserve_light: false,
            },
            crate::ResidentBlockEdit {
                pos: stale,
                new_state: BlockStateId(1),
                preserve_light: false,
            },
        ],
        &[
            crate::ResidentBlockPrecondition {
                pos: first,
                expected_state: BlockStateId(0),
                expected_token: first_token,
            },
            crate::ResidentBlockPrecondition {
                pos: stale,
                expected_state: BlockStateId(0),
                expected_token: stale_token,
            },
        ],
        &[],
        None,
        None,
    );

    assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
    assert_eq!(world.get_cached_block(first), Some(BlockStateId(0)));
    assert_eq!(world.get_cached_block(stale), Some(BlockStateId(0)));
    assert_eq!(world.block_mutation_token(first), Some(first_token));
}

#[test]
fn resident_fluid_tick_commit_consumes_edits_and_reschedules_atomically() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let source = BlockPos { x: 1, y: 0, z: 1 };
    let target = BlockPos { x: 1, y: 1, z: 1 };
    let water = Identifier::parse("minecraft:water").unwrap();
    world.set_block_at(target, BlockStateId(1)).unwrap();
    let due = ScheduledFluidTick::new(source, water.clone(), 10, 0);
    world.schedule_fluid_tick(due.clone()).unwrap();
    let target_token = world.block_mutation_token(target).unwrap();
    let follow_up = ScheduledFluidTick::new(target, water, 15, 0);

    let (result, touched) = world
        .mutation_view()
        .apply_fluid_tick_plan_conditionally_journaled(
            7,
            &crate::ResidentFluidTickPlan {
                consumed_ticks: &[due],
                edits: &[crate::ResidentBlockEdit {
                    pos: target,
                    new_state: BlockStateId(2),
                    preserve_light: false,
                }],
                preconditions: &[crate::ResidentBlockPrecondition {
                    pos: target,
                    expected_state: BlockStateId(1),
                    expected_token: target_token,
                }],
                scheduled_ticks: std::slice::from_ref(&follow_up),
                light_table: None,
                leaf_trigger_tick: None,
            },
        );

    assert!(matches!(
        result,
        crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
    ));
    assert_eq!(touched, vec![chunk_pos]);
    let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
    assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(2)));
    let scheduled = chunk.scheduled_fluid_ticks();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, follow_up.pos);
    assert_eq!(scheduled[0].fluid, follow_up.fluid);
    assert_eq!(scheduled[0].trigger_tick, follow_up.trigger_tick);
    assert_eq!(scheduled[0].priority, follow_up.priority);
    assert_eq!(scheduled[0].sequence(), 1);
    assert_eq!(chunk.world_journal_lsn(), 7);
}

#[test]
fn resident_fluid_tick_stale_plan_keeps_due_tick_and_block() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let source = BlockPos { x: 1, y: 0, z: 1 };
    let target = BlockPos { x: 1, y: 1, z: 1 };
    let due = ScheduledFluidTick::new(source, Identifier::parse("minecraft:water").unwrap(), 10, 0);
    world.schedule_fluid_tick(due.clone()).unwrap();
    let mut stale_token = world.block_mutation_token(target).unwrap();
    stale_token.version += 1;

    let result =
        world
            .mutation_view()
            .apply_fluid_tick_plan_conditionally(&crate::ResidentFluidTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[crate::ResidentBlockEdit {
                    pos: target,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                }],
                preconditions: &[crate::ResidentBlockPrecondition {
                    pos: target,
                    expected_state: BlockStateId(0),
                    expected_token: stale_token,
                }],
                scheduled_ticks: &[],
                light_table: None,
                leaf_trigger_tick: None,
            });

    assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
    let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
    assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(0)));
    assert_eq!(chunk.scheduled_fluid_ticks(), &[due]);
}

#[test]
fn resident_scheduled_block_tick_commit_consumes_and_edits_atomically() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let position = BlockPos { x: 1, y: 1, z: 1 };
    world.set_block_at(position, BlockStateId(1)).unwrap();
    let due = ScheduledBlockTick::new(
        position,
        Identifier::parse("minecraft:stone").unwrap(),
        10,
        0,
    );
    world.schedule_block_tick(due.clone()).unwrap();
    let token = world.block_mutation_token(position).unwrap();

    let (result, touched) = world
        .mutation_view()
        .apply_scheduled_block_tick_plan_conditionally_journaled(
            8,
            &crate::ResidentScheduledBlockTickPlan {
                consumed_ticks: &[due],
                edits: &[crate::ResidentBlockEdit {
                    pos: position,
                    new_state: BlockStateId(2),
                    preserve_light: false,
                }],
                preconditions: &[crate::ResidentBlockPrecondition {
                    pos: position,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                }],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );

    assert!(matches!(
        result,
        crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
    ));
    assert_eq!(touched, vec![chunk_pos]);
    let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
    assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(2)));
    assert!(chunk.scheduled_block_ticks().is_empty());
    assert_eq!(chunk.world_journal_lsn(), 8);
}

#[test]
fn resident_scheduled_block_tick_stale_plan_keeps_due_tick() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let position = BlockPos { x: 1, y: 1, z: 1 };
    let due = ScheduledBlockTick::new(
        position,
        Identifier::parse("minecraft:stone").unwrap(),
        10,
        0,
    );
    world.schedule_block_tick(due.clone()).unwrap();
    let mut stale_token = world.block_mutation_token(position).unwrap();
    stale_token.version += 1;

    let result = world
        .mutation_view()
        .apply_scheduled_block_tick_plan_conditionally(&crate::ResidentScheduledBlockTickPlan {
            consumed_ticks: std::slice::from_ref(&due),
            edits: &[crate::ResidentBlockEdit {
                pos: position,
                new_state: BlockStateId(1),
                preserve_light: false,
            }],
            preconditions: &[crate::ResidentBlockPrecondition {
                pos: position,
                expected_state: BlockStateId(0),
                expected_token: stale_token,
            }],
            light_table: None,
            leaf_trigger_tick: None,
        });

    assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
    assert_eq!(
        world
            .cached_chunk_snapshot(chunk_pos)
            .unwrap()
            .scheduled_block_ticks(),
        &[due]
    );
}

#[test]
fn resident_hopper_tick_backfill_is_idempotent() {
    let registry = air_stone_hopper_registry();
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let position = BlockPos { x: 1, y: 1, z: 1 };
    world.set_block_at(position, BlockStateId(2)).unwrap();
    world
        .set_hopper_block_entity(position, HopperBlockEntity::default())
        .unwrap();
    let mutation = world.mutation_view();

    assert_eq!(mutation.backfill_hopper_ticks(&[chunk_pos], 20), 1);
    assert_eq!(mutation.backfill_hopper_ticks(&[chunk_pos], 20), 0);
    let ticks = world
        .cached_chunk_snapshot(chunk_pos)
        .unwrap()
        .scheduled_block_ticks()
        .to_vec();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, position);
    assert_eq!(ticks[0].block.as_str(), "minecraft:hopper");
    assert_eq!(ticks[0].trigger_tick, 20);
}

#[test]
fn resident_batch_schedules_tick_only_for_applied_position() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let changed = BlockPos { x: 1, y: 0, z: 1 };
    let unchanged = BlockPos { x: 2, y: 0, z: 1 };
    let changed_token = world.block_mutation_token(changed).unwrap();
    let unchanged_token = world.block_mutation_token(unchanged).unwrap();
    let changed_tick =
        ScheduledBlockTick::new(changed, Identifier::parse("minecraft:air").unwrap(), 20, 0);
    let unchanged_tick = ScheduledBlockTick::new(
        unchanged,
        Identifier::parse("minecraft:air").unwrap(),
        20,
        0,
    );

    let result = world.mutation_view().apply_block_edits_conditionally(
        &[
            crate::ResidentBlockEdit {
                pos: changed,
                new_state: BlockStateId(1),
                preserve_light: false,
            },
            crate::ResidentBlockEdit {
                pos: unchanged,
                new_state: BlockStateId(0),
                preserve_light: false,
            },
        ],
        &[
            crate::ResidentBlockPrecondition {
                pos: changed,
                expected_state: BlockStateId(0),
                expected_token: changed_token,
            },
            crate::ResidentBlockPrecondition {
                pos: unchanged,
                expected_state: BlockStateId(0),
                expected_token: unchanged_token,
            },
        ],
        &[changed_tick.clone(), unchanged_tick],
        None,
        None,
    );

    let crate::ResidentBlockEditBatchResult::Applied(applied) = result else {
        panic!("resident batch did not commit");
    };
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].pos, changed);
    assert_eq!(world.get_cached_block(changed), Some(BlockStateId(1)));
    assert_eq!(
        world.scheduled_block_ticks(chunk_pos).unwrap().unwrap(),
        &[changed_tick]
    );
}

#[test]
fn resident_batch_rejects_cross_region_before_mutation() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let first_chunk = ChunkPos { x: 0, z: 0 };
    let other_chunk = ChunkPos { x: 8, z: 0 };
    for chunk_pos in [first_chunk, other_chunk] {
        world
            .insert_generated_chunk(
                chunk_pos,
                Chunk::empty(chunk_pos, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let first = BlockPos { x: 1, y: 0, z: 1 };
    let other = BlockPos {
        x: other_chunk.x * SECTION_DIM as i32,
        y: 0,
        z: 1,
    };

    let result = world.mutation_view().apply_block_edits_conditionally(
        &[
            crate::ResidentBlockEdit {
                pos: first,
                new_state: BlockStateId(1),
                preserve_light: false,
            },
            crate::ResidentBlockEdit {
                pos: other,
                new_state: BlockStateId(1),
                preserve_light: false,
            },
        ],
        &[],
        &[],
        None,
        None,
    );

    assert_eq!(result, crate::ResidentBlockEditBatchResult::CrossRegion);
    assert_eq!(world.get_cached_block(first), Some(BlockStateId(0)));
    assert_eq!(world.get_cached_block(other), Some(BlockStateId(0)));
}

#[test]
fn resident_batch_updates_highest_opaque_inside_commit() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    world
        .set_baked_light(chunk_pos, &ChunkLight::filled(15, 0))
        .unwrap();
    let pos = BlockPos { x: 1, y: 0, z: 1 };
    let token = world.block_mutation_token(pos).unwrap();
    let light = BlockLightTable::from_arrays(
        "resident batch test",
        vec![0, 0],
        vec![0, 15],
        vec![true, false],
    );

    let result = world.mutation_view().apply_block_edits_conditionally(
        &[crate::ResidentBlockEdit {
            pos,
            new_state: BlockStateId(1),
            preserve_light: false,
        }],
        &[crate::ResidentBlockPrecondition {
            pos,
            expected_state: BlockStateId(0),
            expected_token: token,
        }],
        &[],
        Some(&light),
        None,
    );

    let crate::ResidentBlockEditBatchResult::Applied(applied) = result else {
        panic!("resident light-changing batch did not commit");
    };
    assert_eq!(applied.len(), 1);
    assert!(applied[0].previous_light.is_some());
    assert_eq!(
        world
            .cached_chunk_snapshot(chunk_pos)
            .unwrap()
            .highest_opaque_y(1, 1),
        Some(0)
    );
}

#[test]
fn resident_batch_schedules_adjacent_leaf_inside_commit() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
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
                id: Identifier::parse("minecraft:oak_leaves").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ])
        .unwrap(),
    );
    let mut world = WorldStorage::in_memory(registry);
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
        .unwrap();
    let trunk = BlockPos { x: 1, y: 1, z: 1 };
    let leaf = BlockPos { x: 2, y: 1, z: 1 };
    world.set_block_at(trunk, BlockStateId(1)).unwrap();
    world.set_block_at(leaf, BlockStateId(2)).unwrap();
    let token = world.block_mutation_token(trunk).unwrap();

    let result = world.mutation_view().apply_block_edits_conditionally(
        &[crate::ResidentBlockEdit {
            pos: trunk,
            new_state: BlockStateId(0),
            preserve_light: false,
        }],
        &[crate::ResidentBlockPrecondition {
            pos: trunk,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        &[],
        None,
        Some(12),
    );

    assert!(matches!(
        result,
        crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
    ));
    assert_eq!(
        world.scheduled_block_ticks(chunk_pos).unwrap().unwrap(),
        &[ScheduledBlockTick::new(
            leaf,
            Identifier::parse("minecraft:oak_leaves").unwrap(),
            12,
            0,
        )]
    );
}

#[test]
fn baked_light_replaces_published_chunk_snapshot() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let read_view = world.read_view();
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let before = read_view.snapshot_chunks(&[cpos]);
    assert!(ChunkLight::from_section_lights(&before.chunk(cpos).unwrap().section_lights).is_none());

    let baked = ChunkLight::filled(15, 0);
    assert!(world.set_baked_light(cpos, &baked).unwrap());

    let after = read_view.snapshot_chunks(&[cpos]);
    assert!(ChunkLight::from_section_lights(&after.chunk(cpos).unwrap().section_lights).is_some());
    assert!(
        ChunkLight::from_section_lights(&before.chunk(cpos).unwrap().section_lights).is_none(),
        "an already-issued immutable snapshot must not change"
    );
}

#[test]
fn stale_regional_baked_light_publish_is_all_or_nothing() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let read_view = world.read_view();
    let mutation_view = world.mutation_view();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let first = ChunkPos { x: 0, z: 0 };
    let second = ChunkPos {
        x: crate::resident::WORLD_REGION_AXIS_CHUNKS,
        z: 0,
    };
    for position in [first, second] {
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let expected = [first, second]
        .into_iter()
        .map(|position| {
            let snapshot = read_view.snapshot_chunks(&[position]);
            (position, snapshot.chunk(position))
        })
        .collect::<HashMap<_, _>>();

    let newer_second_light = ChunkLight::filled(7, 3);
    world.set_baked_light(second, &newer_second_light).unwrap();
    let proposed = ChunkLight::filled(15, 0);

    assert!(
        !mutation_view.publish_baked_light_conditionally(
            &expected,
            [(first, &proposed), (second, &proposed)],
        )
    );
    let after = read_view.snapshot_chunks(&[first, second]);
    let first_after = after.chunk(first).unwrap();
    let second_after = after.chunk(second).unwrap();
    assert!(ChunkLight::from_section_lights(&first_after.section_lights).is_none());
    assert_eq!(
        ChunkLight::from_section_lights(&second_after.section_lights),
        Some(newer_second_light)
    );
}

#[test]
fn furnace_block_entities_are_chunk_scoped_runtime_state() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    let mut furnace = world.furnace_block_entity(pos).unwrap().unwrap();
    assert!(furnace.slots[0].is_empty());
    furnace.slots[0] = crate::chunk::FurnaceSlot {
        count: 2,
        item_id: 42,
        damage: Some(7),
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    furnace.cook_progress = 11;

    assert!(
        world
            .set_furnace_block_entity(pos, furnace.clone())
            .unwrap()
    );
    assert_eq!(world.dirty_count(), 1);
    assert_eq!(world.furnace_block_entity(pos).unwrap(), Some(furnace));
}

#[test]
fn resident_double_chest_commit_preflights_before_any_write() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let positions = [BlockPos { x: 1, y: 2, z: 3 }, BlockPos { x: 2, y: 2, z: 3 }];
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let mut initial = [ChestBlockEntity::default(), ChestBlockEntity::default()];
    initial[0].slots[0].item_id = 41;
    initial[0].slots[0].count = 1;
    initial[1].slots[0].item_id = 42;
    initial[1].slots[0].count = 2;
    for (&position, chest) in positions.iter().zip(&initial) {
        world
            .set_chest_block_entity(position, chest.clone())
            .unwrap();
    }
    let mutation = world.mutation_view();
    let mut updated = initial.clone();
    updated[0].slots[0].count = 3;
    updated[1].slots[0].count = 4;
    let mut stale = initial.clone();
    stale[1].slots[0].count = 99;

    assert!(matches!(
        mutation.commit_chests_conditionally(&positions, &stale, &updated),
        crate::ResidentChestCommitResult::Rejected(authoritative)
            if authoritative == initial
    ));
    for (&position, chest) in positions.iter().zip(&initial) {
        assert_eq!(
            world.chest_block_entity(position).unwrap(),
            Some(chest.clone())
        );
    }

    assert_eq!(
        mutation.commit_chests_conditionally(&positions, &initial, &updated),
        crate::ResidentChestCommitResult::Applied
    );
    for (&position, chest) in positions.iter().zip(&updated) {
        assert_eq!(
            world.chest_block_entity(position).unwrap(),
            Some(chest.clone())
        );
    }
}

#[test]
fn resident_furnace_commit_rejects_stale_before_write() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let mut initial = FurnaceBlockEntity::default();
    initial.slots[0].item_id = 41;
    initial.slots[0].count = 2;
    world
        .set_furnace_block_entity(position, initial.clone())
        .unwrap();
    let mutation = world.mutation_view();
    let mut stale = initial.clone();
    stale.slots[0].count = 99;
    let mut updated = initial.clone();
    updated.slots[0].count = 1;

    assert!(matches!(
        mutation.commit_furnace_conditionally(position, &stale, &updated),
        crate::ResidentFurnaceCommitResult::Rejected(authoritative)
            if *authoritative == initial
    ));
    assert_eq!(
        world.furnace_block_entity(position).unwrap(),
        Some(initial.clone())
    );

    assert_eq!(
        mutation.commit_furnace_conditionally(position, &initial, &updated),
        crate::ResidentFurnaceCommitResult::Applied
    );
    assert_eq!(world.furnace_block_entity(position).unwrap(), Some(updated));
}

#[test]
fn resident_furnace_tick_commit_rejects_stale_burn_state() {
    let registry = air_stone_furnace_registry();
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(position, BlockStateId(2)).unwrap();
    let initial = FurnaceBlockEntity {
        burn_remaining: 10,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    world
        .set_furnace_block_entity(position, initial.clone())
        .unwrap();
    let mutation = world.mutation_view();
    let mut current = initial.clone();
    current.burn_remaining = 9;
    world
        .set_furnace_block_entity(position, current.clone())
        .unwrap();
    let mut stale_update = initial.clone();
    stale_update.burn_remaining = 8;

    assert_eq!(
        mutation.commit_furnace_tick_conditionally(
            position,
            BlockStateId(2),
            BlockStateId(2),
            &initial,
            &stale_update,
        ),
        crate::ResidentFurnaceTickCommitResult::Stale
    );
    assert_eq!(
        mutation.furnace_tick_snapshot(position),
        Some((BlockStateId(2), current))
    );
}

#[test]
fn resident_furnace_tick_commit_changes_state_and_entity_together() {
    let registry = air_stone_furnace_registry();
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let position = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(position, BlockStateId(2)).unwrap();
    let initial = FurnaceBlockEntity::default();
    world
        .set_furnace_block_entity(position, initial.clone())
        .unwrap();
    let updated = FurnaceBlockEntity {
        burn_remaining: 20,
        burn_total: 20,
        ..initial.clone()
    };
    let mutation = world.mutation_view();

    assert_eq!(
        mutation.commit_furnace_tick_conditionally(
            position,
            BlockStateId(2),
            BlockStateId(3),
            &initial,
            &updated,
        ),
        crate::ResidentFurnaceTickCommitResult::Applied
    );
    assert_eq!(
        mutation.furnace_tick_snapshot(position),
        Some((BlockStateId(3), updated))
    );
}

#[test]
fn resident_hopper_transfer_rejects_stale_endpoint_without_partial_write() {
    let registry = air_stone_hopper_registry();
    let mut world = WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    let hopper_position = BlockPos { x: 1, y: 2, z: 3 };
    let chest_position = BlockPos { x: 2, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world
        .set_block_at(hopper_position, BlockStateId(2))
        .unwrap();
    world.set_block_at(chest_position, BlockStateId(1)).unwrap();
    let mut hopper = HopperBlockEntity::default();
    hopper.slots[0].item_id = 42;
    hopper.slots[0].count = 1;
    world
        .set_hopper_block_entity(hopper_position, hopper.clone())
        .unwrap();
    let chest = ChestBlockEntity::default();
    world
        .set_chest_block_entity(chest_position, chest.clone())
        .unwrap();
    let mut updated_hopper = hopper.clone();
    updated_hopper.slots[0] = crate::FurnaceSlot::EMPTY;
    updated_hopper.transfer_cooldown = 8;
    let mut updated_chest = chest.clone();
    updated_chest.slots[0].item_id = 42;
    updated_chest.slots[0].count = 1;
    let next_tick = ScheduledBlockTick::new(
        hopper_position,
        Identifier::parse("minecraft:hopper").unwrap(),
        21,
        0,
    );
    let plan = crate::ResidentHopperTransferPlan {
        expected_states: vec![
            (hopper_position, BlockStateId(2)),
            (chest_position, BlockStateId(1)),
        ],
        hoppers: vec![crate::ResidentBlockEntityChange {
            position: hopper_position,
            expected: hopper.clone(),
            updated: updated_hopper.clone(),
        }],
        chests: vec![crate::ResidentBlockEntityChange {
            position: chest_position,
            expected: chest.clone(),
            updated: updated_chest.clone(),
        }],
        furnaces: Vec::new(),
        scheduled_block_ticks: vec![next_tick.clone()],
    };
    let mutation = world.mutation_view();
    let mut stale = plan.clone();
    stale.chests[0].expected.slots[0].count = 99;

    assert_eq!(
        mutation.commit_hopper_transfer_conditionally(&stale),
        crate::ResidentHopperTransferCommitResult::Stale
    );
    assert_eq!(
        world.hopper_block_entity(hopper_position).unwrap(),
        Some(hopper)
    );
    assert_eq!(
        world.chest_block_entity(chest_position).unwrap(),
        Some(chest)
    );
    assert!(
        world
            .scheduled_block_ticks(cpos)
            .unwrap()
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        mutation.commit_hopper_transfer_conditionally(&plan),
        crate::ResidentHopperTransferCommitResult::Applied
    );
    assert_eq!(
        world.hopper_block_entity(hopper_position).unwrap(),
        Some(updated_hopper)
    );
    assert_eq!(
        world.chest_block_entity(chest_position).unwrap(),
        Some(updated_chest)
    );
    assert_eq!(
        world.scheduled_block_ticks(cpos).unwrap().unwrap(),
        &[next_tick]
    );
}

#[test]
fn hopper_block_entities_are_chunk_scoped_runtime_state() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    let mut hopper = world.hopper_block_entity(pos).unwrap().unwrap();
    assert!(hopper.slots[0].is_empty());
    hopper.slots[4] = crate::chunk::FurnaceSlot {
        count: 2,
        item_id: 42,
        damage: Some(7),
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };

    assert!(world.set_hopper_block_entity(pos, hopper.clone()).unwrap());
    assert_eq!(world.dirty_count(), 1);
    assert_eq!(world.hopper_block_entity(pos).unwrap(), Some(hopper));
}

#[test]
fn scheduled_block_ticks_are_chunk_scoped_runtime_state() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    assert!(
        world
            .schedule_block_tick(ScheduledBlockTick::new(pos, block.clone(), 20, 0))
            .unwrap()
    );
    assert_eq!(world.dirty_count(), 1);
    assert_eq!(
        world.scheduled_block_ticks(cpos).unwrap().unwrap()[0].block,
        block
    );

    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
    assert!(
        world
            .drain_due_block_ticks(cpos, 19, usize::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(world.dirty_count(), 0);

    let due = world.drain_due_block_ticks(cpos, 20, usize::MAX).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].pos, pos);
    assert_eq!(world.dirty_count(), 1);

    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
    assert!(
        world
            .schedule_block_tick(ScheduledBlockTick::new(pos, block, 30, 0))
            .unwrap()
    );
    let removed = world.remove_scheduled_block_ticks_at(pos).unwrap();
    assert_eq!(removed.len(), 1);
    assert!(
        world
            .scheduled_block_ticks(cpos)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn scheduled_fluid_ticks_are_chunk_scoped_runtime_state() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

    assert!(
        world
            .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid.clone(), 20, 0))
            .unwrap()
    );
    assert_eq!(world.dirty_count(), 1);
    assert_eq!(
        world.scheduled_fluid_ticks(cpos).unwrap().unwrap()[0].fluid,
        fluid
    );

    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
    assert!(
        world
            .drain_due_fluid_ticks(cpos, 19, usize::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(world.dirty_count(), 0);

    let due = world.drain_due_fluid_ticks(cpos, 20, usize::MAX).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].pos, pos);
    assert_eq!(world.dirty_count(), 1);

    world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
    assert!(
        world
            .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 30, 0))
            .unwrap()
    );
    let removed = world.remove_scheduled_fluid_ticks_at(pos).unwrap();
    assert_eq!(removed.len(), 1);
    assert!(
        world
            .scheduled_fluid_ticks(cpos)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replacing_chest_block_prunes_stale_block_entity() {
    let registry = air_stone_chest_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = crate::chunk::FurnaceSlot {
        count: 1,
        item_id: 10,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    world.set_chest_block_entity(pos, chest).unwrap();

    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let chunk = world.resident.snapshot(cpos).unwrap();
    assert!(!chunk.chests.contains_key(&pos));
}

#[test]
fn replacing_hopper_block_prunes_stale_block_entity() {
    let registry = air_stone_hopper_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    let mut hopper = crate::chunk::HopperBlockEntity::default();
    hopper.slots[0] = crate::chunk::FurnaceSlot {
        count: 1,
        item_id: 10,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    world.set_hopper_block_entity(pos, hopper).unwrap();

    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let chunk = world.resident.snapshot(cpos).unwrap();
    assert!(!chunk.hoppers.contains_key(&pos));
}
