use super::{
    BlockEdit, BlockEditPrecondition, BlockStateId, Chunk, ChunkPos, Identifier,
    ScheduledBlockTick, ServerConfig, SessionRegistry, in_memory_button_world,
    leaf_distance_test_registry, play_loop_slow_client_test_config, register_loaded_button_session,
    register_ticketed_button_session, run_scheduled_block_ticks, simulation_channel,
};
use std::sync::Arc;
use std::task::{Context, Poll};
#[tokio::test]
async fn removed_log_pushes_leaf_distance_updates_through_scheduled_ticks() {
    let blocks = leaf_distance_test_registry();
    let mut storage = in_memory_button_world(Arc::clone(&blocks));
    let log = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let first_leaf = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let second_leaf = mc_world::BlockPos { x: 3, y: 64, z: 1 };
    storage
        .set_block_at(log, mc_world::BlockStateId(1))
        .expect("place supporting log");
    storage
        .set_block_at(first_leaf, mc_world::BlockStateId(2))
        .expect("place first leaf");
    storage
        .set_block_at(second_leaf, mc_world::BlockStateId(2))
        .expect("place second leaf");
    let log_token = storage
        .block_mutation_token(log)
        .expect("supporting log has a mutation token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "LeafDistanceUpdates");
    let _ = sessions.mark_loaded(session, (0, 0));
    let (handle, mut owner) = simulation_channel();
    let session_handle = handle.for_session(session);
    let mut removal = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: log,
            new_state: mc_world::BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: log,
            expected_state: mc_world::BlockStateId(1),
            expected_token: log_token,
        }],
    ));
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(removal.as_mut(), &mut context),
        Poll::Pending
    ));
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert_eq!(
        removal
            .await
            .expect("simulation owner applies log removal")
            .expect("matching log precondition")
            .applied
            .len(),
        1
    );
    {
        let mut storage = world.lock().await;
        let first_tick = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("loaded chunk exposes leaf tick");
        assert_eq!(first_tick.len(), 1);
        assert_eq!(first_tick[0].pos, first_leaf);
        assert_eq!(first_tick[0].trigger_tick, 1);
    }

    let config = ServerConfig {
        world: Some(Arc::clone(&world)),
        blocks,
        ..play_loop_slow_client_test_config()
    };

    let first = run_scheduled_block_ticks(&config, &sessions, 1).await;
    assert_eq!(first.drained, 1);
    assert_eq!(first.applied, 1);
    {
        let mut storage = world.lock().await;
        assert_eq!(
            storage.get_cached_block(first_leaf),
            Some(mc_world::BlockStateId(3)),
            "the first leaf should move from distance 1 to 2"
        );
        let second_tick = storage
            .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .expect("loaded chunk exposes propagated leaf tick");
        assert!(
            second_tick
                .iter()
                .any(|tick| tick.pos == second_leaf && tick.trigger_tick == 2),
            "the changed first leaf must notify the second leaf"
        );
    }

    let second = run_scheduled_block_ticks(&config, &sessions, 2).await;
    assert_eq!(second.applied, 1);
    assert_eq!(
        world.lock().await.get_cached_block(second_leaf),
        Some(mc_world::BlockStateId(4)),
        "the second leaf should move from distance 1 to 3"
    );
}

#[tokio::test]
async fn stable_leaf_tick_is_checkpoint_only_without_world_journal_decision() {
    let blocks = leaf_distance_test_registry();
    let cpos = ChunkPos { x: 0, z: 0 };
    let leaf = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let log = mc_world::BlockPos { x: 2, y: 64, z: 1 };
    let mut storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(leaf, BlockStateId(2)).unwrap();
    storage.set_block_at(log, BlockStateId(1)).unwrap();
    storage
        .schedule_block_tick(ScheduledBlockTick::new(
            leaf,
            Identifier::parse("minecraft:oak_leaves").unwrap(),
            20,
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
    register_loaded_button_session(&sessions, "StableLeafNoop");
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

    let report = run_scheduled_block_ticks(&config, &sessions, 20).await;

    assert_eq!(report.drained, 1);
    assert_eq!(report.applied, 0);
    let mut storage = world.lock().await;
    let ticks = storage
        .scheduled_block_ticks(cpos)
        .unwrap()
        .expect("loaded chunk exposes scheduled ticks");
    assert!(
        ticks.is_empty(),
        "the no-op tick is consumed in resident state"
    );
    drop(storage);
    drop(sessions);
    let (_reopened, pending) = super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&config.blocks),
        Arc::clone(&config.items),
    )
    .unwrap();
    assert!(
        pending.is_empty(),
        "replaying a stable no-op leaf tick after a crash is harmless"
    );
}
