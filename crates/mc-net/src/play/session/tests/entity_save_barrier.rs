use super::*;
use crate::play::world_journal::WorldChunkJournal;

#[test]
fn entity_save_owner_barrier_does_not_hold_session_or_world() {
    let registry = Arc::new(SessionRegistry::new());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:cow".to_owned(),
        Vec3::new(0.5, 64.0, 0.5),
    );
    let blocks = Arc::new(
        BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }])
        .unwrap(),
    );
    let world = Arc::new(tokio::sync::Mutex::new(mc_world::WorldStorage::in_memory(
        Arc::clone(&blocks),
    )));
    let temp = tempfile::tempdir().unwrap();
    let items = Arc::new(mc_data::items::solaris_required_items());
    let (journal, _) =
        WorldChunkJournal::open_for_test(temp.path(), Arc::clone(&blocks), Arc::clone(&items))
            .unwrap();
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunk = |position| Chunk::empty(position, BlockStateId(0), biome.clone());
    journal
        .record_snapshots(0, vec![Arc::new(chunk(ChunkPos { x: 0, z: 0 }))])
        .unwrap();
    registry.install_world_chunk_journal(journal.clone());
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .entity_save_owner_probe
        .lock()
        .expect("test lock poisoned") = Some(EntityApplyReleaseProbe {
        reached: reached_tx,
        resume: resume_rx,
    });

    let save_registry = Arc::clone(&registry);
    let save_world = Arc::clone(&world);
    let save = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let (handle, mut owner) =
                    crate::play::simulation::simulation_channel_with_capacity(1);
                let request = tokio::spawn(async move { handle.save_barrier(true).await });
                assert!(owner.wait_for_command().await);
                assert_eq!(
                    owner
                        .process_commands_with_world(&save_registry, Some(&save_world), None, 1)
                        .await
                        .processed,
                    1
                );
                request.await.unwrap().expect("save barrier snapshot")
            })
    });
    reached_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("save reaches regional owner barrier");
    let session_available = registry.inner.try_lock().is_ok();
    let position = ChunkPos { x: 1, z: 0 };
    let neighbour_commit = world
        .try_lock()
        .ok()
        .map(|mut storage| storage.insert_generated_chunk(position, chunk(position)));
    let later_decision = journal.record_snapshots(0, vec![Arc::new(chunk(position))]);
    resume_tx.send(()).expect("release entity save barrier");
    let snapshot = save.join().expect("entity save snapshot worker");

    assert!(
        session_available,
        "regional save barrier must not retain session state"
    );
    neighbour_commit
        .expect("streaming must progress during entity save capture")
        .expect("neighbour chunk commit succeeds");
    assert_eq!(snapshot.entities.records.len(), 1);
    assert_eq!(snapshot.entities.records[0].type_name, "minecraft:cow");
    let later_decision = later_decision.expect("world write accepted during entity capture");
    journal
        .checkpoint_through(snapshot.world_chunk_journal_watermark.unwrap())
        .unwrap();
    drop(registry);
    drop(journal);
    let (_journal, pending) = WorldChunkJournal::open_for_test(temp.path(), blocks, items).unwrap();
    assert_eq!(
        pending
            .iter()
            .map(|decision| decision.id())
            .collect::<Vec<_>>(),
        vec![later_decision],
        "the save cut must retain world writes accepted after world capture"
    );
}
