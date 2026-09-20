use super::super::*;
use super::support::*;
use std::future::Future;
use std::sync::atomic::AtomicUsize;
use tokio::sync::mpsc;

#[tokio::test]
async fn startup_dirty_flush_detects_dirty_disk_world_and_skips_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);

    assert_eq!(startup_dirty_flush_dirty_count(&config).await, None);

    {
        let world = config.world.as_ref().unwrap();
        let mut storage = world.lock().await;
        let cpos = mc_world::ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(
                    cpos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_block_at(
                mc_world::BlockPos { x: 1, y: 64, z: 1 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
    }

    assert_eq!(startup_dirty_flush_dirty_count(&config).await, Some(1));

    config.shutdown.request();
    assert_eq!(startup_dirty_flush_dirty_count(&config).await, None);
}

#[test]
fn fenced_dirty_report_waits_for_the_exact_producer_wake() {
    assert_eq!(
        log_dirty_only_flush(
            "fenced dirty report test",
            Ok(DirtyOnlyFlushReport {
                planned_chunks: 0,
                flushed_chunks: 0,
                remaining_dirty: 1,
                immediately_flushable: false,
            }),
        ),
        crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer
    );
}

#[test]
fn dirty_flush_failure_is_not_reported_as_complete() {
    assert_eq!(
        log_dirty_only_flush("dirty failure report test", Err("disk full".to_owned())),
        crate::dirty_flush::DirtyFlushCompletion::Failed
    );
}

#[tokio::test]
async fn journal_fenced_dirty_flush_runs_once_and_allows_drain() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let world = Arc::new(Mutex::new(
        WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 1)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let mut config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        items,
        canonical_entity_types(),
    );
    config.world = Some(Arc::clone(&world));
    let config = Arc::new(config);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(
                    position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert!(matches!(
            storage.stamp_cached_chunks_for_world_journal(7, &[position]),
            mc_world::JournalStampResult::Stamped(_)
        ));
    }
    let dirty_calls = Arc::new(AtomicUsize::new(0));
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        {
            let config = Arc::clone(&config);
            let dirty_calls = Arc::clone(&dirty_calls);
            move || {
                let config = Arc::clone(&config);
                let dirty_calls = Arc::clone(&dirty_calls);
                async move {
                    dirty_calls.fetch_add(1, Ordering::SeqCst);
                    log_dirty_only_flush(
                        "journal-fenced drain test",
                        flush_dirty_chunks_only(&config, 0).await,
                    )
                }
            }
        },
        || async { panic!("journal fence must not request a full checkpoint") },
    );

    coordinator.notifier().request_dirty_flush();
    coordinator.drain().await;

    assert_eq!(dirty_calls.load(Ordering::SeqCst), 1);
    assert_eq!(world.lock().await.dirty_count(), 1);
}

#[tokio::test]
async fn exact_journal_fence_release_wakes_and_flushes_dirty_chunk() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let world = Arc::new(Mutex::new(
        WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 1)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let mut config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        items,
        canonical_entity_types(),
    );
    config.world = Some(Arc::clone(&world));
    let config = Arc::new(config);
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mutation = {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(
                    position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert!(matches!(
            storage.stamp_cached_chunks_for_world_journal(8, &[position]),
            mc_world::JournalStampResult::Stamped(_)
        ));
        storage.mutation_view()
    };
    let (completed, mut completed_rx) = mpsc::channel(2);
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        {
            let config = Arc::clone(&config);
            move || {
                let config = Arc::clone(&config);
                let completed = completed.clone();
                async move {
                    let result = log_dirty_only_flush(
                        "journal-fence release test",
                        flush_dirty_chunks_only(&config, 0).await,
                    );
                    completed.send(result).await.expect("test observes flush");
                    result
                }
            }
        },
        || async { panic!("journal fence must not request a full checkpoint") },
    );
    let notifier = coordinator.notifier();
    world
        .lock()
        .await
        .set_dirty_high_water_notifier(Arc::new(move || {
            notifier.request_dirty_flush();
        }));

    coordinator.notifier().request_dirty_flush();
    assert_eq!(
        completed_rx.recv().await,
        Some(crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer)
    );
    assert_eq!(
        mutation.clear_journal_pending_conditionally(8, &[position]),
        1
    );
    assert_eq!(
        completed_rx.recv().await,
        Some(crate::dirty_flush::DirtyFlushCompletion::Complete)
    );
    coordinator.drain().await;

    assert_eq!(world.lock().await.dirty_count(), 0);
}

#[tokio::test]
async fn saturated_dirty_mutation_retries_after_failed_pressure_flush() {
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(blocks, 1)));
    let dirty_calls = Arc::new(AtomicUsize::new(0));
    let (completed, mut completed_rx) = mpsc::channel(2);
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        {
            let dirty_calls = Arc::clone(&dirty_calls);
            move || {
                let call = dirty_calls.fetch_add(1, Ordering::SeqCst);
                let completed = completed.clone();
                async move {
                    completed.send(call).await.expect("test observes flush");
                    if call == 0 {
                        crate::dirty_flush::DirtyFlushCompletion::Failed
                    } else {
                        crate::dirty_flush::DirtyFlushCompletion::Complete
                    }
                }
            }
        },
        || async { panic!("dirty pressure must not request a full checkpoint") },
    );
    let notifier = coordinator.notifier();
    world
        .lock()
        .await
        .set_dirty_high_water_notifier(Arc::new(move || {
            notifier.request_dirty_flush();
        }));
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    world
        .lock()
        .await
        .insert_generated_chunk(
            position,
            mc_world::Chunk::empty(
                position,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(completed_rx.recv().await, Some(0));

    world
        .lock()
        .await
        .set_block_at(
            mc_world::BlockPos { x: 1, y: 64, z: 1 },
            mc_world::BlockStateId(1),
        )
        .unwrap();
    assert_eq!(completed_rx.recv().await, Some(1));
    coordinator.drain().await;

    assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bind_prepares_spawn_chunk_without_holding_world_lock() {
    struct PausedGenerator {
        entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl mc_world::chunk::ChunkGenerator for PausedGenerator {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                let _ = entered.send(());
            }
            self.release.lock().unwrap().recv().unwrap();
            let mut chunk = mc_world::Chunk::empty(
                pos,
                mc_world::BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.mark_dirty();
            chunk
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let generator = Arc::new(PausedGenerator {
        entered: std::sync::Mutex::new(Some(entered_tx)),
        release: std::sync::Mutex::new(release_rx),
    });
    let world = Arc::new(Mutex::new(
        WorldStorage::open(tmp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_generator(generator),
    ));
    let mut config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);
    config.world = Some(Arc::clone(&world));

    let bind_task = tokio::spawn(async move { bind(config).await });
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .expect("bind should start detached spawn generation")
        .expect("spawn generator should report entry");

    let world_available = match tokio::time::timeout(Duration::from_secs(1), world.lock()).await {
        Ok(storage) => {
            drop(storage);
            true
        }
        Err(_) => false,
    };
    release_tx.send(()).unwrap();
    let bound = bind_task.await.unwrap().unwrap();

    assert!(
        world_available,
        "spawn generation must not hold the shared world lock"
    );
    assert!(
        world
            .lock()
            .await
            .cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 })
            .is_some(),
        "bind should commit the prepared spawn chunk"
    );
    drop(bound);
}

#[tokio::test]
async fn bind_replays_and_acknowledges_pending_regional_entity_commit() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let entity_types = canonical_entity_types();
    let snapshot = mc_entity::EntitySnapshot {
        id: mc_entity::EntityId(1_000_001),
        uuid: uuid::Uuid::from_u128(71),
        type_id: 30,
        type_name: "minecraft:cow".into(),
        position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: mc_entity::Vec3::new(0.2, 0.0, 0.0),
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: mc_entity::EntityLifecycle::Alive,
        health: 14.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: mc_entity::GoalState::FollowPosition {
            target: mc_entity::Vec3::new(8.0, 64.0, -3.5),
            speed: 0.4,
        },
        vehicle: None,
        animal: Some(mc_entity::AnimalBreedingState::baby()),
        retained: mc_entity::EntityRetainedState::default(),
    };
    let decision = mc_entity::RegionalCommitDecision::from_parts(
        mc_entity::RegionPhase(1),
        19,
        vec![snapshot.clone()],
        Vec::new(),
    )
    .unwrap();
    let (mut journal, pending) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    assert!(pending.is_empty());
    mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
    drop(journal);

    let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
    let mut bound = bind(config)
        .await
        .expect("bind with pending owner decision");
    let restored = bound.sessions.persisted_entity_save_snapshot().0.records;
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].snapshot, snapshot);
    // Bind publishes recovery to RAM; fence the asynchronous writer before
    // inspecting its durable records through a separate reader.
    bound
        .sessions
        .world_chunk_journal()
        .unwrap()
        .writer
        .flush()
        .unwrap();
    let (_, pending) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0], decision);
    assert_eq!(pending[1].upserts(), std::slice::from_ref(&snapshot));
    assert!(pending[1].removed().is_empty());
    assert!(pending[1].phase() > decision.phase());
    assert!(pending[1].sequence_watermark() > decision.sequence_watermark());

    let report = {
        let mut save = std::pin::pin!(save_all_after_simulation_barrier(
            "recovered journal checkpoint test",
            &bound.config,
            &bound.sessions,
            &bound.simulation,
        ));
        let command_ready = tokio::select! {
            report = &mut save => panic!("save completed before owner snapshot: {report:?}"),
            ready = bound.simulation_owner.wait_for_command() => ready,
        };
        assert!(command_ready, "simulation command channel closed");
        assert_eq!(
            bound
                .simulation_owner
                .process_tick_with_world(
                    &bound.sessions,
                    bound.config.world.as_ref(),
                    bound.config.block_light.as_deref(),
                    1,
                )
                .processed,
            1
        );
        save.as_mut().await
    };
    assert!(report.is_ok(), "save failed: {:?}", report.errors);
    let (_, pending) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    assert_eq!(pending.len(), 2, "checkpoint cleanup stays memory-only");
    let checkpoint = play::persistence::load_persisted_entities(
        tmp.path(),
        bound.config.items.as_ref(),
        bound.config.entity_types.as_ref(),
    )
    .unwrap();
    let replayed = play::persistence::replay_regional_commit_decisions(checkpoint, &pending)
        .expect("saved owner snapshot filters checkpointed WAL records");
    assert_eq!(replayed.records.len(), 1);
    assert_eq!(replayed.records[0].snapshot, snapshot);

    drop(bound);
    let (_, pending) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    assert!(
        pending.is_empty(),
        "normal shutdown compacts checkpointed WAL"
    );
}

#[tokio::test]
async fn bind_keeps_recovered_entity_removal_until_full_snapshot_save() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let entity_types = canonical_entity_types();
    let snapshot = mc_entity::EntitySnapshot {
        id: mc_entity::EntityId(1_000_001),
        uuid: uuid::Uuid::from_u128(72),
        type_id: 30,
        type_name: "minecraft:cow".into(),
        position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: mc_entity::Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: mc_entity::EntityLifecycle::Alive,
        health: 14.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: mc_entity::GoalState::Idle,
        vehicle: None,
        animal: Some(mc_entity::AnimalBreedingState::adult()),
        retained: mc_entity::EntityRetainedState::default(),
    };
    play::persistence::save_persisted_entities(
        tmp.path(),
        items.as_ref(),
        std::slice::from_ref(&snapshot),
    )
    .unwrap();
    let decision = mc_entity::RegionalCommitDecision::from_parts(
        mc_entity::RegionPhase(1),
        19,
        Vec::new(),
        vec![snapshot.id],
    )
    .unwrap();
    let (mut journal, _) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
    drop(journal);

    let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
    let bound = bind(config)
        .await
        .expect("bind with pending entity removal");
    assert!(
        bound
            .sessions
            .persisted_entity_save_snapshot()
            .0
            .records
            .is_empty()
    );
    let (_, pending) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    assert_eq!(pending, vec![decision]);
}

#[tokio::test]
async fn bind_rejects_duplicate_final_entity_uuid_before_restore() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let entity_types = canonical_entity_types();
    let duplicate_uuid = uuid::Uuid::from_u128(73);
    let persisted = mc_entity::EntitySnapshot {
        id: mc_entity::EntityId(1_000_001),
        uuid: duplicate_uuid,
        type_id: 30,
        type_name: "minecraft:cow".into(),
        position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
        rotation: mc_entity::Rotation::ZERO,
        velocity: mc_entity::Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle: mc_entity::EntityLifecycle::Alive,
        health: 14.0,
        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
        goal: mc_entity::GoalState::Idle,
        vehicle: None,
        animal: Some(mc_entity::AnimalBreedingState::adult()),
        retained: mc_entity::EntityRetainedState::default(),
    };
    play::persistence::save_persisted_entities(
        tmp.path(),
        items.as_ref(),
        std::slice::from_ref(&persisted),
    )
    .unwrap();
    let duplicate = mc_entity::EntitySnapshot {
        id: mc_entity::EntityId(1_000_002),
        ..persisted
    };
    let decision = mc_entity::RegionalCommitDecision::from_parts(
        mc_entity::RegionPhase(1),
        19,
        vec![duplicate],
        Vec::new(),
    )
    .unwrap();
    let (mut journal, _) =
        play::persistence::FileRegionalDecisionJournal::open_for_test(tmp.path()).unwrap();
    mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
    drop(journal);
    let journal_path = tmp.path().join("solaris/entity-owner-journal.json");
    let journal_before_bind = std::fs::read(&journal_path).unwrap();

    let error = match bind(save_all_test_config(
        tmp.path(),
        blocks,
        items,
        entity_types,
    ))
    .await
    {
        Ok(_) => panic!("bind accepted duplicate final entity UUID"),
        Err(error) => error,
    };

    assert_eq!(error.kind(), ErrorKind::InvalidData);
    assert!(error.to_string().contains("duplicate restored entity UUID"));
    assert_eq!(std::fs::read(journal_path).unwrap(), journal_before_bind);
}

#[tokio::test]
async fn active_save_acquires_coordinator_before_owner_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let save_coordinator = config.shutdown.save_coordinator();
    let coordinator = save_coordinator.lock().await;
    let mut save = Box::pin(save_all_after_simulation_barrier(
        "save coordinator ordering test",
        &config,
        &sessions,
        &simulation,
    ));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(save.as_mut(), cx).is_pending(),
            "active save must wait for the occupied coordinator"
        );
        std::task::Poll::Ready(())
    })
    .await;

    assert_eq!(
        owner.process_tick(&sessions, 1).processed,
        0,
        "a queued save must not capture its owner snapshot before it owns the coordinator"
    );
    drop(save);
    drop(coordinator);
}

#[tokio::test]
async fn save_coordinator_does_not_serialize_unrelated_servers() {
    let first = ShutdownHandle::default();
    let second = ShutdownHandle::default();
    let first_coordinator = first.save_coordinator();
    let first_guard = first_coordinator.lock().await;
    let second_coordinator = second.save_coordinator();
    let mut second_guard = Box::pin(second_coordinator.lock());

    std::future::poll_fn(|context| match second_guard.as_mut().poll(context) {
        std::task::Poll::Ready(guard) => {
            drop(guard);
            std::task::Poll::Ready(())
        }
        std::task::Poll::Pending => {
            panic!("an unrelated server save coordinator must not wait")
        }
    })
    .await;
    drop(first_guard);
}

#[tokio::test]
async fn active_save_uses_the_ordered_owner_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 1,
        },
    ]));
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(
        tmp.path(),
        blocks,
        Arc::clone(&items),
        Arc::clone(&entity_types),
    );
    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let mut save = std::pin::pin!(save_all_after_simulation_barrier(
        "ordered save test",
        &config,
        &sessions,
        &simulation,
    ));
    let command_ready = tokio::select! {
        report = &mut save => panic!("active save completed before owner snapshot: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready, "simulation command channel closed");

    assert_eq!(
        owner
            .process_tick_with_world(
                &sessions,
                config.world.as_ref(),
                config.block_light.as_deref(),
                1,
            )
            .processed,
        1
    );
    sessions.restore_persisted_entities(play::persistence::PersistedEntityCheckpoint::new(
        0,
        vec![play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_001),
                uuid: uuid::Uuid::from_u128(1),
                type_id: 71,
                type_name: "minecraft:item".into(),
                position: mc_entity::Vec3::new(0.5, 64.0, 0.5),
                rotation: mc_entity::Rotation::ZERO,
                velocity: mc_entity::Vec3::ZERO,
                on_ground: true,
                item_stack: Some(mc_entity::EntityItemStack::new(1, 1)),
                experience_value: None,
                block_state: None,
                lifecycle: mc_entity::EntityLifecycle::Alive,
                health: 20.0,
                attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                goal: mc_entity::GoalState::Idle,
                vehicle: None,
                animal: None,
                retained: mc_entity::EntityRetainedState::default(),
            },
            age: 0,
            pickup_delay: 0,
        }],
    ));
    let report = save.await;

    assert!(report.is_ok(), "save errors: {:?}", report.errors);
    assert_eq!(report.entities_saved, 0);
    assert_eq!(sessions.persisted_entity_records().len(), 1);
    let saved =
        play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();
    assert!(saved.records.is_empty());
}

#[tokio::test]
async fn active_save_world_flush_matches_the_owner_barrier() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);
    let world = config.world.as_ref().unwrap();
    let cpos = mc_world::ChunkPos { x: 0, z: 0 };
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(
                    cpos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_block_at(pos, mc_world::BlockStateId(1))
            .unwrap();
    }
    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let mut save = std::pin::pin!(save_all_after_simulation_barrier(
        "world barrier save test",
        &config,
        &sessions,
        &simulation,
    ));
    let command_ready = tokio::select! {
        report = &mut save => panic!("save completed before owner barrier: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready);
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
            .processed,
        1
    );

    world
        .lock()
        .await
        .set_block_at(pos, mc_world::BlockStateId(0))
        .unwrap();
    let save_report = save.await;
    assert!(save_report.is_ok(), "save errors: {:?}", save_report.errors);
    assert_eq!(save_report.chunks_flushed, 1);

    let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
    assert_eq!(
        reopened.get_block(pos).unwrap(),
        Some(mc_world::BlockStateId(1)),
        "disk state must match the world at the owner barrier"
    );
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(mc_world::BlockStateId(0)),
        "post-barrier mutation must remain live and dirty"
    );
}

#[tokio::test]
async fn active_save_waits_for_the_exact_world_journal_fence() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::default());
    let config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        items,
        canonical_entity_types(),
    );
    let world = config.world.as_ref().unwrap();
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let mutation = {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(
                    position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert!(matches!(
            storage.stamp_cached_chunks_for_world_journal(41, &[position]),
            mc_world::JournalStampResult::Stamped(_)
        ));
        storage.mutation_view()
    };
    let dirty_tail_progress = config.shutdown.clone();
    world
        .lock()
        .await
        .set_dirty_high_water_notifier(Arc::new(move || {
            dirty_tail_progress.mark_dirty_tail_progress();
        }));

    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let mut save = std::pin::pin!(save_all_after_simulation_barrier(
        "journal-fenced save test",
        &config,
        &sessions,
        &simulation,
    ));
    let command_ready = tokio::select! {
        report = &mut save => panic!("save completed before owner barrier: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready);
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
            .processed,
        1
    );

    assert_eq!(
        mutation.clear_journal_pending_conditionally(40, &[position]),
        0,
        "a different journal decision must not release the save"
    );
    std::future::poll_fn(|context| {
        assert!(
            std::future::Future::poll(save.as_mut(), context).is_pending(),
            "save acknowledged a journal-fenced dirty chunk"
        );
        std::task::Poll::Ready(())
    })
    .await;

    assert_eq!(
        mutation.clear_journal_pending_conditionally(41, &[position]),
        1
    );
    let command_ready = tokio::select! {
        report = &mut save => panic!("save completed before the replacement barrier: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(
        command_ready,
        "fence release must request a new owner barrier"
    );
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 2)
            .processed,
        1
    );
    let report = save.await;
    assert!(report.is_ok(), "save errors: {:?}", report.errors);
    assert_eq!(report.chunks_flushed, 1);
    assert_eq!(world.lock().await.dirty_count(), 0);
}

#[tokio::test]
async fn final_save_rejects_an_orphaned_world_journal_fence() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let config = save_all_test_config(
        tmp.path(),
        blocks,
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    );
    let world = config.world.as_ref().unwrap();
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    {
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(
                    position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert!(matches!(
            storage.stamp_cached_chunks_for_world_journal(52, &[position]),
            mc_world::JournalStampResult::Stamped(_)
        ));
    }

    let sessions = play::SessionRegistry::new();
    let report =
        save_all_after_drain_with_context("orphaned journal fence test", &config, &sessions).await;

    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|error| { error.contains("journal-pending chunks after producer drain") })
    );
    assert_eq!(world.lock().await.dirty_count(), 1);
}

#[tokio::test]
async fn last_session_event_enqueues_periodic_checkpoint() {
    let sessions = play::SessionRegistry::new();
    let observed = sessions.session_empty_generation();
    let (flush_started, mut flush_started_receiver) = mpsc::channel(1);
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        || async { panic!("disconnect must not request the dirty-only action") },
        move || {
            let flush_started = flush_started.clone();
            async move {
                flush_started
                    .send(())
                    .await
                    .expect("test observes full checkpoint request");
            }
        },
    );
    let save_requests = coordinator.notifier();
    let mut request = Box::pin(wait_for_session_empty_save_request(
        &sessions,
        observed,
        Some(&save_requests),
        41,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(request.as_mut(), cx).is_pending(),
            "checkpoint request must wait for the last session event"
        );
        std::task::Poll::Ready(())
    })
    .await;

    sessions.mark_session_empty_for_test();
    assert_eq!(request.await, observed + 1);
    assert_eq!(flush_started_receiver.recv().await, Some(()));
    coordinator.drain().await;
}

#[tokio::test]
async fn runtime_dirty_high_water_drains_tail_across_bounded_actions() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let world = Arc::new(Mutex::new(
        WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 65)
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    let mut config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        items,
        canonical_entity_types(),
    );
    config.world = Some(Arc::clone(&world));
    let config = Arc::new(config);
    let dirty_calls = Arc::new(AtomicUsize::new(0));
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
        {
            let config = Arc::clone(&config);
            let dirty_calls = Arc::clone(&dirty_calls);
            move || {
                let config = Arc::clone(&config);
                let dirty_calls = Arc::clone(&dirty_calls);
                async move {
                    dirty_calls.fetch_add(1, Ordering::SeqCst);
                    log_dirty_only_flush(
                        "runtime dirty-only flush test",
                        flush_dirty_chunks_only(&config, 41).await,
                    )
                }
            }
        },
        || async { panic!("dirty high water must not run a full checkpoint") },
    );
    let dirty_flush = coordinator.notifier();
    world
        .lock()
        .await
        .set_dirty_high_water_notifier(Arc::new(move || {
            dirty_flush.request_dirty_flush();
        }));
    let biome = Identifier::parse("minecraft:plains").unwrap();
    {
        let mut storage = world.lock().await;
        for x in 0..=DIRTY_ONLY_FLUSH_MAX_CHUNKS as i32 {
            let position = mc_world::ChunkPos { x, z: 0 };
            storage
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(position, mc_world::BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
    }

    coordinator.drain().await;

    assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
    assert_eq!(world.lock().await.stats().dirty_chunks, 0);
    assert!(
        play::persistence::load_world_metadata(tmp.path())
            .unwrap()
            .is_none(),
        "runtime dirty-only flush must exclude full-checkpoint metadata"
    );
}

#[tokio::test]
async fn periodic_checkpoint_persists_ordered_owner_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 1,
        },
    ]));
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
        Arc::clone(&entity_types),
    );
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut world = config.world.as_ref().unwrap().lock().await;
        let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                chunk_pos,
                mc_world::Chunk::empty(
                    chunk_pos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        world
            .set_block_at(position, mc_world::BlockStateId(1))
            .unwrap();
    }
    let sessions = play::SessionRegistry::new();
    sessions.set_world_time(73);
    let (simulation, mut owner) = play::simulation_channel();
    let mut retained = mc_entity::EntityRetainedState::default();
    retained.item_pickup_ready_tick = Some(13);
    assert_eq!(
        owner.restore_persisted_entities(
            &sessions,
            play::persistence::PersistedEntityCheckpoint::new(
                11,
                vec![play::persistence::PersistedEntityRecord {
                    snapshot: mc_entity::EntitySnapshot {
                        id: mc_entity::EntityId(1_000_003),
                        uuid: uuid::Uuid::from_u128(3),
                        type_id: 71,
                        type_name: "minecraft:item".into(),
                        position: mc_entity::Vec3::new(1.5, 64.0, 2.5),
                        rotation: mc_entity::Rotation::ZERO,
                        velocity: mc_entity::Vec3::ZERO,
                        on_ground: true,
                        item_stack: Some(mc_entity::EntityItemStack::new(1, 3)),
                        experience_value: None,
                        block_state: None,
                        lifecycle: mc_entity::EntityLifecycle::Alive,
                        health: 20.0,
                        attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                        goal: mc_entity::GoalState::Idle,
                        vehicle: None,
                        animal: None,
                        retained,
                    },
                    age: 11,
                    pickup_delay: 2,
                },]
            ),
        ),
        1
    );

    let shutdown = ShutdownHandle::default();
    let mut save = std::pin::pin!(save_periodic_checkpoint(
        &config,
        &sessions,
        &simulation,
        &shutdown,
    ));
    let command_ready = tokio::select! {
        report = &mut save => {
            panic!("periodic checkpoint completed before owner snapshot: {report:?}")
        }
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready, "simulation command channel closed");
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 9)
            .processed,
        1
    );

    let report = save.await.expect("checkpoint is not superseded");

    assert!(report.is_ok(), "checkpoint errors: {:?}", report.errors);
    assert_eq!(report.entities_saved, 1);
    assert_eq!(report.chunks_flushed, 1);
    assert!(report.world_metadata_saved);
    let saved = play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types)
        .unwrap()
        .records;
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[0].item_stack,
        Some(mc_entity::EntityItemStack::new(1, 3))
    );
    assert_eq!(saved[0].age, 11);
    assert_eq!(saved[0].pickup_delay, 2);
    let metadata = play::persistence::load_world_metadata(tmp.path())
        .unwrap()
        .unwrap();
    assert_eq!(metadata.world_time, 73);
    let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
    assert_eq!(
        reopened.get_block(position).unwrap(),
        Some(mc_world::BlockStateId(1))
    );
}

#[tokio::test]
async fn periodic_checkpoint_is_superseded_by_shutdown_before_owner_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let config = save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        items,
        canonical_entity_types(),
    );
    let shutdown = ShutdownHandle::default();
    let sessions = play::SessionRegistry::new();
    let (simulation, mut owner) = play::simulation_channel();
    let mut save = std::pin::pin!(save_periodic_checkpoint(
        &config,
        &sessions,
        &simulation,
        &shutdown,
    ));

    let command_ready = tokio::select! {
        report = &mut save => {
            panic!("periodic checkpoint completed before owner snapshot: {report:?}")
        }
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready, "simulation command channel closed");

    shutdown.request();
    assert!(save.await.is_none());
    owner.shutdown();
}

#[tokio::test]
async fn save_all_reports_zero_entities_when_entity_write_fails() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
    let sessions = play::SessionRegistry::new();
    sessions.restore_persisted_entities(play::persistence::PersistedEntityCheckpoint::new(
        0,
        vec![play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_004),
                uuid: uuid::Uuid::from_u128(4),
                type_id: 71,
                type_name: "minecraft:item".into(),
                position: mc_entity::Vec3::new(0.5, 64.0, 0.5),
                rotation: mc_entity::Rotation::ZERO,
                velocity: mc_entity::Vec3::ZERO,
                on_ground: true,
                item_stack: Some(mc_entity::EntityItemStack::new(99, 1)),
                experience_value: None,
                block_state: None,
                lifecycle: mc_entity::EntityLifecycle::Alive,
                health: 20.0,
                attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                goal: mc_entity::GoalState::Idle,
                vehicle: None,
                animal: None,
                retained: mc_entity::EntityRetainedState::default(),
            },
            age: 0,
            pickup_delay: 0,
        }],
    ));

    let report = save_all(&config, &sessions).await;

    assert!(!report.is_ok());
    assert_eq!(report.entities_saved, 0);
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("entities: save failed"))
    );
}

#[tokio::test]
async fn save_all_writes_entities_and_world_metadata_to_real_storage() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 1,
        },
    ]));
    let entity_types = canonical_entity_types();
    let sessions = play::SessionRegistry::new();
    sessions.set_world_time(42);
    let mut retained = mc_entity::EntityRetainedState::default();
    retained.item_pickup_ready_tick = Some(15);
    let checkpoint = play::persistence::PersistedEntityCheckpoint::new(
        12,
        vec![play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_001),
                uuid: uuid::Uuid::from_u128(1),
                type_id: 71,
                type_name: "minecraft:item".into(),
                position: mc_entity::Vec3::new(1.0, 2.0, 3.0),
                rotation: mc_entity::Rotation::ZERO,
                velocity: mc_entity::Vec3::ZERO,
                on_ground: true,
                item_stack: Some(mc_entity::EntityItemStack::new(1, 2)),
                experience_value: None,
                block_state: None,
                lifecycle: mc_entity::EntityLifecycle::Alive,
                health: 20.0,
                attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                goal: mc_entity::GoalState::Idle,
                vehicle: None,
                animal: None,
                retained,
            },
            age: 12,
            pickup_delay: 3,
        }],
    );
    assert_eq!(sessions.restore_persisted_entities(checkpoint), 1);
    let config = save_all_test_config(
        tmp.path(),
        blocks,
        Arc::clone(&items),
        Arc::clone(&entity_types),
    );

    let report = save_all(&config, &sessions).await;

    assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
    assert_eq!(report.entities_saved, 1);
    assert!(report.world_metadata_saved);
    let entities = play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types)
        .unwrap()
        .records;
    assert_eq!(entities.len(), 1);
    assert_eq!(
        entities[0].item_stack,
        Some(mc_entity::EntityItemStack::new(1, 2))
    );
    assert_eq!(entities[0].age, 12);
    assert_eq!(entities[0].pickup_delay, 3);
    let metadata = play::persistence::load_world_metadata(tmp.path())
        .unwrap()
        .unwrap();
    assert_eq!(metadata.world_time, 42);
}

#[tokio::test]
async fn save_all_then_bind_restores_world_time_and_item_entities() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 1,
        },
    ]));
    let entity_types = canonical_entity_types();
    let sessions = play::SessionRegistry::new();
    sessions.set_world_time(99);
    sessions.set_daylight_cycle_enabled(false);
    sessions.set_weather(play::WeatherKind::Thunder);
    sessions.tick_weather(25);
    sessions.set_players_sleeping_percentage(50);
    let mut retained = mc_entity::EntityRetainedState::default();
    retained.item_pickup_ready_tick = Some(12);
    let checkpoint = play::persistence::PersistedEntityCheckpoint::new(
        8,
        vec![play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_002),
                uuid: uuid::Uuid::from_u128(2),
                type_id: 71,
                type_name: "minecraft:item".into(),
                position: mc_entity::Vec3::new(4.0, 5.0, 6.0),
                rotation: mc_entity::Rotation::ZERO,
                velocity: mc_entity::Vec3::ZERO,
                on_ground: true,
                item_stack: Some(mc_entity::EntityItemStack::new(1, 5)),
                experience_value: None,
                block_state: None,
                lifecycle: mc_entity::EntityLifecycle::Alive,
                health: 20.0,
                attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                goal: mc_entity::GoalState::Idle,
                vehicle: None,
                animal: None,
                retained,
            },
            age: 8,
            pickup_delay: 4,
        }],
    );
    assert_eq!(sessions.restore_persisted_entities(checkpoint), 1);
    let save_config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
        Arc::clone(&entity_types),
    );

    let report = save_all(&save_config, &sessions).await;
    assert!(report.is_ok(), "save-all errors: {:?}", report.errors);

    let bound = bind(save_all_test_config(
        tmp.path(),
        blocks,
        items,
        entity_types,
    ))
    .await
    .unwrap();
    assert_eq!(bound.sessions.world_time(), 99);
    assert!(!bound.sessions.daylight_cycle_enabled());
    assert_eq!(bound.sessions.weather().kind(), play::WeatherKind::Thunder);
    assert!((bound.sessions.weather().rain_level() - 0.25).abs() < f32::EPSILON);
    assert!((bound.sessions.weather().thunder_level() - 0.25).abs() < f32::EPSILON);
    assert_eq!(bound.sessions.players_sleeping_percentage(), 50);
    let records = bound.sessions.persisted_entity_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, mc_entity::EntityId(1_000_002));
    assert_eq!(
        records[0].item_stack,
        Some(mc_entity::EntityItemStack::new(1, 5))
    );
    assert_eq!(records[0].age, 8);
    assert_eq!(records[0].pickup_delay, 4);
}

#[tokio::test]
async fn bind_replays_world_chunk_journal_and_clean_save_checkpoints_it() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_position,
        mc_world::BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk
        .set_block(1, 64, 1, mc_world::BlockStateId(1))
        .unwrap();
    chunk
        .extras
        .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));

    let (journal, pending) = play::world_journal::WorldChunkJournal::open_for_test(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        journal.record_snapshots(12, vec![Arc::new(chunk)]).unwrap(),
        1
    );
    drop(journal);

    let bound = bind(save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
        Arc::clone(&entity_types),
    ))
    .await
    .unwrap();
    assert_eq!(
        bound
            .config
            .world
            .as_ref()
            .unwrap()
            .lock()
            .await
            .get_cached_block(position),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(bound.sessions.world_chunk_journal_watermark(), Some(1));

    let report = save_all(&bound.config, &bound.sessions).await;
    assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
    assert_eq!(bound.sessions.world_chunk_journal_watermark(), None);
    drop(bound);

    let (_, pending) =
        play::world_journal::WorldChunkJournal::open_for_test(tmp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn bind_skips_pending_world_journal_image_at_disk_lsn() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
    let biome = Identifier::parse("minecraft:plains").unwrap();

    let mut image_a =
        mc_world::Chunk::empty(chunk_position, mc_world::BlockStateId(0), biome.clone());
    image_a
        .set_block(1, 64, 1, mc_world::BlockStateId(1))
        .unwrap();
    image_a
        .extras
        .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));
    let (journal, pending) = play::world_journal::WorldChunkJournal::open_for_test(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    assert_eq!(
        journal
            .record_snapshots(12, vec![Arc::new(image_a)])
            .unwrap(),
        1
    );
    drop(journal);

    let mut image_b = mc_world::Chunk::empty(chunk_position, mc_world::BlockStateId(0), biome);
    image_b
        .set_block(1, 64, 1, mc_world::BlockStateId(1))
        .unwrap();
    image_b
        .set_block(1, 64, 1, mc_world::BlockStateId(0))
        .unwrap();
    image_b
        .extras
        .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));
    image_b.mark_dirty();
    let mut storage = WorldStorage::open(tmp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    storage
        .commit_chunk_snapshot(chunk_position, image_b)
        .unwrap();
    assert_eq!(storage.flush_dirty().unwrap(), 1);
    drop(storage);

    let bound = bind(save_all_test_config(
        tmp.path(),
        blocks,
        items,
        entity_types,
    ))
    .await
    .unwrap();
    let storage = bound.config.world.as_ref().unwrap().lock().await;
    assert_eq!(
        storage.get_cached_block(position),
        Some(mc_world::BlockStateId(0)),
        "disk image B at the matching LSN must win over pending journal image A"
    );
    assert_eq!(
        storage
            .cached_chunk_snapshot(chunk_position)
            .unwrap()
            .world_journal_lsn(),
        1
    );
}

#[tokio::test]
async fn failed_world_flush_keeps_world_chunk_journal_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let region_root = tmp.path().join("region");
    std::fs::create_dir_all(&region_root).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
        entity_types,
    );
    let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
    let snapshot = {
        let mut storage = config.world.as_ref().unwrap().lock().await;
        storage
            .insert_generated_chunk(
                chunk_position,
                mc_world::Chunk::empty(
                    chunk_position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_block_at(
                mc_world::BlockPos { x: 1, y: 64, z: 1 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
        storage.cached_chunk_snapshot(chunk_position).unwrap()
    };
    let sessions = play::SessionRegistry::new();
    let (journal, pending) =
        play::world_journal::WorldChunkJournal::open_for_test(tmp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    assert_eq!(journal.record_snapshots(1, vec![snapshot]).unwrap(), 1);
    sessions.install_world_chunk_journal(journal);
    std::fs::remove_dir(&region_root).unwrap();
    std::fs::write(&region_root, b"blocks region writes").unwrap();

    let report = save_all(&config, &sessions).await;

    assert!(!report.is_ok());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("dirty chunks:")),
        "save errors: {:?}",
        report.errors
    );
    assert_eq!(sessions.world_chunk_journal_watermark(), Some(1));
}

#[tokio::test]
async fn bind_rejects_corrupt_world_metadata_without_overwriting_it() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let metadata_path = tmp.path().join("solaris/world.dat");
    std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
    let corrupt = b"not gzip nbt";
    std::fs::write(&metadata_path, corrupt).unwrap();

    let err = match bind(save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    ))
    .await
    {
        Ok(_) => panic!("bind accepted corrupt world metadata"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("world metadata load failed"));
    assert_eq!(std::fs::read(metadata_path).unwrap(), corrupt);
}

#[tokio::test]
async fn bind_rejects_corrupt_entities_without_overwriting_them() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let entities_path = tmp.path().join("solaris/entities.dat");
    std::fs::create_dir_all(entities_path.parent().unwrap()).unwrap();
    let corrupt = b"not gzip nbt";
    std::fs::write(&entities_path, corrupt).unwrap();

    let err = match bind(save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    ))
    .await
    {
        Ok(_) => panic!("bind accepted corrupt persisted entities"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("persisted entity load failed"));
    assert_eq!(std::fs::read(entities_path).unwrap(), corrupt);
}

#[tokio::test]
async fn bind_rejects_world_metadata_from_another_world() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    play::persistence::save_world_metadata(
        tmp.path(),
        &play::persistence::WorldPersistedMetadata {
            world_time: 77,
            daylight_cycle_enabled: true,
            weather: play::SessionRegistry::new().weather(),
            players_sleeping_percentage: 100,
            keep_inventory: false,
            world_identity: "different-world".into(),
        },
    )
    .unwrap();

    let err = match bind(save_all_test_config(
        tmp.path(),
        Arc::new(BlockRegistry::from_report(&[]).unwrap()),
        Arc::new(mc_data::items::ItemRegistry::default()),
        canonical_entity_types(),
    ))
    .await
    {
        Ok(_) => panic!("bind accepted metadata from another world"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("world metadata identity mismatch"));
}

#[tokio::test]
async fn save_all_persists_scheduled_fluid_ticks_as_remaining_restart_delay() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let blocks = Arc::new(
        BlockRegistry::from_report(&[
            report("minecraft:air", &[], &[(0, true, &[])]),
            report(
                "minecraft:water",
                &[("level", &["0", "1"])],
                &[(1, true, &[("level", "0")]), (2, false, &[("level", "1")])],
            ),
        ])
        .unwrap(),
    );
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
    let entity_types = canonical_entity_types();
    let sessions = play::SessionRegistry::new();
    sessions.advance_world_time(100);
    let config = save_all_test_config(
        tmp.path(),
        Arc::clone(&blocks),
        items,
        Arc::clone(&entity_types),
    );
    let cpos = mc_world::ChunkPos { x: 0, z: 0 };
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let water = Identifier::parse("minecraft:water").unwrap();
    {
        let world = config.world.as_ref().unwrap();
        let mut storage = world.lock().await;
        storage
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(
                    cpos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_block_at(pos, mc_world::BlockStateId(1))
            .unwrap();
        assert!(
            storage
                .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
                    pos,
                    water.clone(),
                    112,
                    0,
                ))
                .unwrap()
        );
    }

    let report = save_all(&config, &sessions).await;

    assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
    drop(config);

    let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
    let ticks = reopened.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].fluid, water);
    assert_eq!(
        ticks[0].trigger_tick, 12,
        "fresh runtimes must reload persisted fluid ticks as remaining delay"
    );
}
