use super::*;

#[test]
fn pending_campfire_output_round_trips_full_stack_and_identity() {
    let items = mc_data::items::solaris_required_items();
    let cooked_porkchop = items
        .id_of(&Identifier::parse("minecraft:cooked_porkchop").unwrap())
        .expect("required cooked porkchop");
    let sharpness = Identifier::parse("minecraft:sharpness").unwrap();
    let position = mc_world::BlockPos { x: -7, y: 64, z: 9 };
    let output = PendingCampfireOutput {
        uuid: campfire_output_uuid(41, position, 2),
        stack: EntityItemStack::new(cooked_porkchop, 2)
            .with_damage(5)
            .with_enchantment(sharpness, 3),
    };
    let cooking = CampfireCookingState {
        pending_outputs: vec![output.clone()],
        ..CampfireCookingState::default()
    };

    let bytes =
        campfire_block_entity_persistent_bytes("minecraft:campfire", position, &items, &cooking)
            .expect("encode pending output");
    let decoded = campfire_cooking_state_from_persistent_nbt_strict(
        &bytes,
        &[],
        &items,
        &TagsData::default(),
    )
    .expect("decode pending output")
    .expect("pending-only campfire is retained");

    assert_eq!(decoded.pending_outputs, vec![output]);
}

#[test]
fn pending_campfire_output_uuid_is_stable_and_slot_specific() {
    let position = mc_world::BlockPos { x: 1, y: 70, z: -3 };

    assert_eq!(
        campfire_output_uuid(7, position, 0),
        campfire_output_uuid(7, position, 0)
    );
    assert_ne!(
        campfire_output_uuid(7, position, 0),
        campfire_output_uuid(7, position, 1)
    );
    assert_ne!(
        campfire_output_uuid(7, position, 0),
        campfire_output_uuid(8, position, 0)
    );
}

#[tokio::test]
async fn campfire_completion_persists_intent_before_entity_materialization() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = create_campfire_runtime(tmp.path());
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    runtime
        .sessions
        .install_campfire_d1_probe_for_test(reached_tx, resume_rx);
    let live_sessions = Arc::clone(&runtime.sessions);
    let task = tokio::spawn({
        let config = Arc::clone(&runtime.config);
        let sessions = Arc::clone(&runtime.sessions);
        async move {
            runtime
                .owner
                .run_campfire_cooking_ticks(&config, &sessions, None, None)
                .await
        }
    });

    tokio::time::timeout(Duration::from_secs(3), reached_rx)
        .await
        .expect("D1 gate was not reached")
        .expect("D1 gate sender dropped");
    let output = pending_output_from_live_world_journal(
        live_sessions.as_ref(),
        mc_world::BlockPos { x: 1, y: 64, z: 1 },
    );
    let (_, entity_pending) = persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert!(entity_pending.is_empty());
    assert_eq!(output.stack.count, 1);

    resume_tx.send(()).unwrap();
    let report = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("campfire tick did not finish after D1 release")
        .unwrap();
    assert_eq!(report.dropped, 1);
}

#[tokio::test]
async fn restart_materializes_pending_campfire_output_once() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = create_campfire_runtime(tmp.path());
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (_resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    runtime
        .sessions
        .install_campfire_d1_probe_for_test(reached_tx, resume_rx);
    abort_runtime_at_gate(runtime, reached_rx).await;
    let expected =
        pending_output_from_world_journal(tmp.path(), mc_world::BlockPos { x: 1, y: 64, z: 1 });

    let (first_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
    assert_eq!(recovered, 1);
    assert_one_output_and_no_intent(&first_restart, &expected).await;
    drop(first_restart);

    let (second_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
    assert_eq!(recovered, 0);
    assert_one_output_and_no_intent(&second_restart, &expected).await;
}

#[tokio::test]
async fn restart_after_entity_commit_before_world_ack_deduplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = create_campfire_runtime(tmp.path());
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (_resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    runtime
        .sessions
        .install_campfire_entity_probe_for_test(reached_tx, resume_rx);
    abort_runtime_at_gate(runtime, reached_rx).await;
    let expected =
        pending_output_from_world_journal(tmp.path(), mc_world::BlockPos { x: 1, y: 64, z: 1 });
    let (_, entity_pending) = persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    let replayed = persistence::replay_regional_commit_decisions(
        PersistedEntityCheckpoint::new(0, Vec::<PersistedEntityRecord>::new()),
        &entity_pending,
    )
    .expect("committed campfire entity journal replays");
    assert_eq!(replayed.records.len(), 1);
    assert_eq!(replayed.records[0].snapshot.uuid, expected.uuid);

    let (first_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
    assert_eq!(recovered, 1);
    assert_one_output_and_no_intent(&first_restart, &expected).await;
    drop(first_restart);

    let (second_restart, recovered) = reopen_campfire_runtime(tmp.path()).await;
    assert_eq!(recovered, 0);
    assert_one_output_and_no_intent(&second_restart, &expected).await;
}

#[tokio::test]
async fn successful_d2_checkpoint_does_not_resurrect_campfire_output() {
    let tmp = tempfile::tempdir().unwrap();
    let mut runtime = create_campfire_runtime(tmp.path());

    let report = runtime
        .owner
        .run_campfire_cooking_ticks(&runtime.config, &runtime.sessions, None, None)
        .await;
    assert_eq!(report.dropped, 1);
    let records = runtime.sessions.persisted_entity_save_snapshot().0.records;
    assert_eq!(records.len(), 1);
    let expected = PendingCampfireOutput {
        uuid: records[0].snapshot.uuid,
        stack: records[0]
            .snapshot
            .item_stack
            .clone()
            .expect("campfire output item stack"),
    };

    let journal = runtime.sessions.world_chunk_journal().unwrap();
    let world_pending = journal.pending_decisions_for_test();
    assert_eq!(world_pending.len(), 2, "D1 and D2 must precede checkpoint");
    drop(world_pending);
    drop(journal);
    let (_, entity_pending) = persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert_eq!(entity_pending.len(), 1, "E must precede checkpoint");

    checkpoint_campfire_runtime(&mut runtime).await;

    let journal = runtime.sessions.world_chunk_journal().unwrap();
    let world_pending = journal.pending_decisions_for_test();
    assert!(world_pending.is_empty());
    drop(world_pending);
    drop(journal);
    let (_, entity_pending) = persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert_eq!(
        entity_pending.len(),
        1,
        "entity checkpoint cleanup stays memory-only"
    );
    let checkpoint = persistence::load_persisted_entities(
        tmp.path(),
        runtime.config.items.as_ref(),
        runtime.config.entity_types.as_ref(),
    )
    .unwrap();
    let replayed = persistence::replay_regional_commit_decisions(checkpoint, &entity_pending)
        .expect("entity checkpoint watermark filters old campfire output");
    assert_eq!(replayed.records.len(), 1);
    assert_eq!(replayed.records[0].snapshot.uuid, expected.uuid);
    drop(runtime);

    let (_, entity_pending) = persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
    assert!(
        entity_pending.is_empty(),
        "normal shutdown compacts checkpointed entity WAL"
    );

    let (restarted, recovered) = reopen_campfire_runtime(tmp.path()).await;
    assert_eq!(recovered, 0);
    assert_one_output_and_no_intent(&restarted, &expected).await;
}
