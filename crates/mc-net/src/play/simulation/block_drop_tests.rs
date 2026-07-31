use super::*;
use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::ItemRegistry;
use mc_entity::{EntityItemStack, Vec3};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::mpsc;

fn block_report(id: &str, state_id: u32) -> BlockReport {
    BlockReport {
        id: Identifier::parse(id).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: state_id,
            default: true,
            properties: BTreeMap::new(),
        }],
    }
}

fn test_block_reports() -> Vec<BlockReport> {
    vec![
        block_report("minecraft:air", 0),
        block_report("minecraft:stone", 1),
    ]
}

fn test_block_storage() -> (WorldStorage, BlockPos, BlockMutationToken) {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let chunk = ChunkPos { x: 0, z: 0 };
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
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let token = storage.block_mutation_token(position).unwrap();
    (storage, position, token)
}

fn test_drop(position: Vec3) -> SurvivalBreakDrop {
    SurvivalBreakDrop {
        entity_type_id: 7,
        position,
        stack: EntityItemStack::new(42, 2),
    }
}

fn block_drop_command(
    position: BlockPos,
    token: BlockMutationToken,
    drops: Vec<SurvivalBreakDrop>,
) -> SimulationCommand {
    SimulationCommand::CommitBlockDrops {
        actor_session: 0,
        edits: vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        drops,
    }
}

fn persisted_item_drop_count(registry: &SessionRegistry) -> usize {
    registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.item_stack.is_some())
        .count()
}

fn register_observer(
    registry: &SessionRegistry,
    name: &str,
) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
    let profile = crate::login::LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (tx, rx) = mpsc::channel(32);
    let desired = HashSet::from([(0, 0)]);
    let session = registry
        .register(
            &profile,
            (0, 0),
            2,
            desired,
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0;
    dispatch_visibility_commands(registry.mark_loaded(session, (0, 0)));
    assert!(
        !registry
            .loaded_recipients_for_chunks(&HashSet::from([(0, 0)]), None)
            .is_empty()
    );
    (session, rx)
}

fn assert_no_block_or_entity_publication(outbound: &mut mpsc::Receiver<OutboundCommand>) {
    while let Ok(command) = outbound.try_recv() {
        assert!(
            !matches!(
                command,
                OutboundCommand::BlockDeltas(_) | OutboundCommand::SpawnEntity(_)
            ),
            "unexpected block/drop publication: {command:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_block_drop_completes_while_world_storage_writer_is_held() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_read_view = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    });

    let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
        .await
        .expect("resident block drop completion event");
    drop(writer);

    assert!(matches!(
        response.unwrap().unwrap(),
        SimulationResponse::BlockDrops(Ok(outcome)) if outcome.is_some()
    ));
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert_eq!(owner_task.await.unwrap().processed, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_without_resident_mutation_view_does_not_fall_back_to_storage() {
    let (storage, position, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world(&registry, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::WorldUnavailable)
    ));
    assert_eq!(
        world.lock().await.get_cached_block(position),
        Some(BlockStateId(1))
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_rejects_cross_region_input_without_storage_fallback() {
    let (mut storage, first, first_token) = test_block_storage();
    let second_chunk = ChunkPos { x: 8, z: 0 };
    storage
        .insert_generated_chunk(
            second_chunk,
            Chunk::empty(
                second_chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let second = BlockPos {
        x: 128,
        y: 64,
        z: 1,
    };
    storage.set_block_at(second, BlockStateId(1)).unwrap();
    let second_token = storage.block_mutation_token(second).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::CommitBlockDrops {
            actor_session: 0,
            edits: vec![
                BlockEdit {
                    pos: first,
                    new_state: BlockStateId(0),
                },
                BlockEdit {
                    pos: second,
                    new_state: BlockStateId(0),
                },
            ],
            preconditions: vec![
                BlockEditPrecondition {
                    pos: first,
                    expected_state: BlockStateId(1),
                    expected_token: first_token,
                },
                BlockEditPrecondition {
                    pos: second,
                    expected_state: BlockStateId(1),
                    expected_token: second_token,
                },
            ],
            drops: vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        })
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world(&registry, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::CrossRegion)
    ));
    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(first), Some(BlockStateId(1)));
    assert_eq!(storage.get_cached_block(second), Some(BlockStateId(1)));
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_rejects_drop_outside_owner_region_without_publication() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(128.5, 64.5, 0.5))],
        ))
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::CrossRegion)
    ));
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(1)));
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn journaled_resident_block_drop_append_failure_rejects_before_drop_publication() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk = ChunkPos { x: 0, z: 0 };
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
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let token = storage.block_mutation_token(position).unwrap();
    let following_position = BlockPos { x: 2, y: 64, z: 1 };
    storage
        .set_block_at(following_position, BlockStateId(1))
        .unwrap();
    let following_token = storage.block_mutation_token(following_position).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (_, mut outbound) = register_observer(&sessions, "JournalFailureObserver");
    let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
    let (journal, pending) = super::super::world_journal::WorldChunkJournal::open(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();
    let following_response = handle
        .enqueue(block_drop_command(
            following_position,
            following_token,
            vec![test_drop(Vec3::new(1.5, 64.5, 0.5))],
        ))
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);
    let owner_sessions = Arc::clone(&sessions);
    let owner_world = Arc::clone(&world);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                2,
            )
            .await
    });

    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    let journal_path = temp.path().join("solaris/world-chunk-journal.bin");
    std::fs::remove_file(&journal_path).unwrap();
    std::fs::create_dir(&journal_path).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::WorldMutationFailed)
    ));
    assert!(matches!(
        following_response.await.unwrap(),
        Err(SimulationRequestError::OwnerStopped)
    ));
    journal_failure.changed().await.unwrap();
    assert!(*journal_failure.borrow_and_update());
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert_eq!(
        read_view.get_cached_block(following_position),
        Some(BlockStateId(1))
    );
    assert_eq!(persisted_item_drop_count(&sessions), 0);
    assert_no_block_or_entity_publication(&mut outbound);
    assert!(world.lock().await.plan_dirty_flush().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_block_drop_stale_cas_returns_none_without_drop() {
    let (mut storage, position, stale_token) = test_block_storage();
    storage.set_block_at(position, BlockStateId(0)).unwrap();
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            stale_token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_read_view = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    });

    let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
        .await
        .expect("resident stale block-drop completion event");
    drop(writer);

    assert!(matches!(
        response.unwrap().unwrap(),
        SimulationResponse::BlockDrops(Ok(outcome)) if outcome.is_none()
    ));
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(1)));
    assert_eq!(persisted_item_drop_count(&registry), 0);
    assert_eq!(owner_task.await.unwrap().processed, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_resident_block_drop_never_waits_for_storage_or_mutates() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();
    drop(response);

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_read_view = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    });

    let report = tokio::time::timeout(std::time::Duration::from_secs(1), owner_task)
        .await
        .expect("cancelled resident block drop does not wait for storage")
        .unwrap();
    drop(writer);

    assert_eq!(report.processed, 0);
    assert_eq!(handle.snapshot().cancelled, 1);
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(1)));
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_rejects_empty_duplicate_and_malformed_commands_before_mutation() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(6);

    let empty = handle
        .enqueue(SimulationCommand::CommitBlockDrops {
            actor_session: 0,
            edits: Vec::new(),
            preconditions: Vec::new(),
            drops: Vec::new(),
        })
        .unwrap();
    let no_drops = handle
        .enqueue(block_drop_command(position, token, Vec::new()))
        .unwrap();
    let duplicate = handle
        .enqueue(SimulationCommand::CommitBlockDrops {
            actor_session: 0,
            edits: vec![
                BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                },
                BlockEdit {
                    pos: position,
                    new_state: BlockStateId(0),
                },
            ],
            preconditions: vec![
                BlockEditPrecondition {
                    pos: position,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                },
                BlockEditPrecondition {
                    pos: position,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                },
            ],
            drops: vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        })
        .unwrap();
    let malformed = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(f64::NAN, 64.5, 0.5))],
        ))
        .unwrap();
    let no_op = handle
        .enqueue(SimulationCommand::CommitBlockDrops {
            actor_session: 0,
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(1),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            drops: vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        })
        .unwrap();
    let oversized = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5)); MAX_SURVIVAL_BREAK_DROPS + 1],
        ))
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                6,
            )
            .await
            .processed,
        6
    );
    for response in [empty, no_drops, duplicate, malformed, no_op, oversized] {
        assert!(matches!(
            response.await.unwrap(),
            Err(SimulationRequestError::InvalidCommand)
        ));
    }
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(1)));
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_missing_resident_chunk_rejects_without_publication() {
    let (storage, _, _) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (_, mut outbound) = register_observer(&registry, "MissingResidentObserver");
    let missing = BlockPos { x: 17, y: 64, z: 1 };
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            missing,
            BlockMutationToken {
                chunk_instance_id: 1,
                version: 0,
            },
            vec![test_drop(Vec3::new(17.5, 64.5, 1.5))],
        ))
        .unwrap();

    owner
        .process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                ..SimulationWorldAccess::default()
            },
            None,
            1,
        )
        .await;

    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::WorldUnavailable)
    ));
    assert_eq!(persisted_item_drop_count(&registry), 0);
    assert_no_block_or_entity_publication(&mut outbound);
}

#[tokio::test(flavor = "current_thread")]
async fn resident_block_drop_spawns_exact_ordered_batch_and_then_publishes() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (_, mut outbound) = register_observer(&registry, "BatchDropObserver");
    let drops = vec![
        SurvivalBreakDrop {
            entity_type_id: 7,
            position: Vec3::new(1.25, 64.5, 1.25),
            stack: EntityItemStack::new(42, 2),
        },
        SurvivalBreakDrop {
            entity_type_id: 9,
            position: Vec3::new(1.75, 64.75, 1.75),
            stack: EntityItemStack::new(43, 3),
        },
        SurvivalBreakDrop {
            entity_type_id: 11,
            position: Vec3::new(1.5, 65.0, 1.5),
            stack: EntityItemStack::new(44, 4),
        },
    ];
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(position, token, drops.clone()))
        .unwrap();

    owner
        .process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                ..SimulationWorldAccess::default()
            },
            None,
            1,
        )
        .await;
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BlockDrops(Ok(outcome)) if outcome.is_some()
    ));

    let mut snapshots = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot)
        .filter(|snapshot| snapshot.item_stack.is_some())
        .collect::<Vec<_>>();
    snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
    assert_eq!(snapshots.len(), drops.len());
    for (snapshot, expected) in snapshots.iter().zip(&drops) {
        assert_eq!(snapshot.type_id, expected.entity_type_id);
        assert_eq!(snapshot.position, expected.position);
        assert_eq!(snapshot.item_stack.as_ref(), Some(&expected.stack));
    }

    let mut published = Vec::new();
    while let Ok(command) = outbound.try_recv() {
        if let OutboundCommand::SpawnEntity(snapshot) = command {
            published.push((snapshot.type_id, snapshot.position, snapshot.item_stack));
        }
    }
    assert_eq!(published.len(), drops.len());
    for (published, expected) in published.iter().zip(&drops) {
        assert_eq!(published.0, expected.entity_type_id);
        assert_eq!(published.1, expected.position);
        assert_eq!(published.2.as_ref(), Some(&expected.stack));
    }
}

#[test]
fn item_drop_batch_preflights_every_entry_before_authoritative_spawn() {
    let registry = SessionRegistry::new();
    let result = registry.try_spawn_item_drop_batch_owned(
        &SimulationAuthority::for_test(),
        [
            (7, Vec3::new(0.5, 64.5, 0.5), EntityItemStack::new(42, 2)),
            (
                9,
                Vec3::new(f64::NAN, 64.5, 0.5),
                EntityItemStack::new(43, 3),
            ),
        ],
    );

    assert!(matches!(
        result,
        Err(mc_entity::RegionOwnerLaneError::InvalidMutation)
    ));
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

async fn run_authority_recheck_case(
    stage: BlockDropAwaitStage,
    cancel_response: bool,
    expected_block: BlockStateId,
) {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk = ChunkPos { x: 0, z: 0 };
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
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let token = storage.block_mutation_token(position).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (actor, _) = register_observer(&sessions, "AwaitAuthorityActor");
    let (_, mut observer) = register_observer(&sessions, "AwaitAuthorityObserver");
    let (journal, pending) =
        super::super::world_journal::WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(actor)
        .enqueue(SimulationCommand::CommitBlockDrops {
            actor_session: actor,
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            drops: vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        })
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let probe = block_drop_await_probe(stage, entered_tx, release_rx);
    let owner_sessions = Arc::clone(&sessions);
    let owner_world = Arc::clone(&world);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(with_block_drop_await_probe(probe, async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    }));
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    if cancel_response {
        drop(response);
        release_tx.send(()).unwrap();
        assert_eq!(owner_task.await.unwrap().processed, 1);
    } else {
        dispatch_visibility_commands(sessions.unregister(actor));
        release_tx.send(()).unwrap();
        assert_eq!(owner_task.await.unwrap().processed, 1);
        assert!(matches!(
            response.await.unwrap(),
            Err(SimulationRequestError::StaleSession)
        ));
    }

    assert_eq!(read_view.get_cached_block(position), Some(expected_block));
    assert_eq!(persisted_item_drop_count(&sessions), 0);
    assert_eq!(sessions.world_chunk_journal_watermark(), Some(1));
    assert_no_block_or_entity_publication(&mut observer);
    if expected_block == BlockStateId(0) {
        assert!(!world.lock().await.plan_dirty_flush().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_drop_rechecks_cancellation_and_session_after_each_journal_await() {
    run_authority_recheck_case(BlockDropAwaitStage::AfterReservation, true, BlockStateId(1)).await;
    run_authority_recheck_case(
        BlockDropAwaitStage::AfterReservation,
        false,
        BlockStateId(1),
    )
    .await;
    run_authority_recheck_case(BlockDropAwaitStage::AfterAppend, true, BlockStateId(0)).await;
    run_authority_recheck_case(BlockDropAwaitStage::AfterAppend, false, BlockStateId(0)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_drop_waits_for_earlier_reserved_decision_without_reordering() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk = ChunkPos { x: 0, z: 0 };
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
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let token = storage.block_mutation_token(position).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (journal, pending) =
        super::super::world_journal::WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    let earlier_id = journal.reserve_decision_ids(1).unwrap()[0];
    assert_eq!(earlier_id, 1);
    sessions.install_world_chunk_journal(journal.clone());

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let probe = block_drop_await_probe(
        BlockDropAwaitStage::AfterReservation,
        entered_tx,
        release_rx,
    );
    let owner_sessions = Arc::clone(&sessions);
    let owner_world = Arc::clone(&world);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(with_block_drop_await_probe(probe, async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    }));
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || {
        journal.record_reserved_snapshot_groups(0, vec![(earlier_id, Vec::new())])
    })
    .await
    .unwrap()
    .unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BlockDrops(Ok(outcome)) if outcome.is_some()
    ));
    assert_eq!(sessions.world_chunk_journal_watermark(), Some(2));
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert_eq!(persisted_item_drop_count(&sessions), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn block_drop_clear_mismatch_fail_stops_without_old_publication() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk = ChunkPos { x: 0, z: 0 };
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
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let token = storage.block_mutation_token(position).unwrap();
    let following_position = BlockPos { x: 2, y: 64, z: 1 };
    storage
        .set_block_at(following_position, BlockStateId(1))
        .unwrap();
    let following_token = storage.block_mutation_token(following_position).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (_, mut outbound) = register_observer(&sessions, "ClearMismatchObserver");
    let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
    let (journal, pending) =
        super::super::world_journal::WorldChunkJournal::open(temp.path(), blocks, items).unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let response = handle
        .enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        ))
        .unwrap();
    let following_response = handle
        .enqueue(block_drop_command(
            following_position,
            following_token,
            vec![test_drop(Vec3::new(1.5, 64.5, 0.5))],
        ))
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let probe = block_drop_await_probe(BlockDropAwaitStage::AfterAppend, entered_tx, release_rx);
    let owner_sessions = Arc::clone(&sessions);
    let owner_world = Arc::clone(&world);
    let owner_read = read_view.clone();
    let competing_mutation = mutation_view.clone();
    let owner_task = tokio::spawn(with_block_drop_await_probe(probe, async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    ..SimulationWorldAccess::default()
                },
                None,
                2,
            )
            .await
    }));
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    let stamp = competing_mutation.stamp_chunks_for_world_journal(99, &[chunk]);
    assert!(
        matches!(stamp, mc_world::JournalStampResult::Stamped(ref chunks) if chunks.len() == 1)
    );
    release_tx.send(()).unwrap();

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::WorldMutationFailed)
    ));
    assert!(matches!(
        following_response.await.unwrap(),
        Err(SimulationRequestError::OwnerStopped)
    ));
    journal_failure.changed().await.unwrap();
    assert!(*journal_failure.borrow_and_update());
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert_eq!(
        read_view.get_cached_block(following_position),
        Some(BlockStateId(1))
    );
    assert_eq!(persisted_item_drop_count(&sessions), 0);
    assert_no_block_or_entity_publication(&mut outbound);
    assert_eq!(sessions.world_chunk_journal_watermark(), Some(1));
    assert!(world.lock().await.plan_dirty_flush().unwrap().is_empty());
    assert!(matches!(
        handle.enqueue(block_drop_command(
            position,
            token,
            vec![test_drop(Vec3::new(0.5, 64.5, 0.5))],
        )),
        Err(SimulationRequestError::Closed)
    ));
}
