use super::super::test_support::*;
use super::super::*;

#[test]
fn resident_scheduled_hopper_transfer_consumes_due_tick_and_sets_journal_fence() {
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
    let due = ScheduledBlockTick::new(
        hopper_position,
        Identifier::parse("minecraft:hopper").unwrap(),
        20,
        0,
    );
    world.schedule_block_tick(due.clone()).unwrap();
    let mut updated_hopper = hopper;
    updated_hopper.slots[0] = crate::FurnaceSlot::EMPTY;
    updated_hopper.transfer_cooldown = 8;
    let mut updated_chest = chest;
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
            expected: world.hopper_block_entity(hopper_position).unwrap().unwrap(),
            updated: updated_hopper.clone(),
        }],
        chests: vec![crate::ResidentBlockEntityChange {
            position: chest_position,
            expected: world.chest_block_entity(chest_position).unwrap().unwrap(),
            updated: updated_chest.clone(),
        }],
        furnaces: Vec::new(),
        scheduled_block_ticks: vec![next_tick.clone()],
    };
    let mutation = world.mutation_view();

    let (result, touched) =
        mutation.commit_scheduled_hopper_transfer_conditionally_journaled(7, &[due], &plan);

    assert_eq!(result, crate::ResidentHopperTransferCommitResult::Applied);
    assert_eq!(touched, vec![cpos]);
    assert_eq!(
        world.hopper_block_entity(hopper_position).unwrap(),
        Some(updated_hopper)
    );
    assert_eq!(
        world.chest_block_entity(chest_position).unwrap(),
        Some(updated_chest)
    );
    let scheduled = world.scheduled_block_ticks(cpos).unwrap().unwrap();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, next_tick.pos);
    assert_eq!(scheduled[0].block, next_tick.block);
    assert_eq!(scheduled[0].trigger_tick, next_tick.trigger_tick);
    assert_eq!(scheduled[0].priority, next_tick.priority);
    let snapshot = world.cached_chunk_snapshot(cpos).unwrap();
    assert_eq!(snapshot.world_journal_lsn(), 7);
    assert!(world.plan_dirty_flush().unwrap().is_empty());
    assert_eq!(mutation.clear_journal_pending_conditionally(7, &[cpos]), 1);
    assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
}

#[test]
fn world_journal_lsn_survives_anvil_flush_and_reopen() {
    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let registry = single_air_registry();
    let position = ChunkPos { x: -1, z: 2 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(position, BlockStateId(0), biome);
    chunk.set_world_journal_lsn(73);

    let mut world =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    world.insert_chunk(position, chunk).unwrap();
    assert_eq!(world.flush_dirty().unwrap(), 1);
    drop(world);

    let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
    assert_eq!(
        reopened
            .get_chunk(position)
            .unwrap()
            .unwrap()
            .world_journal_lsn(),
        73
    );
}

#[test]
fn journaled_resident_batch_stamps_complete_sorted_touched_footprint() {
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
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    for x in 0..=2 {
        let position = ChunkPos { x, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
    }
    let direct = BlockPos { x: 1, y: 1, z: 1 };
    let requested = BlockPos { x: 31, y: 1, z: 1 };
    let leaf = BlockPos { x: 32, y: 1, z: 1 };
    world.set_block_at(direct, BlockStateId(1)).unwrap();
    world.set_block_at(requested, BlockStateId(1)).unwrap();
    world.set_block_at(leaf, BlockStateId(2)).unwrap();
    let direct_token = world.block_mutation_token(direct).unwrap();
    let requested_token = world.block_mutation_token(requested).unwrap();

    let (result, touched) = world
        .mutation_view()
        .apply_block_edits_conditionally_journaled(
            91,
            &[
                crate::ResidentBlockEdit {
                    pos: direct,
                    new_state: BlockStateId(0),
                    preserve_light: false,
                },
                crate::ResidentBlockEdit {
                    pos: requested,
                    new_state: BlockStateId(0),
                    preserve_light: false,
                },
            ],
            &[
                crate::ResidentBlockPrecondition {
                    pos: direct,
                    expected_state: BlockStateId(1),
                    expected_token: direct_token,
                },
                crate::ResidentBlockPrecondition {
                    pos: requested,
                    expected_state: BlockStateId(1),
                    expected_token: requested_token,
                },
            ],
            &[ScheduledBlockTick::new(
                requested,
                Identifier::parse("minecraft:air").unwrap(),
                20,
                0,
            )],
            None,
            Some(12),
        );

    assert!(matches!(
        result,
        crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 2
    ));
    assert_eq!(
        touched,
        vec![
            ChunkPos { x: 0, z: 0 },
            ChunkPos { x: 1, z: 0 },
            ChunkPos { x: 2, z: 0 },
        ]
    );
    for position in touched {
        assert_eq!(
            world
                .cached_chunk_snapshot(position)
                .unwrap()
                .world_journal_lsn(),
            91
        );
    }
    assert_eq!(
        world
            .scheduled_block_ticks(ChunkPos { x: 1, z: 0 })
            .unwrap()
            .unwrap(),
        &[ScheduledBlockTick::new(
            requested,
            Identifier::parse("minecraft:air").unwrap(),
            20,
            0,
        )]
    );
    assert_eq!(
        world
            .scheduled_block_ticks(ChunkPos { x: 2, z: 0 })
            .unwrap()
            .unwrap(),
        &[ScheduledBlockTick::new(
            leaf,
            Identifier::parse("minecraft:oak_leaves").unwrap(),
            12,
            0,
        )]
    );
}

#[test]
fn dirty_flush_plan_excludes_journal_pending_chunk() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let position = ChunkPos { x: 0, z: 0 };
    let flushable = ChunkPos { x: 1, z: 0 };
    for chunk_position in [position, flushable] {
        world
            .insert_generated_chunk(
                chunk_position,
                Chunk::empty(
                    chunk_position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }

    let (result, touched) = world
        .mutation_view()
        .apply_block_edits_conditionally_journaled(
            41,
            &[crate::ResidentBlockEdit {
                pos: BlockPos { x: 1, y: 1, z: 1 },
                new_state: BlockStateId(1),
                preserve_light: false,
            }],
            &[],
            &[],
            None,
            None,
        );

    assert!(matches!(
        result,
        crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
    ));
    assert_eq!(touched, vec![position]);
    let plan = world.plan_dirty_flush().unwrap();
    assert_eq!(plan.chunk_count(), 1);
    assert_eq!(plan.dirty_chunks_at_capture(), 2);
    assert!(!plan.captures_all_dirty_chunks());
    assert!(matches!(
        world.flush_dirty(),
        Err(WorldError::JournalPendingDirtyChunks {
            dirty_chunks: 2,
            flushable_chunks: 1,
        })
    ));
    assert_eq!(world.dirty_count(), 2);
}

#[test]
fn clearing_journal_pending_chunk_makes_it_flushable() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let position = ChunkPos { x: 0, z: 0 };
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
    let mutation_view = world.mutation_view();
    let (_, touched) = mutation_view.apply_block_edits_conditionally_journaled(
        42,
        &[crate::ResidentBlockEdit {
            pos: BlockPos { x: 1, y: 1, z: 1 },
            new_state: BlockStateId(1),
            preserve_light: false,
        }],
        &[],
        &[],
        None,
        None,
    );
    let notifications = Arc::new(AtomicUsize::new(0));
    world.set_dirty_high_water_notifier({
        let notifications = Arc::clone(&notifications);
        Arc::new(move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        })
    });

    assert!(world.plan_dirty_flush().unwrap().is_empty());
    assert_eq!(
        mutation_view.clear_journal_pending_conditionally(42, &touched),
        1
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert_eq!(
        mutation_view.clear_journal_pending_conditionally(41, &touched),
        0
    );
    assert_eq!(notifications.load(Ordering::SeqCst), 1);
    assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
    assert_eq!(world.dirty_count(), 1);
}

#[test]
fn stale_journal_completion_cannot_clear_newer_pending_lsn() {
    let registry = air_stone_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let position = ChunkPos { x: 0, z: 0 };
    let block = BlockPos { x: 1, y: 1, z: 1 };
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
    let mutation_view = world.mutation_view();
    let (_, first_touched) = mutation_view.apply_block_edits_conditionally_journaled(
        50,
        &[crate::ResidentBlockEdit {
            pos: block,
            new_state: BlockStateId(1),
            preserve_light: false,
        }],
        &[],
        &[],
        None,
        None,
    );
    let (_, second_touched) = mutation_view.apply_block_edits_conditionally_journaled(
        51,
        &[crate::ResidentBlockEdit {
            pos: block,
            new_state: BlockStateId(0),
            preserve_light: false,
        }],
        &[],
        &[],
        None,
        None,
    );

    assert_eq!(
        mutation_view.clear_journal_pending_conditionally(50, &first_touched),
        0
    );
    assert!(world.plan_dirty_flush().unwrap().is_empty());
    assert_eq!(
        mutation_view.clear_journal_pending_conditionally(51, &second_touched),
        1
    );
    assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
}

#[test]
fn coordinator_journal_stamp_fences_flush_until_exact_clear() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let position = ChunkPos { x: 0, z: 0 };
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
    let mutation = world.mutation_view();

    let crate::JournalStampResult::Stamped(snapshots) =
        world.stamp_cached_chunks_for_world_journal(7, &[position])
    else {
        panic!("cached chunk accepts coordinator journal stamp");
    };

    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].world_journal_lsn(), 7);
    assert!(world.plan_dirty_flush().unwrap().is_empty());
    assert_eq!(
        mutation.clear_journal_pending_conditionally(7, &[position]),
        1
    );
    assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
}

#[test]
fn coordinator_journal_stamp_never_decreases_lsn() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let position = ChunkPos { x: 0, z: 0 };
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
    assert!(matches!(
        world.stamp_cached_chunks_for_world_journal(12, &[position]),
        crate::JournalStampResult::Stamped(_)
    ));

    assert!(matches!(
        world.stamp_cached_chunks_for_world_journal(11, &[position]),
        crate::JournalStampResult::NewerDecision(12)
    ));
    assert_eq!(
        world
            .cached_chunk_snapshot(position)
            .unwrap()
            .world_journal_lsn(),
        12
    );
}

#[test]
fn journal_replay_applies_only_images_newer_than_disk_or_resident_chunk() {
    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
    let registry = air_stone_registry();
    let position = ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut durable = Chunk::empty(position, BlockStateId(0), biome.clone());
    durable.set_block(1, 0, 1, BlockStateId(1));
    durable.set_world_journal_lsn(10);

    let mut initial =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    initial.insert_chunk(position, durable).unwrap();
    assert_eq!(initial.flush_dirty().unwrap(), 1);
    drop(initial);

    let mut reopened =
        WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
    let mut equal = Chunk::empty(position, BlockStateId(0), biome.clone());
    equal.set_world_journal_lsn(10);
    assert!(!reopened.replay_journal_chunk(equal).unwrap());
    assert_eq!(
        reopened.get_block(BlockPos { x: 1, y: 0, z: 1 }).unwrap(),
        Some(BlockStateId(1))
    );

    let mut older = Chunk::empty(position, BlockStateId(0), biome.clone());
    older.set_world_journal_lsn(9);
    assert!(!reopened.replay_journal_chunk(older).unwrap());

    let mut newer = Chunk::empty(position, BlockStateId(0), biome);
    newer.set_world_journal_lsn(11);
    assert!(reopened.replay_journal_chunk(newer).unwrap());
    let replayed = reopened.cached_chunk_snapshot(position).unwrap();
    assert_eq!(replayed.world_journal_lsn(), 11);
    assert_eq!(replayed.get_block(1, 0, 1), Some(BlockStateId(0)));
    assert!(replayed.dirty);
}
