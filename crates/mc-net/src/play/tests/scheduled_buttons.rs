use super::{
    BlockRegistry, BlockStateId, Chunk, ChunkPipelineResources, ChunkPos, Identifier,
    LoggedInProfile, PlayerPose, ResidentBlockCommit, ScheduledBlockTick, ServerConfig,
    SessionRegistry, SimulationWorldAccess, button_and_door_test_registry, button_test_registry,
    commit_cross_region_scheduled_block_tick, commit_resident_block_edits, in_memory_button_world,
    play_loop_slow_client_test_config, register_loaded_button_session,
    register_ticketed_button_session, simulation_channel,
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::mpsc;
#[tokio::test]
async fn scheduled_button_tick_releases_powered_button() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "ButtonRelease");
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 1);
    let world_writer = world.lock().await;
    let block_tick = owner.run_scheduled_block_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        120,
        1,
    );
    let report = tokio::time::timeout(Duration::from_secs(1), block_tick)
        .await
        .expect("resident scheduled-block commit must not wait for the world writer");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn scheduled_buttons_in_distinct_regions_do_not_wait_for_world_writer() {
    let blocks = Arc::new(button_test_registry());
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let east_chunk = ChunkPos { x: 8, z: 0 };
    storage
        .insert_generated_chunk(
            east_chunk,
            Chunk::empty(
                east_chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let positions = [
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
        mc_world::BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for position in positions {
        storage
            .set_block_at(position, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "RegionalButtonRelease");
    let _ = sessions.mark_loaded(session, (0, 0));
    let _ = sessions.mark_loaded(session, (8, 0));
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, mut owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);
    let world_writer = world.lock().await;
    let mut block_tick = Box::pin(owner.run_scheduled_block_ticks_with_budget(
        &config,
        &sessions,
        SimulationWorldAccess {
            read: Some(&world_read),
            mutation: Some(&world_mutation),
            cpu: Some(&resources),
            light: config.block_light.as_ref(),
        },
        120,
        2,
    ));
    let entered_task = tokio::task::spawn_blocking(move || {
        [entered_rx.recv().unwrap(), entered_rx.recv().unwrap()]
    });
    let entered = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::select! {
            entered = entered_task => entered.unwrap(),
            _ = &mut block_tick => {
                panic!("scheduled regional fanout completed before worker probe")
            }
        }
    })
    .await
    .expect("both scheduled regional workers enter before either release");
    assert_ne!(entered[0], entered[1]);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(1), block_tick)
        .await
        .expect("distinct resident regions complete without the world writer");

    assert_eq!(report.drained, 2);
    assert_eq!(report.applied, 2);
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "one regional wave uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 2);
    for position in positions {
        let chunk_pos = ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        };
        let chunk = restored
            .iter()
            .find(|chunk| chunk.pos == chunk_pos)
            .expect("journaled regional button chunk");
        assert_eq!(
            chunk.get_block(
                position.x.rem_euclid(16) as u8,
                position.y,
                position.z.rem_euclid(16) as u8,
            ),
            Some(mc_world::BlockStateId(1))
        );
        assert!(chunk.scheduled_block_ticks().is_empty());
    }
    drop(world_writer);
    let storage = world.lock().await;
    for position in positions {
        assert_eq!(
            storage.get_cached_block(position),
            Some(mc_world::BlockStateId(1))
        );
    }
}

#[tokio::test]
async fn scheduled_button_regions_replan_when_region_order_repeats() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 8, z: 0 },
        ChunkPos { x: 0, z: 1 },
    ] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let first_button = mc_world::BlockPos { x: 1, y: 64, z: 15 };
    let middle_button = mc_world::BlockPos {
        x: 8 * 16 + 1,
        y: 64,
        z: 1,
    };
    let last_button = mc_world::BlockPos { x: 1, y: 64, z: 17 };
    let lower_door = mc_world::BlockPos { x: 1, y: 64, z: 16 };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    let positions = [first_button, middle_button, last_button];
    for position in positions {
        storage.set_block_at(position, BlockStateId(2)).unwrap();
        storage
            .schedule_block_tick(ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .unwrap();
    }
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(122),
        name: "RepeatedScheduledRegion".to_string(),
    };
    let loaded = HashSet::from([(0, 0), (8, 0), (0, 1)]);
    let (tx, _rx) = mpsc::channel(16);
    let (session, _) = sessions.register(
        &profile,
        (0, 0),
        16,
        loaded.clone(),
        tx,
        PlayerPose::new(1.5, 64.0, 1.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session, chunk);
    }
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = owner
        .run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            3,
        )
        .await;
    drop(world_writer);

    assert_eq!(report.drained, 3);
    assert_eq!(report.applied, 5);
    let mut storage = world.lock().await;
    for position in positions {
        assert_eq!(storage.get_cached_block(position), Some(BlockStateId(1)));
        assert!(
            storage
                .scheduled_block_ticks(ChunkPos {
                    x: position.x.div_euclid(16),
                    z: position.z.div_euclid(16),
                })
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
}

#[tokio::test]
async fn scheduled_button_crossing_region_boundary_commits_without_world_storage() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [ChunkPos { x: 7, z: 0 }, ChunkPos { x: 8, z: 0 }] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let button = mc_world::BlockPos {
        x: 8 * 16 - 1,
        y: 64,
        z: 1,
    };
    let lower_door = mc_world::BlockPos {
        x: 8 * 16,
        y: 64,
        z: 1,
    };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    storage.set_block_at(button, BlockStateId(2)).unwrap();
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    storage
        .schedule_block_tick(ScheduledBlockTick::new(
            button,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(120),
        name: "BoundaryButtonRelease".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let (session, _) = sessions.register(
        &profile,
        (7, 0),
        1,
        HashSet::from([(7, 0), (8, 0)]),
        tx,
        PlayerPose::new(127.5, 64.0, 1.5),
    );
    let _ = sessions.mark_loaded(session, (7, 0));
    let _ = sessions.mark_loaded(session, (8, 0));
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            1,
        ),
    )
    .await
    .expect("cross-region scheduled block transaction must not wait for WorldStorage");
    drop(world_writer);

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 3);
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "boundary commit uses one WAL decision");
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 2);
    let west = restored
        .iter()
        .find(|chunk| chunk.pos == ChunkPos { x: 7, z: 0 })
        .unwrap();
    let east = restored
        .iter()
        .find(|chunk| chunk.pos == ChunkPos { x: 8, z: 0 })
        .unwrap();
    assert_eq!(west.get_block(15, 64, 1), Some(BlockStateId(1)));
    assert!(west.scheduled_block_ticks().is_empty());
    assert_eq!(east.get_block(0, 64, 1), Some(BlockStateId(3)));
    assert_eq!(east.get_block(0, 65, 1), Some(BlockStateId(4)));
    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(button), Some(BlockStateId(1)));
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
}

#[tokio::test]
async fn aborted_cross_region_scheduled_task_finishes_reserved_transaction() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let west_chunk = ChunkPos { x: 7, z: 0 };
    let east_chunk = ChunkPos { x: 8, z: 0 };
    for chunk in [west_chunk, east_chunk] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let east = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(west, BlockStateId(1)).unwrap();
    storage.set_block_at(east, BlockStateId(1)).unwrap();
    let due = ScheduledBlockTick::new(west, Identifier::parse("minecraft:stone").unwrap(), 20, 0);
    storage.schedule_block_tick(due.clone()).unwrap();
    let west_token = storage.block_mutation_token(west).unwrap();
    let east_token = storage.block_mutation_token(east).unwrap();
    let mutation = storage.mutation_view();
    let read = storage.read_view();

    let sessions = Arc::new(SessionRegistry::new());
    let (requests, receiver) = std::sync::mpsc::sync_channel(4);
    let (append_started_tx, append_started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (appended_tx, appended_rx) = tokio::sync::oneshot::channel();
    let worker = std::thread::spawn(move || {
        let super::world_journal::WriterRequest::Replace { reply, .. } = receiver.recv().unwrap()
        else {
            panic!("expected reservation write");
        };
        reply.send(Ok(())).unwrap();
        let super::world_journal::WriterRequest::Append { reply, .. } = receiver.recv().unwrap()
        else {
            panic!("expected decision append");
        };
        append_started_tx.send(()).unwrap();
        release_rx.blocking_recv().unwrap();
        reply.send(Ok(())).unwrap();
        appended_tx.send(()).unwrap();
        let super::world_journal::WriterRequest::Shutdown { reply } = receiver.recv().unwrap()
        else {
            panic!("expected journal shutdown");
        };
        reply.send(()).unwrap();
    });
    let journal = super::world_journal::WorldChunkJournal::from_parts_for_test(
        std::path::PathBuf::from("abort-cross-region-journal"),
        Arc::clone(&blocks),
        Arc::new(mc_data::items::solaris_required_items()),
        requests,
        worker,
    );
    sessions.install_world_chunk_journal(journal.clone());

    let task_sessions = Arc::clone(&sessions);
    let task_mutation = mutation.clone();
    let task = tokio::spawn(async move {
        let edits = [
            mc_world::ResidentBlockEdit {
                pos: west,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
            mc_world::ResidentBlockEdit {
                pos: east,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
        ];
        let preconditions = [
            mc_world::ResidentBlockPrecondition {
                pos: west,
                expected_state: BlockStateId(1),
                expected_token: west_token,
            },
            mc_world::ResidentBlockPrecondition {
                pos: east,
                expected_state: BlockStateId(1),
                expected_token: east_token,
            },
        ];
        commit_cross_region_scheduled_block_tick(
            &task_sessions,
            &task_mutation,
            20,
            ResidentBlockCommit {
                edits: &edits,
                preconditions: &preconditions,
                consumed_block_ticks: std::slice::from_ref(&due),
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        )
        .await
    });
    append_started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();
    appended_rx.await.unwrap();

    assert_eq!(mutation.schedule_fluid_ticks(&[]), 0);
    assert_eq!(read.get_cached_block(west), Some(BlockStateId(0)));
    assert_eq!(read.get_cached_block(east), Some(BlockStateId(0)));
    assert_eq!(journal.watermark(), Some(1));
    assert_eq!(storage.plan_dirty_flush().unwrap().chunk_count(), 2);
}

#[tokio::test]
async fn known_cross_region_append_failure_closes_reserved_decision_empty() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let west_chunk = ChunkPos { x: 7, z: 0 };
    let east_chunk = ChunkPos { x: 8, z: 0 };
    for chunk in [west_chunk, east_chunk] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west = mc_world::BlockPos {
        x: 127,
        y: 64,
        z: 1,
    };
    let east = mc_world::BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(west, BlockStateId(1)).unwrap();
    storage.set_block_at(east, BlockStateId(1)).unwrap();
    let due = ScheduledBlockTick::new(west, Identifier::parse("minecraft:stone").unwrap(), 20, 0);
    storage.schedule_block_tick(due.clone()).unwrap();
    let west_token = storage.block_mutation_token(west).unwrap();
    let east_token = storage.block_mutation_token(east).unwrap();
    let mutation = storage.mutation_view();
    let read = storage.read_view();

    let temp = tempfile::tempdir().unwrap();
    let journal_blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&journal_blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    let sessions = Arc::new(SessionRegistry::new());
    let failure = sessions.subscribe_world_chunk_journal_failure();
    sessions.install_world_chunk_journal(journal.clone());

    let edits = [
        mc_world::ResidentBlockEdit {
            pos: west,
            new_state: BlockStateId(0),
            preserve_light: true,
        },
        mc_world::ResidentBlockEdit {
            pos: east,
            new_state: BlockStateId(0),
            preserve_light: true,
        },
    ];
    let preconditions = [
        mc_world::ResidentBlockPrecondition {
            pos: west,
            expected_state: BlockStateId(1),
            expected_token: west_token,
        },
        mc_world::ResidentBlockPrecondition {
            pos: east,
            expected_state: BlockStateId(1),
            expected_token: east_token,
        },
    ];
    let outcome = commit_cross_region_scheduled_block_tick(
        &sessions,
        &mutation,
        20,
        ResidentBlockCommit {
            edits: &edits,
            preconditions: &preconditions,
            consumed_block_ticks: std::slice::from_ref(&due),
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("known append failure closes its reservation")
    .expect("known append failure is a rejected resident transaction");

    assert!(outcome.applied.is_empty());
    assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
    assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
    assert_eq!(
        storage.scheduled_block_ticks(west_chunk).unwrap().unwrap(),
        std::slice::from_ref(&due)
    );
    assert_eq!(journal.watermark(), Some(1));
    assert!(!*failure.borrow());
    drop(sessions);
    drop(journal);

    let (reopened, pending) =
        super::world_journal::WorldChunkJournal::open(temp.path(), journal_blocks, items).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(reopened.decode_pending(&pending).unwrap().is_empty());
    let next_decision_id = reopened.reserve_decision_ids(1).unwrap()[0];
    assert_eq!(next_decision_id, pending[0].id() + 1);
    reopened
        .record_reserved_snapshot_groups(21, vec![(next_decision_id, Vec::new())])
        .unwrap();
    assert_eq!(reopened.watermark(), Some(next_decision_id));
}

#[tokio::test]
async fn scheduled_button_regions_commit_without_the_global_world_writer() {
    let blocks = Arc::new(button_and_door_test_registry());
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    for chunk in [
        ChunkPos { x: -1, z: 0 },
        ChunkPos { x: 7, z: 0 },
        ChunkPos { x: 8, z: 0 },
        ChunkPos { x: 16, z: 0 },
    ] {
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }
    let west_button = mc_world::BlockPos {
        x: -16 + 1,
        y: 64,
        z: 1,
    };
    let boundary_button = mc_world::BlockPos {
        x: 8 * 16 - 1,
        y: 64,
        z: 1,
    };
    let lower_door = mc_world::BlockPos {
        x: 8 * 16,
        y: 64,
        z: 1,
    };
    let upper_door = mc_world::BlockPos {
        y: 65,
        ..lower_door
    };
    let east_button = mc_world::BlockPos {
        x: 16 * 16 + 1,
        y: 64,
        z: 1,
    };
    for position in [west_button, boundary_button, east_button] {
        storage.set_block_at(position, BlockStateId(2)).unwrap();
        storage
            .schedule_block_tick(ScheduledBlockTick::new(
                position,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .unwrap();
    }
    storage.set_block_at(lower_door, BlockStateId(5)).unwrap();
    storage.set_block_at(upper_door, BlockStateId(6)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::from_u128(121),
        name: "RegionalBarrierOrder".to_string(),
    };
    let (tx, _rx) = mpsc::channel(16);
    let loaded = HashSet::from([(-1, 0), (7, 0), (8, 0), (16, 0)]);
    let (session, _) = sessions.register(
        &profile,
        (-1, 0),
        16,
        loaded.clone(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    for chunk in loaded {
        let _ = sessions.mark_loaded(session, chunk);
    }
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let resources = ChunkPipelineResources::with_limits(1, 2);
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            120,
            3,
        ),
    )
    .await
    .expect("mixed single-region and cross-region wave completes without the world writer");
    drop(world_writer);
    assert!(
        !*sessions.subscribe_world_chunk_journal_failure().borrow(),
        "mixed regional wave must not fail-stop its world journal"
    );
    assert_eq!(report.drained, 3);
    assert_eq!(report.applied, 5);
    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(west_button), Some(BlockStateId(1)));
    assert_eq!(
        storage.get_cached_block(boundary_button),
        Some(BlockStateId(1))
    );
    assert_eq!(storage.get_cached_block(lower_door), Some(BlockStateId(3)));
    assert_eq!(storage.get_cached_block(upper_door), Some(BlockStateId(4)));
    assert_eq!(storage.get_cached_block(east_button), Some(BlockStateId(1)));
}

#[tokio::test]
async fn resident_scheduled_button_tick_updates_without_world_writer_or_journal_wait() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                Identifier::parse("minecraft:stone_button").unwrap(),
                120,
                0,
            ))
            .expect("schedule button release");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    register_loaded_button_session(&sessions, "DurableButtonRelease");
    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    drop(storage);
    let (_simulation, owner) = simulation_channel();
    let world_writer = world.lock().await;
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        owner.run_scheduled_block_ticks_with_budget(
            &config,
            &sessions,
            SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                ..SimulationWorldAccess::default()
            },
            120,
            1,
        ),
    )
    .await
    .expect("resident button tick completion event");
    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 1);

    drop(world_writer);
    let storage = world.lock().await;
    assert_eq!(
        storage.get_cached_block(pos),
        Some(mc_world::BlockStateId(1))
    );
    assert!(
        storage
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .scheduled_block_ticks()
            .is_empty()
    );
}

#[tokio::test]
async fn stale_resident_journal_commit_does_not_block_the_next_decision() {
    let blocks = Arc::new(button_test_registry());
    let world = Arc::new(tokio::sync::Mutex::new(in_memory_button_world(Arc::clone(
        &blocks,
    ))));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .set_block_at(pos, mc_world::BlockStateId(2))
            .expect("place powered button");
    }
    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };
    let sessions = SessionRegistry::new();
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let (journal, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let storage = world.lock().await;
    let world_read = storage.read_view();
    let world_mutation = storage.mutation_view();
    let token = storage
        .block_mutation_token(pos)
        .expect("button mutation token");
    drop(storage);
    let edit = mc_world::ResidentBlockEdit {
        pos,
        new_state: mc_world::BlockStateId(1),
        preserve_light: true,
    };
    let stale = mc_world::ResidentBlockPrecondition {
        pos,
        expected_state: mc_world::BlockStateId(1),
        expected_token: token,
    };
    let current = mc_world::ResidentBlockPrecondition {
        pos,
        expected_state: mc_world::BlockStateId(2),
        expected_token: token,
    };

    let first = commit_resident_block_edits(
        &sessions,
        &world_read,
        &world_mutation,
        120,
        ResidentBlockCommit {
            edits: std::slice::from_ref(&edit),
            preconditions: std::slice::from_ref(&stale),
            consumed_block_ticks: &[],
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("stale resident decision is a normal rejection")
    .expect("same-region stale decision has an outcome");
    assert!(first.applied.is_empty());

    let second = commit_resident_block_edits(
        &sessions,
        &world_read,
        &world_mutation,
        121,
        ResidentBlockCommit {
            edits: std::slice::from_ref(&edit),
            preconditions: std::slice::from_ref(&current),
            consumed_block_ticks: &[],
            consumed_fluid_ticks: &[],
            scheduled_fluid_ticks: &[],
            light_table: None,
            leaf_trigger_tick: None,
        },
    )
    .await
    .expect("decision after stale reservation must remain writable")
    .expect("same-region current decision has an outcome");
    assert_eq!(second.applied.len(), 1);

    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].get_block(1, pos.y, 1),
        Some(mc_world::BlockStateId(1))
    );
}
