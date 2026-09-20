use super::super::{
    EntityPhysicsStep, GameMode, HOSTILE_MELEE_PERIOD_TICKS, ITEM_PICKUP_DELAY_TICKS, PlayerPose,
    SKELETON_BOW_DRAW_TICKS, SessionRegistry, SurvivalState,
};
use super::*;
use crate::login::LoggedInProfile;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::{PlayerPersistedState, XpState};
use crate::play::session::OutboundCommand;
use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::item_stack::ItemStack;
use mc_data::items::ItemRegistry;
use mc_entity::{EntityItemStack, Rotation, Vec3};
use mc_script::{ScriptAxisAlignedZone, ScriptPosition};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};
use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

#[path = "tests/precommit_tests.rs"]
mod precommit_tests;

#[test]
fn explosion_support_cascade_pops_ground_plant_with_precondition() {
    let reports = vec![
        block_report("minecraft:air", 0),
        block_report("minecraft:dirt", 1),
        block_report("minecraft:poppy", 2),
    ];
    let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
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
    let support = BlockPos { x: 4, y: 64, z: 4 };
    let plant = BlockPos { x: 4, y: 65, z: 4 };
    storage.set_block_at(support, BlockStateId(1)).unwrap();
    storage.set_block_at(plant, BlockStateId(2)).unwrap();

    let cascade = plan_explosion_support_cascade(&blocks, &storage, &[], support, BlockStateId(0));
    assert_eq!(cascade.len(), 1);
    assert_eq!(cascade[0].0.pos, plant);
    assert_eq!(cascade[0].0.new_state, BlockStateId(0));
    assert_eq!(cascade[0].1.pos, plant);
    assert_eq!(cascade[0].1.expected_state, BlockStateId(2));

    // The blast already destroying the plant itself must not duplicate it.
    let existing = vec![super::BlockEdit {
        pos: plant,
        new_state: BlockStateId(0),
    }];
    assert!(
        plan_explosion_support_cascade(&blocks, &storage, &existing, support, BlockStateId(0))
            .is_empty()
    );
}

struct FailOnceEntityCommitJournal {
    failure: Option<mc_entity::RegionalDecisionJournalError>,
    commits: Arc<AtomicUsize>,
}

impl mc_entity::RegionalDecisionJournal for FailOnceEntityCommitJournal {
    fn record_commit(
        &mut self,
        _decision: &mc_entity::RegionalCommitDecision,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        match self.failure.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn clear_commit(
        &mut self,
        _phase: mc_entity::RegionPhase,
    ) -> Result<(), mc_entity::RegionalDecisionJournalError> {
        Ok(())
    }
}

async fn assert_request_enqueued<F>(mut request: std::pin::Pin<&mut F>, handle: &SimulationHandle)
where
    F: std::future::Future,
{
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(request.as_mut(), cx).is_pending(),
            "request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 1, "request must be enqueued");
}

fn seed_claim_entities(registry: &SessionRegistry) -> (EntityId, EntityId) {
    let position = Vec3::new(0.5, 64.0, 0.5);
    registry.spawn_item_drop(1, position, EntityItemStack::new(42, 3));
    registry.spawn_xp_orb(2, position, 5);
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let item = registry.nearby_item_entities(position, 2.25)[0].id;
    let experience = registry.nearby_experience_entities(position, 2.25)[0].id;
    (item, experience)
}

fn publish_entity_spawns(
    dispatches: Vec<VisibilityDispatch>,
    outbound: &mut mpsc::Receiver<OutboundCommand>,
) -> Vec<EntityId> {
    let entity_ids = dispatches
        .iter()
        .filter_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(entity_ids.len(), dispatches.len());
    dispatch_visibility_commands(dispatches);
    for expected in &entity_ids {
        assert!(matches!(
            outbound.try_recv(),
            Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == *expected
        ));
    }
    entity_ids
}

fn seed_claim_entities_published(
    registry: &SessionRegistry,
    outbound: &mut mpsc::Receiver<OutboundCommand>,
) -> (EntityId, EntityId) {
    let position = Vec3::new(0.5, 64.0, 0.5);
    dispatch_visibility_commands(registry.spawn_item_drop(
        1,
        position,
        EntityItemStack::new(42, 3),
    ));
    dispatch_visibility_commands(registry.spawn_xp_orb(2, position, 5));
    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    let item = registry.nearby_item_entities(position, 2.25)[0].id;
    let experience = registry.nearby_experience_entities(position, 2.25)[0].id;
    let mut published_spawns = 0;
    while let Ok(command) = outbound.try_recv() {
        if matches!(command, OutboundCommand::SpawnEntity(_)) {
            published_spawns += 1;
        }
    }
    assert_eq!(published_spawns, 2);
    (item, experience)
}

fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
    register_test_session_with_outbound(registry, name).0
}

fn register_test_session_with_outbound(
    registry: &SessionRegistry,
    name: &str,
) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
    register_test_session_at_with_outbound(registry, name, PlayerPose::new(0.5, 64.0, 0.5))
}

fn register_test_session_at_with_outbound(
    registry: &SessionRegistry,
    name: &str,
    pose: PlayerPose,
) -> (SessionId, mpsc::Receiver<OutboundCommand>) {
    let profile = crate::login::LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (tx, rx) = mpsc::channel(64);
    let session_id = registry
        .register(&profile, (0, 0), 2, HashSet::new(), tx, pose)
        .0;
    (session_id, rx)
}

fn register_test_player_state(
    registry: &SessionRegistry,
    session_id: SessionId,
    inventory: PlayerInventory,
) -> Arc<Mutex<PlayerPersistedState>> {
    let mut state = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    state.inventory = inventory;
    let state = Arc::new(Mutex::new(state));
    registry.register_player_persistence(session_id, Arc::clone(&state));
    state
}

fn empty_container_player_plan() -> ContainerPlayerPlan {
    let inventory = PlayerInventory::empty();
    ContainerPlayerPlan {
        expected_inventory: inventory.clone(),
        expected_carried_item: ItemStack::EMPTY,
        updated_inventory: inventory,
        updated_carried_item: ItemStack::EMPTY,
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops: Vec::new(),
        xp_orb: None,
    }
}

fn test_survival_break_plan(
    pos: BlockPos,
    token: BlockMutationToken,
    tool_item_id: u32,
    drop_item_id: u32,
) -> SurvivalBreakPlan {
    SurvivalBreakPlan {
        edits: vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        blocks: Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &test_block_reports(),
        )),
        falling_block_entity_type_id: Some(99),
        held: SurvivalBreakHeldItem {
            hotbar_slot: 0,
            expected: ItemStack::new(tool_item_id, 1),
            max_damage: Some(10),
        },
        drops: vec![SurvivalBreakDrop {
            entity_type_id: 1,
            position: Vec3::new(0.5, 64.5, 0.5),
            stack: EntityItemStack::new(drop_item_id, 1),
        }],
        hook_approval: None,
        zone_fence: None,
    }
}

fn test_survival_block_break_plan(
    pos: BlockPos,
    token: BlockMutationToken,
) -> SurvivalBlockBreakPlan {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:wooden_pickaxe").unwrap(),
            protocol_id: 42,
        },
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 7,
        },
        mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:paper").unwrap(),
            protocol_id: 45,
        },
    ]));
    SurvivalBlockBreakPlan {
        position: pos,
        expected_target: BlockMutationSnapshot {
            state: BlockStateId(1),
            token,
        },
        blocks,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &test_block_reports(),
        )),
        water: Some(BlockStateId(2)),
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        item_entity_type_id: Some(1),
        falling_block_entity_type_id: Some(99),
        loader_block_drop: None,
        held: SurvivalBreakHeldItem {
            hotbar_slot: 0,
            expected: ItemStack::new(42, 1),
            max_damage: Some(10),
        },
        drop_items: true,
        hook_approval: None,
        zone_fence: None,
    }
}

fn test_survival_placement_plan(
    target: BlockPos,
    target_token: BlockMutationToken,
    support: BlockPos,
    support_token: BlockMutationToken,
    item_id: u32,
    count: i32,
) -> SurvivalPlacementPlan {
    SurvivalPlacementPlan {
        edits: vec![BlockEdit {
            pos: target,
            new_state: BlockStateId(1),
        }],
        preconditions: vec![
            BlockEditPrecondition {
                pos: target,
                expected_state: BlockStateId(0),
                expected_token: target_token,
            },
            BlockEditPrecondition {
                pos: support,
                expected_state: BlockStateId(1),
                expected_token: support_token,
            },
        ],
        scheduled_block_ticks: Vec::new(),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &test_block_reports(),
        )),
        held: SurvivalPlacementHeldItem {
            inventory_slot: PlayerInventory::HOTBAR_BASE,
            expected: ItemStack::new(item_id, count),
        },
        hook_approval: None,
        zone_fence: None,
        expected_game_mode: GameMode::Survival,
    }
}

fn test_bucket_use_plan(target: BlockPos, target_token: BlockMutationToken) -> BucketUsePlan {
    BucketUsePlan {
        edit: BlockEdit {
            pos: target,
            new_state: BlockStateId(2),
        },
        precondition: BlockEditPrecondition {
            pos: target,
            expected_state: BlockStateId(0),
            expected_token: target_token,
        },
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &test_block_reports(),
        )),
        inventory: Some(BucketInventoryChange {
            held_slot: PlayerInventory::HOTBAR_BASE,
            expected_held: ItemStack::new(61, 1),
            replacement_item: 60,
            replacement_max_stack: 16,
        }),
        schedule_fluid_ticks: true,
        hook_approval: None,
        zone_fence: None,
    }
}

fn persisted_item_drop_count(registry: &SessionRegistry) -> usize {
    registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.item_stack.is_some())
        .count()
}

fn claim_xp(entity_id: i32, collector_session: SessionId) -> SimulationCommand {
    SimulationCommand::ClaimExperiencePickup {
        entity_id: EntityId(entity_id),
        collector_session,
    }
}

fn seed_grounded_arrow(registry: &SessionRegistry) -> EntityId {
    registry.spawn_arrow_for_test(
        None,
        3,
        Vec3::new(0.5, 64.0, 0.5),
        Vec3::new(0.0, 0.0, 1.0),
        Rotation::ZERO,
    );
    let entity_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:arrow")
        .expect("spawned arrow record")
        .snapshot
        .id;
    registry.apply_entity_physics_and_dispatch(
        1,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        }],
    );
    entity_id
}

fn seed_attack_target(registry: &SessionRegistry) -> EntityId {
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        4,
        "minecraft:zombie".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    );
    registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:zombie")
        .expect("spawned attack target")
        .snapshot
        .id
}

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

fn fluid_block_report(id: &str, state_id: u32) -> BlockReport {
    let mut properties = BTreeMap::new();
    properties.insert("level".to_owned(), vec!["0".to_owned()]);
    let mut state_properties = BTreeMap::new();
    state_properties.insert("level".to_owned(), "0".to_owned());
    BlockReport {
        id: Identifier::parse(id).unwrap(),
        properties,
        states: vec![BlockStateReport {
            id: state_id,
            default: true,
            properties: state_properties,
        }],
    }
}

fn test_block_reports() -> Vec<BlockReport> {
    vec![
        block_report("minecraft:air", 0),
        block_report("minecraft:stone", 1),
        fluid_block_report("minecraft:water", 2),
        block_report("minecraft:sand", 3),
        block_report("minecraft:campfire", 4),
        block_report("minecraft:tnt", 5),
        block_report("minecraft:dirt", 6),
    ]
}

fn test_block_storage() -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    seed_test_block_storage(WorldStorage::in_memory(blocks))
}

fn test_block_storage_with_capacity(
    capacity: usize,
) -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    seed_test_block_storage(WorldStorage::in_memory_with_capacity(blocks, capacity))
}

fn seed_test_block_storage(
    mut storage: WorldStorage,
) -> (WorldStorage, BlockPos, mc_world::BlockMutationToken) {
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
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    assert_eq!(
        storage.set_block_at(pos, BlockStateId(1)).unwrap(),
        Some(BlockStateId(0))
    );
    let token = storage
        .block_mutation_token(pos)
        .expect("resident block mutation token");
    (storage, pos, token)
}

fn test_container_storage() -> (WorldStorage, BlockPos) {
    let (storage, pos, _) = test_block_storage();
    (storage, pos)
}

#[tokio::test(flavor = "current_thread")]
async fn block_snapshot_query_runs_through_simulation_owner() {
    let (storage, position, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "BlockSnapshotReader");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.read_block_snapshot(position));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world(&registry, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    assert_eq!(
        request.await.unwrap(),
        Some(BlockMutationSnapshot {
            state: BlockStateId(1),
            token,
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_block_snapshot_does_not_wait_for_world_writer() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "ResidentBlockSnapshotReader");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.read_block_snapshot(position));
    assert_request_enqueued(request.as_mut(), &handle).await;

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
                    ..SimulationWorldAccess::default()
                },
                None,
                1,
            )
            .await
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request).await;
    drop(writer);

    assert_eq!(
        outcome.expect("resident read completes while storage writer is held"),
        Ok(Some(BlockMutationSnapshot {
            state: BlockStateId(1),
            token,
        }))
    );
    assert_eq!(owner_task.await.expect("owner task").processed, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_block_edit_does_not_wait_for_world_writer() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let light = BlockLightTable::from_arrays(
        "resident inert edit",
        vec![0, 0, 0, 0, 0],
        vec![0, 0, 0, 0, 0],
        vec![true, true, true, true, true],
    );
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "ResidentBlockEditor");
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

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
                    cpu: Some(&resources),
                    light: None,
                },
                Some(&light),
                1,
            )
            .await
    });
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("resident mutation completion event");
    drop(writer);

    let outcome = outcome.expect("resident mutation response");
    assert_eq!(outcome.unwrap().applied.len(), 1);
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert_eq!(owner_task.await.expect("owner task").processed, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn regional_block_edit_is_durable_before_response_publication() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk_position = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_position,
            Chunk::empty(
                chunk_position,
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
    let sessions = SessionRegistry::new();
    let (journal, pending) = super::super::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(!world.lock().await.plan_dirty_flush().unwrap().is_empty());

    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].get_block(1, 64, 1),
        Some(BlockStateId(0)),
        "response publication requires the post-mutation chunk image to be durable"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn journaled_block_edit_does_not_capture_following_non_journaled_mutation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk_position = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_position,
            Chunk::empty(
                chunk_position,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let block_position = BlockPos { x: 1, y: 64, z: 1 };
    let entity_position = BlockPos { x: 2, y: 64, z: 1 };
    storage
        .set_block_at(block_position, BlockStateId(1))
        .unwrap();
    storage
        .set_block_at(entity_position, BlockStateId(1))
        .unwrap();
    let block_token = storage.block_mutation_token(block_position).unwrap();
    let entity_token = storage.block_mutation_token(entity_position).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (journal, pending) = super::super::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    sessions.install_world_chunk_journal(journal);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let block_response = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos: block_position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: block_position,
                expected_state: BlockStateId(1),
                expected_token: block_token,
            }],
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();
    let bytes = vec![10, 0, 0, 0];
    let entity_response = handle
        .enqueue(SimulationCommand::CommitOpaqueBlockEntity {
            position: entity_position,
            expected_state: BlockStateId(1),
            expected_token: entity_token,
            bytes: bytes.clone(),
        })
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                2,
            )
            .await
            .processed,
        2
    );
    assert!(matches!(
        block_response.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(matches!(
        entity_response.await.unwrap().unwrap(),
        SimulationResponse::OpaqueBlockEntity(Ok(true))
    ));

    let live = read_view.snapshot_chunks(&[chunk_position]);
    assert_eq!(
        live.chunk(chunk_position)
            .unwrap()
            .block_entities
            .get(&entity_position),
        Some(&bytes)
    );
    let reopened = sessions.world_chunk_journal().unwrap();
    let pending = reopened.pending_decisions_for_test();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].get_block(1, 64, 1), Some(BlockStateId(0)));
    assert!(!restored[0].block_entities.contains_key(&entity_position));
}

#[tokio::test(flavor = "current_thread")]
async fn journal_failure_after_ram_acceptance_stops_further_world_mutations() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk_position = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_position,
            Chunk::empty(
                chunk_position,
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
    let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
    let (journal, pending) = super::super::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    let writer = Arc::clone(&journal.writer);
    world.lock().await.set_journal_barrier({
        let writer = Arc::clone(&writer);
        Arc::new(move || writer.flush().map_err(std::io::Error::other))
    });
    sessions.install_world_chunk_journal(journal);
    let (session, _outbound) = register_test_session_with_outbound(&sessions, "WalFailure");
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(session),
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
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
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    writer.flush().unwrap();
    let (paused, resume_writer) = std::sync::mpsc::sync_channel(0);
    writer
        .requests
        .send(super::super::world_journal::WriterRequest::Flush { reply: paused })
        .unwrap();
    let journal_path = temp.path().join("solaris/world-chunk-journal.bin");
    std::fs::remove_file(&journal_path).unwrap();
    std::fs::create_dir(&journal_path).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(owner_task.await.unwrap().processed, 1);
    response.await.unwrap().unwrap();
    resume_writer.recv().unwrap();
    journal_failure.changed().await.unwrap();
    assert!(*journal_failure.borrow_and_update());
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
    assert!(matches!(
        world.lock().await.plan_dirty_flush().unwrap().write(),
        Err(mc_world::WorldError::JournalBarrier(_))
    ));

    let snapshot = read_view.snapshot_chunks(&[chunk_position]);
    let error = sessions
        .world_chunk_journal()
        .unwrap()
        .record_snapshots(2, vec![snapshot.chunk(chunk_position).unwrap()])
        .unwrap_err();
    assert!(matches!(
        error,
        super::super::world_journal::WorldChunkJournalError::PoisonedOutcomeUnknown
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_fluid_tick_schedule_notifies_flush_without_world_writer() {
    let (mut storage, position, _) = test_block_storage_with_capacity(1);
    storage.set_block_at(position, BlockStateId(2)).unwrap();
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let (flush_started, flush_started_rx) = oneshot::channel();
    let mut flush_started = Some(flush_started);
    let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn({
        let flush_calls = Arc::clone(&flush_calls);
        move || {
            let flush_calls = Arc::clone(&flush_calls);
            let flush_started = flush_started.take();
            async move {
                flush_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(flush_started) = flush_started {
                    let _ = flush_started.send(());
                }
            }
        }
    });
    let dirty_flush = coordinator.notifier();
    storage.set_dirty_high_water_notifier(Arc::new(move || dirty_flush.request()));
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &test_block_reports(),
    ));
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::ScheduleFluidTicksNearApplied {
            applied: vec![AppliedBlockEdit {
                pos: position,
                previous: BlockStateId(0),
                new_state: BlockStateId(2),
            }],
            block_facts: facts,
            world_tick: 40,
        })
        .unwrap();

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    cpu: None,
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
        .await
        .expect("resident fluid schedule completion event")
        .unwrap()
        .unwrap();
    assert!(matches!(response, SimulationResponse::FluidTicksScheduled));
    assert_eq!(flush_started_rx.await, Ok(()));
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    let mut storage = world.lock().await;
    let ticks = storage
        .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .unwrap();
    assert!(ticks.iter().any(|tick| tick.pos == position));
    drop(storage);
    coordinator.drain().await;
    assert!(flush_calls.load(Ordering::SeqCst) >= 1);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_block_edit_does_not_notify_dirty_flush() {
    let (storage, position, token) = test_block_storage();
    let notifications = Arc::new(AtomicUsize::new(0));
    storage.set_dirty_high_water_notifier({
        let notifications = Arc::clone(&notifications);
        Arc::new(move || {
            notifications.fetch_add(1, Ordering::SeqCst);
        })
    });
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos: position,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos: position,
                expected_state: BlockStateId(0),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();

    let report = owner
        .process_commands_with_world_views(
            &sessions,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            1,
        )
        .await;

    assert_eq!(report.processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
    ));
    assert_eq!(notifications.load(Ordering::SeqCst), 0);
}

/// A server-owned batch carries no writer session, so it must never take an
/// accelerated lane: those lanes publish through the writer's visible-edit
/// finalize, which is the only place a writer's replaced campfire loses its
/// cooking state. The routing predicate in `command_can_use_resident_mutation`
/// is what keeps that invariant, so assert it directly.
#[test]
fn server_owned_block_edits_never_take_a_session_fast_lane() {
    let (storage, position, _token) = test_block_storage();
    let read_view = storage.read_view();
    let edit = BlockEdit {
        pos: position,
        new_state: BlockStateId(0),
    };
    let session_batch = SimulationCommand::ApplyBlockEdits {
        actor_session: Some(0),
        edits: vec![edit],
        preconditions: Vec::new(),
        scheduled_block_ticks: Vec::new(),
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };
    let server_owned_batch = SimulationCommand::ApplyBlockEdits {
        actor_session: None,
        edits: vec![edit],
        preconditions: Vec::new(),
        scheduled_block_ticks: Vec::new(),
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };

    assert!(
        command_can_use_resident_mutation(&session_batch, Some(&read_view), None, false),
        "a cached single-region session batch still qualifies for the fast lane"
    );
    assert!(
        !command_can_use_resident_mutation(&server_owned_batch, Some(&read_view), None, false),
        "a server-owned batch must take the staged path that evicts its cooking state"
    );
    assert!(
        !command_can_use_regional_mutation(&server_owned_batch, Some(&read_view), None),
        "the regional lane delegates to the same eligibility, so it is closed too"
    );
}

#[tokio::test]
async fn regional_block_edits_in_distinct_lanes_overlap() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let positions = [
        BlockPos { x: 1, y: 64, z: 1 },
        BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }
    let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let light = Arc::new(BlockLightTable::from_arrays(
        "regional light-changing edit",
        vec![0, 0, 0, 0, 0],
        vec![0, 15, 0, 0, 0],
        vec![true, false, true, true, true],
    ));
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = positions
        .into_iter()
        .zip(tokens)
        .map(|(position, token)| {
            handle
                .enqueue(SimulationCommand::ApplyBlockEdits {
                    actor_session: Some(0),
                    edits: vec![BlockEdit {
                        pos: position,
                        new_state: BlockStateId(0),
                    }],
                    preconditions: vec![BlockEditPrecondition {
                        pos: position,
                        expected_state: BlockStateId(1),
                        expected_token: token,
                    }],
                    scheduled_block_ticks: Vec::new(),
                    leaf_trigger: true,
                    hook_approval: None,
                    zone_fence: None,
                    plugin_receipt: None,
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);

    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&light),
                },
                Some(light.as_ref()),
                2,
            ))
    });

    let first = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional worker entry");
    let second = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional worker enters before release");
    assert_ne!(first, second);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    let report = worker.join().unwrap();
    assert_eq!(report.processed, 2);
    assert_eq!(report.lane_attribution.len(), 2);
    assert!(
        report
            .lane_attribution
            .iter()
            .flat_map(|lane| &lane.commands)
            .all(|attribution| attribution.kind == "apply_block_edits")
    );
    for response in responses {
        let SimulationResponse::BlockEdits(Ok(outcome)) = response.await.unwrap().unwrap() else {
            panic!("regional light-changing response mismatch");
        };
        let outcome = outcome.expect("regional light-changing commit");
        assert!(outcome.precomputed_light_updates.is_some());
    }
    let world = world.lock().await;
    for position in positions {
        assert_eq!(world.get_cached_block(position), Some(BlockStateId(0)));
        assert!(
            mc_world::light::ChunkLight::from_section_lights(
                &world
                    .cached_chunk_snapshot(ChunkPos {
                        x: position.x.div_euclid(16),
                        z: position.z.div_euclid(16),
                    })
                    .unwrap()
                    .section_lights,
            )
            .is_some()
        );
    }
}

#[test]
fn lane_attribution_counts_wait_once_for_two_commands() {
    let report = SimulationTickReport {
        processed: 2,
        remaining_depth: 0,
        lane_attribution: vec![SimulationLaneAttribution {
            cpu_admission_wait_us: 65_467,
            commands: vec![
                SimulationCommandAttribution {
                    kind: "apply_block_edits",
                    post_admission_command_us: 321,
                },
                SimulationCommandAttribution {
                    kind: "apply_block_edits",
                    post_admission_command_us: 654,
                },
            ],
        }],
    };

    assert_eq!(report.lane_attribution.len(), 1);
    assert_eq!(report.lane_attribution[0].cpu_admission_wait_us, 65_467);
    assert_eq!(report.lane_attribution[0].commands.len(), 2);
    assert_eq!(
        report.lane_attribution[0].commands[1].post_admission_command_us,
        654
    );
}

#[tokio::test(flavor = "current_thread")]
async fn regional_block_edits_in_one_lane_preserve_sequence() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let command = || SimulationCommand::ApplyBlockEdits {
        actor_session: Some(0),
        edits: vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        scheduled_block_ticks: Vec::new(),
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };
    let first = handle.enqueue(command()).unwrap();
    let stale = handle.enqueue(command()).unwrap();

    let report = owner
        .process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            2,
        )
        .await;
    assert_eq!(report.processed, 2);
    assert_eq!(report.lane_attribution.len(), 1);
    assert_eq!(report.lane_attribution[0].commands.len(), 2);
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(matches!(
        stale.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
    ));
    assert_eq!(read_view.get_cached_block(position), Some(BlockStateId(0)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_world_run_preserves_regional_waves_around_coordinator_barrier() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let positions = [
        BlockPos { x: 1, y: 64, z: 1 },
        BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }
    let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    let command = |position, token| SimulationCommand::ApplyBlockEdits {
        actor_session: Some(0),
        edits: vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        scheduled_block_ticks: Vec::new(),
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };
    let first = handle.enqueue(command(positions[0], tokens[0])).unwrap();
    let second = handle.enqueue(command(positions[1], tokens[1])).unwrap();
    let barrier = handle
        .enqueue(SimulationCommand::ReadChestSnapshot {
            positions: vec![positions[0]],
        })
        .unwrap();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                3,
            )
            .await
    });

    let first_region = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional worker entry");
    let second_region = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional worker enters before release");
    assert_ne!(first_region, second_region);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    let first = first.await.unwrap().unwrap();
    assert!(matches!(
        first,
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(matches!(
        second.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert_eq!(
        read_view.get_cached_block(positions[0]),
        Some(BlockStateId(0))
    );
    assert_eq!(
        read_view.get_cached_block(positions[1]),
        Some(BlockStateId(0))
    );

    drop(writer);
    assert!(matches!(
        barrier.await.unwrap().unwrap(),
        SimulationResponse::ChestSnapshot(_)
    ));
    assert_eq!(owner_task.await.unwrap().processed, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn chest_snapshot_query_runs_through_simulation_owner() {
    let (mut storage, position) = test_container_storage();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 3,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    storage
        .set_chest_block_entity(position, chest.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "ChestSnapshotReader");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.read_chest_snapshot(vec![position]));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world(&registry, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    let snapshot = request.await.expect("chest snapshot owner response");
    assert_eq!(snapshot.state_id, 1);
    assert_eq!(snapshot.view.chests, vec![chest]);
}

#[tokio::test(flavor = "current_thread")]
async fn furnace_snapshot_query_runs_through_simulation_owner() {
    let (mut storage, position) = test_container_storage();
    let mut furnace = FurnaceBlockEntity::default();
    furnace.slots[1] = mc_world::FurnaceSlot {
        item_id: 17,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    storage
        .set_furnace_block_entity(position, furnace.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "FurnaceSnapshotReader");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.read_furnace_snapshot(position));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world(&registry, Some(&world), None, 1)
            .await
            .processed,
        1
    );
    let snapshot = request.await.expect("furnace snapshot owner response");
    assert_eq!(snapshot.state_id, 1);
    assert_eq!(snapshot.furnace, furnace);
}

#[tokio::test(flavor = "current_thread")]
async fn player_attack_validation_and_damage_run_in_one_owner_command() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "AtomicAttackAlice");
    register_test_player_state(&registry, session, PlayerInventory::empty());
    let target = seed_attack_target(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.player_attack_server_entity(target, 5.0));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let PlayerAttackResult::Damaged(outcome) = request.await.expect("player attack owner response")
    else {
        panic!("nearby living target must take damage");
    };
    assert_eq!(outcome.damage().snapshot.health, 15.0);

    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        "minecraft:cow".to_owned(),
        Vec3::new(20.5, 64.0, 0.5),
    );
    let far_target = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:cow")
        .expect("far attack target")
        .snapshot
        .id;
    let far_health_before = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == far_target)
        .expect("far target before attack")
        .snapshot
        .health;
    let mut rejected = Box::pin(session_handle.player_attack_server_entity(far_target, 5.0));

    assert_request_enqueued(rejected.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        rejected.await.expect("far attack owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert_eq!(
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == far_target)
            .expect("far target remains")
            .snapshot
            .health,
        far_health_before
    );
}

#[tokio::test(flavor = "current_thread")]
async fn player_attack_rechecks_authoritative_attacker_mode_and_liveness() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "FencedAttackAlice");
    let attacker_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    let target = seed_attack_target(&registry);
    let target_health = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == target)
        .expect("attack target before fenced requests")
        .snapshot
        .health;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);

    attacker_state.lock().unwrap().game_mode = GameMode::Spectator;
    let mut spectator_attack = Box::pin(session_handle.player_attack_server_entity(target, 5.0));
    assert_request_enqueued(spectator_attack.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        spectator_attack
            .await
            .expect("spectator attack owner response"),
        PlayerAttackResult::ValidationRejected
    ));

    {
        let mut state = attacker_state.lock().unwrap();
        state.game_mode = GameMode::Survival;
        state.survival.health = 0.0;
    }
    let mut dead_attack = Box::pin(session_handle.player_attack_server_entity(target, 5.0));
    assert_request_enqueued(dead_attack.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        dead_attack.await.expect("dead attack owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert_eq!(
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == target)
            .expect("attack target after fenced requests")
            .snapshot
            .health,
        target_health
    );
}

#[tokio::test(flavor = "current_thread")]
async fn player_attack_rejects_out_of_reach_adventure_and_creative_targets() {
    for game_mode in [GameMode::Adventure, GameMode::Creative] {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "RemoteAttackAlice");
        register_test_player_state(&registry, session, PlayerInventory::empty());
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            5,
            "minecraft:cow".to_owned(),
            Vec3::new(20.5, 64.0, 0.5),
        );
        let target = registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.type_name == "minecraft:cow")
            .expect("far attack target")
            .snapshot;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let session_handle = handle.for_session(session);
        registry
            .commit_player_state_event(
                &SimulationAuthority::for_test(),
                session,
                PlayerStateEvent::GameMode(game_mode),
            )
            .expect("set authoritative attacker mode");
        let mut request = Box::pin(session_handle.player_attack_server_entity(target.id, 5.0));

        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            request.await.expect("far attack owner response"),
            PlayerAttackResult::ValidationRejected
        ));
        assert_eq!(
            registry
                .persisted_entity_records()
                .into_iter()
                .find(|record| record.snapshot.id == target.id)
                .expect("far target remains")
                .snapshot
                .health,
            target.health
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn local_shield_commit_refreshes_identity_before_queued_pvp_in_same_batch() {
    let registry = SessionRegistry::new();
    let attacker_pose = PlayerPose::new(0.5, 64.0, 2.5);
    let target_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (attacker, _attacker_rx) =
        register_test_session_at_with_outbound(&registry, "ShieldBatchAttacker", attacker_pose);
    let (target, _target_rx) =
        register_test_session_at_with_outbound(&registry, "ShieldBatchTarget", target_pose);
    register_test_player_state(&registry, attacker, PlayerInventory::empty());

    let items = Arc::new(mc_data::items::solaris_required_items());
    let item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    let shield_item = items
        .id_of(&Identifier::parse("minecraft:shield").unwrap())
        .unwrap();
    registry.configure_player_combat(None, None, Arc::clone(&items), item_facts);
    registry.set_world_time(10);

    let shield_slot = PlayerInventory::OFFHAND_SLOT;
    let initial_stack = ItemStack::new(shield_item, 1);
    let locally_damaged_stack = initial_stack.clone().with_damage(5);
    let mut initial_inventory = PlayerInventory::empty();
    initial_inventory.slots[shield_slot] = initial_stack.clone();
    let target_state = register_test_player_state(&registry, target, initial_inventory.clone());
    let initial_shield = crate::play::combat::ActiveShield {
        started_tick: 0,
        slot: shield_slot,
        expected_stack: initial_stack,
    };
    let locally_refreshed_shield = crate::play::combat::ActiveShield {
        expected_stack: locally_damaged_stack.clone(),
        ..initial_shield.clone()
    };
    registry.set_active_shield(target, Some(initial_shield.clone()));

    let mut locally_updated_inventory = initial_inventory.clone();
    locally_updated_inventory.slots[shield_slot] = locally_damaged_stack;
    let local_plan = PlayerSurvivalPlan {
        hook_approval: None,
        expected_survival: SurvivalState::FULL,
        updated_survival: SurvivalState::FULL,
        expected_inventory: initial_inventory,
        updated_inventory: locally_updated_inventory,
        expected_carried_item: ItemStack::EMPTY,
        expected_xp: XpState::default(),
        updated_xp: XpState::default(),
        active_shield: Some(ActiveShieldTransition {
            expected: Some(initial_shield),
            updated: Some(locally_refreshed_shield),
        }),
        enchanting_table_input: None,
        item_entity_type_id: None,
        xp_orb_entity_type_id: None,
        keep_inventory: false,
        position: Vec3::new(target_pose.x, target_pose.y, target_pose.z),
    };

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let target_handle = handle.for_session(target);
    let attacker_handle = handle.for_session(attacker);
    let mut local_commit = Box::pin(target_handle.commit_player_survival(Box::new(local_plan)));
    assert_request_enqueued(local_commit.as_mut(), &handle).await;
    let mut pvp = Box::pin(
        attacker_handle.player_attack_server_entity(EntityId(i32::try_from(target).unwrap()), 4.0),
    );
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(pvp.as_mut(), cx).is_pending(),
            "queued PvP must wait for the owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 2);

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    assert!(matches!(
        local_commit.await.unwrap(),
        Some(outcome) if matches!(*outcome, PlayerSurvivalCommitOutcome::Committed(_))
    ));
    assert!(matches!(
        pvp.await.unwrap(),
        PlayerAttackResult::Damaged(outcome)
            if matches!(
                *outcome,
                EntityAttackOutcome::PlayerDamaged {
                    damage_applied: false,
                    ..
                }
            )
    ));
    let target_state = target_state.lock().unwrap();
    assert_eq!(target_state.survival, SurvivalState::FULL);
    assert_eq!(
        target_state.inventory.slots[shield_slot],
        ItemStack::new(shield_item, 1).with_damage(10)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn player_melee_routes_damage_only_to_target_session() {
    let registry = SessionRegistry::new();
    let (alice, mut alice_rx) = register_test_session_at_with_outbound(
        &registry,
        "PvpAlice",
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, mut bob_rx) = register_test_session_at_with_outbound(
        &registry,
        "PvpBob",
        PlayerPose::new(0.5, 64.0, 2.5),
    );
    let (carol, mut carol_rx) = register_test_session_at_with_outbound(
        &registry,
        "PvpCarol",
        PlayerPose::new(0.5, 64.0, 3.5),
    );
    for session in [alice, bob, carol] {
        register_test_player_state(&registry, session, PlayerInventory::empty());
    }
    while alice_rx.try_recv().is_ok() {}
    while bob_rx.try_recv().is_ok() {}
    while carol_rx.try_recv().is_ok() {}

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let alice_handle = handle.for_session(alice);
    let mut request = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let PlayerAttackResult::Damaged(outcome) = request.await.expect("player melee owner response")
    else {
        panic!("reachable player target must accept melee damage");
    };
    assert!(matches!(
        &*outcome,
        EntityAttackOutcome::PlayerDamaged { target_session, .. } if *target_session == bob
    ));
    dispatch_visibility_commands(outcome.into_dispatches());
    assert!(matches!(
        bob_rx.try_recv(),
        Ok(OutboundCommand::PlayerDamageCommitted { .. })
    ));
    assert!(
        bob_rx.try_recv().is_err(),
        "the authoritative commit must publish exactly once to the victim"
    );
    while let Ok(command) = alice_rx.try_recv() {
        assert!(!matches!(
            command,
            OutboundCommand::DamagePlayer { .. } | OutboundCommand::PlayerDamageCommitted { .. }
        ));
        assert!(!matches!(command, OutboundCommand::EntityEvent { .. }));
    }
    while let Ok(command) = carol_rx.try_recv() {
        assert!(!matches!(
            command,
            OutboundCommand::DamagePlayer { .. } | OutboundCommand::PlayerDamageCommitted { .. }
        ));
        assert!(!matches!(command, OutboundCommand::EntityEvent { .. }));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn reciprocal_player_attacks_commit_without_connection_loop_progress() {
    let registry = SessionRegistry::new();
    let mut attacks = registry.subscribe_player_attacks();
    let (alice, _alice_rx) = register_test_session_at_with_outbound(
        &registry,
        "ReciprocalAlice",
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, _bob_rx) = register_test_session_at_with_outbound(
        &registry,
        "ReciprocalBob",
        PlayerPose::new(0.5, 64.0, 2.5),
    );
    let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
    let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let alice_handle = handle.for_session(alice);
    let bob_handle = handle.for_session(bob);
    let attack_costs = |position: Vec3| {
        let mut updated_survival = SurvivalState::FULL;
        updated_survival
            .add_exhaustion(mc_entity::player_survival_26_1_2::ENTITY_ATTACK_EXHAUSTION);
        PlayerSurvivalPlan {
            hook_approval: None,
            expected_survival: SurvivalState::FULL,
            updated_survival,
            expected_inventory: PlayerInventory::empty(),
            updated_inventory: PlayerInventory::empty(),
            expected_carried_item: ItemStack::EMPTY,
            expected_xp: XpState::default(),
            updated_xp: XpState::default(),
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            keep_inventory: false,
            position,
        }
    };
    let mut alice_attack = Box::pin(alice_handle.player_attack_server_entity_with_costs(
        EntityId(i32::try_from(bob).unwrap()),
        4.0,
        attack_costs(Vec3::new(0.5, 64.0, 0.5)),
        7,
    ));
    let mut bob_attack = Box::pin(bob_handle.player_attack_server_entity_with_costs(
        EntityId(i32::try_from(alice).unwrap()),
        4.0,
        attack_costs(Vec3::new(0.5, 64.0, 2.5)),
        8,
    ));

    std::future::poll_fn(|cx| {
        assert!(alice_attack.as_mut().poll(cx).is_pending());
        assert!(bob_attack.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 2);
    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let first = attacks.try_recv().expect("first authority observation");
    let second = attacks.try_recv().expect("second authority observation");
    assert_eq!(
        (first.attacker_session_id, first.target_entity_id),
        (alice, i32::try_from(bob).unwrap())
    );
    assert_eq!(
        (second.attacker_session_id, second.target_entity_id),
        (bob, i32::try_from(alice).unwrap())
    );
    assert_eq!((first.cooldown_tick, second.cooldown_tick), (7, 8));
    assert_eq!((first.authority_tick, second.authority_tick), (0, 0));
    assert_eq!(
        (first.authority_sequence, second.authority_sequence),
        (1, 2)
    );
    assert!(attacks.try_recv().is_err());
    assert!(matches!(
        alice_attack.await.expect("Alice attack owner response"),
        PlayerAttackResult::Damaged(_)
    ));
    assert!(matches!(
        bob_attack.await.expect("Bob attack owner response"),
        PlayerAttackResult::Damaged(_)
    ));
    let alice_state = alice_state.lock().unwrap();
    let bob_state = bob_state.lock().unwrap();
    assert_eq!(alice_state.survival.health, 16.0);
    assert_eq!(bob_state.survival.health, 16.0);
    assert_eq!(alice_state.survival.exhaustion, 0.1);
    assert_eq!(bob_state.survival.exhaustion, 0.1);
}

#[tokio::test(flavor = "current_thread")]
async fn player_melee_rejects_out_of_range_target() {
    let registry = SessionRegistry::new();
    let (alice, _alice_rx) = register_test_session_at_with_outbound(
        &registry,
        "FarPvpAlice",
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, mut bob_rx) = register_test_session_at_with_outbound(
        &registry,
        "FarPvpBob",
        PlayerPose::new(0.5, 64.0, 2.5),
    );
    for session in [alice, bob] {
        register_test_player_state(&registry, session, PlayerInventory::empty());
    }
    while bob_rx.try_recv().is_ok() {}

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let alice_handle = handle.for_session(alice);
    let mut nearby = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(nearby.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        nearby.await.expect("near player melee owner response"),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
    ));
    while bob_rx.try_recv().is_ok() {}

    registry
        .commit_player_pose(
            &SimulationAuthority::for_test(),
            bob,
            PlayerPose::new(20.5, 64.0, 0.5),
            0.0,
        )
        .expect("move target out of melee range");
    let mut request = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        request.await.expect("far player melee owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(bob_rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn player_melee_accepts_adventure_and_rejects_invulnerable_modes() {
    let registry = SessionRegistry::new();
    let (alice, _alice_rx) = register_test_session_at_with_outbound(
        &registry,
        "ModePvpAlice",
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, mut bob_rx) = register_test_session_at_with_outbound(
        &registry,
        "ModePvpBob",
        PlayerPose::new(0.5, 64.0, 2.5),
    );
    for session in [alice, bob] {
        register_test_player_state(&registry, session, PlayerInventory::empty());
    }
    while bob_rx.try_recv().is_ok() {}

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let alice_handle = handle.for_session(alice);
    let mut survival_target = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(survival_target.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        survival_target.await.expect("survival PvP owner response"),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
    ));
    while bob_rx.try_recv().is_ok() {}

    registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            bob,
            PlayerStateEvent::GameMode(GameMode::Adventure),
        )
        .expect("switch target to adventure");
    let mut adventure_target = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(adventure_target.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        adventure_target
            .await
            .expect("adventure PvP owner response"),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
    ));

    registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            bob,
            PlayerStateEvent::GameMode(GameMode::Creative),
        )
        .expect("switch target to creative");
    let mut creative_target = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(creative_target.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        creative_target.await.expect("creative PvP owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(bob_rx.try_recv().is_err());

    registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            bob,
            PlayerStateEvent::GameMode(GameMode::Spectator),
        )
        .expect("switch target to spectator");
    let mut spectator_target = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(spectator_target.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        spectator_target
            .await
            .expect("spectator PvP owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(bob_rx.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn player_melee_uses_fenced_attacker_identity_and_rejects_self() {
    let registry = SessionRegistry::new();
    let (alice, _alice_rx) = register_test_session_at_with_outbound(
        &registry,
        "IdentityPvpAlice",
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let (bob, mut bob_rx) = register_test_session_at_with_outbound(
        &registry,
        "IdentityPvpBob",
        PlayerPose::new(0.5, 64.0, 1.5),
    );
    for session in [alice, bob] {
        register_test_player_state(&registry, session, PlayerInventory::empty());
    }
    while bob_rx.try_recv().is_ok() {}

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let alice_handle = handle.for_session(alice);
    let mut valid = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(valid.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        valid.await.expect("valid player melee owner response"),
        PlayerAttackResult::Damaged(outcome)
            if matches!(*outcome, EntityAttackOutcome::PlayerDamaged { .. })
    ));
    while bob_rx.try_recv().is_ok() {}

    registry
        .commit_player_pose(
            &SimulationAuthority::for_test(),
            alice,
            PlayerPose::new(20.5, 64.0, 0.5),
            0.0,
        )
        .expect("move attacker away from target");
    let mut authoritative_pose = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(bob).unwrap()), 4.0),
    );
    assert_request_enqueued(authoritative_pose.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        authoritative_pose
            .await
            .expect("authoritative-pose player melee owner response"),
        PlayerAttackResult::ValidationRejected
    ));
    assert!(bob_rx.try_recv().is_err());

    let mut self_attack = Box::pin(
        alice_handle.player_attack_server_entity(EntityId(i32::try_from(alice).unwrap()), 4.0),
    );
    assert_request_enqueued(self_attack.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        self_attack.await.expect("self melee owner response"),
        PlayerAttackResult::ValidationRejected
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn world_lock_is_released_before_following_non_world_command() {
    let (storage, position, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "WorldBatchIsolation");
    let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let session_handle = handle.for_session(session);
    let mut world_request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    let mut inventory_request =
        Box::pin(session_handle.commit_player_inventory(empty_container_player_plan()));

    assert_request_enqueued(world_request.as_mut(), &handle).await;
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(inventory_request.as_mut(), cx).is_pending(),
            "inventory request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 2);

    let blocked_player = Arc::clone(&persisted);
    let (player_locked_tx, player_locked_rx) = tokio::sync::oneshot::channel();
    let (release_player_tx, release_player_rx) = std::sync::mpsc::channel();
    let player_blocker = std::thread::spawn(move || {
        let _guard = blocked_player.lock().unwrap();
        player_locked_tx.send(()).unwrap();
        release_player_rx.recv().unwrap();
    });
    player_locked_rx.await.unwrap();
    let owner_registry = Arc::clone(&registry);
    let owner_world = Arc::clone(&world);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world(&owner_registry, Some(&owner_world), None, 2)
            .await
    });

    assert!(world_request.await.unwrap().is_some());
    let world_is_available =
        match tokio::time::timeout(std::time::Duration::from_secs(1), world.lock()).await {
            Ok(storage) => {
                drop(storage);
                true
            }
            Err(_) => false,
        };

    release_player_tx.send(()).unwrap();
    player_blocker.join().unwrap();
    assert!(
        world_is_available,
        "non-world command must not retain the world lock"
    );
    assert!(matches!(
        inventory_request.await.unwrap(),
        PlayerInventoryCommitOutcome::Committed { .. }
    ));
    assert_eq!(owner_task.await.unwrap().processed, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packet_owner_relight_compute_and_publish_do_not_hold_world_writer() {
    let (storage, position, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "OwnerRelightWriterRelease");
    let table = Arc::new(BlockLightTable::from_arrays(
        "test",
        vec![0, 0, 0, 0, 15],
        vec![0, 15, 0, 15, 0],
        vec![true, false, true, false, true],
    ));
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: position,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: position,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_server_relight_compute_probe(reached_tx, resume_rx);
    let owner_registry = Arc::clone(&registry);
    let owner_world = Arc::clone(&world);
    let owner_table = Arc::clone(&table);
    let owner_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: None,
                    light: Some(&owner_table),
                },
                Some(&owner_table),
                1,
            ))
    });

    reached_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("packet owner relight reaches the compute boundary");
    let mut writer = world
        .try_lock()
        .expect("packet relight compute releases the world writer");
    writer
        .set_block_at(BlockPos { x: 2, y: 64, z: 2 }, BlockStateId(1))
        .unwrap();
    resume_tx.send(()).expect("release packet owner relight");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("packet relight publishes while the world writer remains held")
        .expect("packet owner relight response")
        .expect("block edit committed");
    drop(writer);
    let report = owner_thread.join().expect("packet owner relight joins");

    assert_eq!(report.processed, 1);
    assert_eq!(outcome.applied.len(), 1);
    let updates = outcome
        .precomputed_light_updates
        .expect("owner response includes published light");
    assert_eq!(updates.len(), 1);
    let storage = world
        .try_lock()
        .expect("owner relight released world writer");
    let current = storage
        .cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 })
        .expect("edited chunk remains resident");
    let mut refs = [[None; 3]; 3];
    refs[1][1] = Some(current.as_ref());
    let expected = mc_world::light::compute_chunk_light_in(
        &mut mc_world::light::LightWorkspace::new(),
        refs,
        &table,
    );
    assert_eq!(updates[0].light, expected);
    assert_eq!(
        mc_world::light::ChunkLight::from_section_lights(&current.section_lights),
        Some(expected)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn player_pose_commit_updates_session_and_persistence_in_one_owner_turn() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "PoseOwner");
    let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut pose = PlayerPose::new(7.5, 65.0, -3.5);
    pose.yaw = 91.0;
    pose.pitch = -12.0;
    pose.flags = mc_protocol::packets::play::MovePlayerFlags::new(true, false);
    pose.sprinting = true;
    let mut request = Box::pin(session_handle.commit_player_pose(pose, 0.25));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(
        request.await,
        Ok(CommittedPlayerPose {
            food: 20,
            saturation: 5.0,
            exhaustion: 0.25,
            resources_changed: false,
        })
    );

    let session_pose = registry.player_pose(session).expect("active player pose");
    assert_eq!(session_pose.x, pose.x);
    assert_eq!(session_pose.y, pose.y);
    assert_eq!(session_pose.z, pose.z);
    assert_eq!(session_pose.yaw, pose.yaw);
    assert_eq!(session_pose.pitch, pose.pitch);
    assert_eq!(session_pose.flags, pose.flags);
    assert_eq!(session_pose.sprinting, pose.sprinting);

    let persisted_pose = persisted.lock().unwrap().pose;
    assert_eq!(persisted_pose.x, pose.x);
    assert_eq!(persisted_pose.y, pose.y);
    assert_eq!(persisted_pose.z, pose.z);
    assert_eq!(persisted_pose.yaw, pose.yaw);
    assert_eq!(persisted_pose.pitch, pose.pitch);
    assert_eq!(persisted_pose.flags, pose.flags);
    assert_eq!(persisted_pose.sprinting, pose.sprinting);
    assert_eq!(persisted.lock().unwrap().survival.exhaustion, 0.25);

    let mut threshold = Box::pin(session_handle.commit_player_pose(pose, 3.75));
    assert_request_enqueued(threshold.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 2).processed, 1);
    assert_eq!(
        threshold.await,
        Ok(CommittedPlayerPose {
            food: 20,
            saturation: 4.0,
            exhaustion: 0.0,
            resources_changed: true,
        })
    );
    assert_eq!(persisted.lock().unwrap().survival.saturation, 4.0);

    let mut local = SurvivalState {
        health: 7.0,
        ..SurvivalState::FULL
    };
    CommittedPlayerPose {
        food: 19,
        saturation: 0.0,
        exhaustion: 1.5,
        resources_changed: true,
    }
    .apply_resources_to(&mut local);
    assert_eq!(local.health, 7.0);
    assert_eq!(local.food, 19);
    assert_eq!(local.saturation, 0.0);
    assert_eq!(local.exhaustion, 1.5);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_session_pose_commit_is_rejected_without_mutating_persistence() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StalePoseOwner");
    let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
    let original_pose = persisted.lock().unwrap().pose;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request =
        Box::pin(session_handle.commit_player_pose(PlayerPose::new(40.5, 70.0, 40.5), 0.25));

    assert_request_enqueued(request.as_mut(), &handle).await;
    let _ = registry.unregister(session);
    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::StaleSession)
    ));

    let persisted_pose = persisted.lock().unwrap().pose;
    assert_eq!(persisted_pose.x, original_pose.x);
    assert_eq!(persisted_pose.y, original_pose.y);
    assert_eq!(persisted_pose.z, original_pose.z);
}

#[tokio::test(flavor = "current_thread")]
async fn player_metadata_events_commit_through_the_simulation_owner() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "MetadataOwner");
    let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut respawn = PlayerPose::new(12.5, 70.0, -8.5);
    respawn.yaw = 135.0;

    let mut hotbar_request = Box::pin(session_handle.commit_selected_hotbar_slot(4));
    assert_request_enqueued(hotbar_request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(hotbar_request.await, Ok(()));

    let mut respawn_request = Box::pin(session_handle.commit_respawn_pose(respawn));
    assert_request_enqueued(respawn_request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(respawn_request.await, Ok(()));

    let mut game_mode_request = Box::pin(session_handle.commit_game_mode(GameMode::Creative));
    assert_request_enqueued(game_mode_request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(game_mode_request.await, Ok(()));

    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.selected_hotbar_slot, 4);
    assert_eq!(persisted.spawn.pose().x, respawn.x);
    assert_eq!(persisted.spawn.pose().y, respawn.y);
    assert_eq!(persisted.spawn.pose().z, respawn.z);
    assert_eq!(persisted.spawn.pose().yaw, respawn.yaw);
    assert_eq!(persisted.game_mode, GameMode::Creative);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_session_metadata_event_does_not_mutate_persistence() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleMetadataOwner");
    let persisted = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_selected_hotbar_slot(7));

    assert_request_enqueued(request.as_mut(), &handle).await;
    let _ = registry.unregister(session);
    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert_eq!(request.await, Err(SimulationRequestError::StaleSession));
    assert_eq!(persisted.lock().unwrap().selected_hotbar_slot, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn player_inventory_commit_updates_cursor_persistence_and_drop_in_one_owner_turn() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "InventoryOwner");
    let before_inventory = PlayerInventory::empty();
    let persisted = register_test_player_state(&registry, session, before_inventory.clone());
    let before_cursor = ItemStack::new(99, 2);
    persisted.lock().unwrap().carried_item = before_cursor.clone();
    let mut updated_inventory = before_inventory.clone();
    updated_inventory.slots[9] = ItemStack::new(42, 1);
    let updated_cursor = ItemStack::new(99, 1);
    let plan = ContainerPlayerPlan {
        expected_inventory: before_inventory,
        expected_carried_item: before_cursor,
        updated_inventory: updated_inventory.clone(),
        updated_carried_item: updated_cursor.clone(),
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops: vec![ContainerDropPlan {
            entity_type_id: 1,
            position: Vec3::new(0.5, 65.0, 0.5),
            stack: EntityItemStack::new(99, 1),
        }],
        xp_orb: None,
    };
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_player_inventory(plan));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let outcome = request.await.unwrap();

    assert!(matches!(
        outcome,
        PlayerInventoryCommitOutcome::Committed { ref inventory, ref carried_item, .. }
            if inventory.slots == updated_inventory.slots && carried_item == &updated_cursor
    ));
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.inventory.slots, updated_inventory.slots);
    assert_eq!(persisted.carried_item, updated_cursor);
    drop(persisted);
    assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_player_inventory_commit_has_one_winner_and_one_drop() {
    let registry = SessionRegistry::new();
    let (session, _outbound) =
        register_test_session_with_outbound(&registry, "DuplicateInventoryOwner");
    let before_inventory = PlayerInventory::empty();
    let persisted = register_test_player_state(&registry, session, before_inventory.clone());
    let before_cursor = ItemStack::new(77, 2);
    persisted.lock().unwrap().carried_item = before_cursor.clone();
    let mut updated_inventory = before_inventory.clone();
    updated_inventory.slots[9] = ItemStack::new(42, 1);
    let updated_cursor = ItemStack::new(77, 1);
    let plan = ContainerPlayerPlan {
        expected_inventory: before_inventory,
        expected_carried_item: before_cursor,
        updated_inventory: updated_inventory.clone(),
        updated_carried_item: updated_cursor.clone(),
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops: vec![ContainerDropPlan {
            entity_type_id: 1,
            position: Vec3::new(0.5, 65.0, 0.5),
            stack: EntityItemStack::new(77, 1),
        }],
        xp_orb: None,
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let session_handle = handle.for_session(session);
    let mut first = Box::pin(session_handle.commit_player_inventory(plan.clone()));
    let mut duplicate = Box::pin(session_handle.commit_player_inventory(plan));

    assert_request_enqueued(first.as_mut(), &handle).await;
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(duplicate.as_mut(), cx).is_pending(),
            "duplicate request must wait for the simulation owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 2);
    assert_eq!(owner.process_tick(&registry, 2).processed, 2);

    assert!(matches!(
        first.await.unwrap(),
        PlayerInventoryCommitOutcome::Committed { .. }
    ));
    assert!(matches!(
        duplicate.await.unwrap(),
        PlayerInventoryCommitOutcome::Rejected { ref inventory, ref carried_item, .. }
            if inventory.slots == updated_inventory.slots && carried_item == &updated_cursor
    ));
    assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_session_player_inventory_commit_is_rejected_without_mutation() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleInventoryOwner");
    let before_inventory = PlayerInventory::empty();
    let persisted = register_test_player_state(&registry, session, before_inventory.clone());
    let before_cursor = ItemStack::new(55, 1);
    persisted.lock().unwrap().carried_item = before_cursor.clone();
    let mut updated_inventory = before_inventory.clone();
    updated_inventory.slots[9] = ItemStack::new(42, 1);
    let plan = ContainerPlayerPlan {
        expected_inventory: before_inventory.clone(),
        expected_carried_item: before_cursor.clone(),
        updated_inventory,
        updated_carried_item: ItemStack::EMPTY,
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops: Vec::new(),
        xp_orb: None,
    };
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_player_inventory(plan));

    assert_request_enqueued(request.as_mut(), &handle).await;
    let _ = registry.unregister(session);
    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::StaleSession)
    ));

    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.inventory.slots, before_inventory.slots);
    assert_eq!(persisted.carried_item, before_cursor);
}

#[test]
fn default_simulation_channel_is_bounded() {
    let (handle, _owner) = simulation_channel();
    assert_eq!(handle.snapshot().capacity, 1024);
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn simulation_channel_rejects_zero_capacity() {
    assert!(std::panic::catch_unwind(|| simulation_channel_with_capacity(0)).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn command_arrival_wakes_owner_and_preserves_the_envelope() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut wake = Box::pin(owner.wait_for_command());
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(wake.as_mut(), cx).is_pending(),
            "empty command queue must keep the owner parked"
        );
        std::task::Poll::Ready(())
    })
    .await;

    let _response = handle.enqueue(claim_xp(1, 10)).expect("command fits");
    assert!(wake.as_mut().await, "command arrival must wake the owner");
    drop(wake);

    let batch = owner.drain_batch(1);
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].sequence, 0);
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn pushed_processing_defers_herd_burst_and_serves_later_gameplay_command() {
    const HERD_COMMANDS: usize = 40;
    const HERDS_PER_TICK: usize = 2;

    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(HERD_COMMANDS + 1);
    for index in 0..HERD_COMMANDS {
        handle
            .ensure_chunk_herd((index as i32, 0), Vec::new())
            .expect("herd command fits");
    }
    let gameplay = handle
        .enqueue(SimulationCommand::SpawnCommandEntity {
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(0.5, 64.0, 0.5),
        })
        .expect("gameplay command fits");

    assert!(owner.wait_for_command().await);
    let pushed = owner
        .process_ready_commands_with_world(&registry, None, None, 256)
        .await;
    assert_eq!(pushed.processed, 1);
    assert_eq!(pushed.remaining_depth, HERD_COMMANDS);
    assert!(matches!(
        gameplay.await.unwrap().unwrap(),
        SimulationResponse::EntitySpawn(_)
    ));

    let first_tick = owner
        .process_commands_with_world(&registry, None, None, 256)
        .await;
    assert_eq!(first_tick.processed, HERDS_PER_TICK);
    assert_eq!(first_tick.remaining_depth, HERD_COMMANDS - HERDS_PER_TICK);
}

#[tokio::test(flavor = "current_thread")]
async fn pushed_time_set_orders_earlier_and_later_herds_across_the_barrier() {
    let registry = SessionRegistry::new();
    let observer = register_test_session(&registry, "PushedTimeBarrierObserver");
    let earlier_chunk = (5, 0);
    let later_chunk = (6, 0);
    registry.mark_loaded(observer, earlier_chunk);
    registry.mark_loaded(observer, later_chunk);
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    owner.restore_world_time(&registry, super::super::NIGHT_START_TICK);
    let hostile_herd = |chunk, x| {
        vec![super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 5,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(x, 64.0, 8.5),
            hostile: true,
            sheep_color: None,
        }]
    };

    handle
        .ensure_chunk_herd(earlier_chunk, hostile_herd(earlier_chunk, 88.5))
        .expect("queue earlier night herd");
    let time_set = handle
        .for_session(observer)
        .enqueue(SimulationCommand::SetWorldTime { world_time: 0 })
        .expect("queue daytime barrier");
    handle
        .ensure_chunk_herd(later_chunk, hostile_herd(later_chunk, 104.5))
        .expect("queue later herd");

    assert!(owner.wait_for_command().await);
    let report = owner
        .process_ready_commands_with_world_views(
            &registry,
            None,
            SimulationWorldAccess::default(),
            None,
            256,
        )
        .await;

    assert_eq!(report.processed, 2);
    assert_eq!(report.remaining_depth, 1);
    assert!(matches!(
        time_set.await.expect("time set response"),
        Ok(SimulationResponse::WorldTimeSet)
    ));
    assert_eq!(registry.world_time(), 0);
    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].snapshot.position.x, 88.5);
    let pushed_metrics = handle.snapshot();
    assert_eq!(pushed_metrics.enqueued, 3);
    assert_eq!(pushed_metrics.depth, 1);
    assert_eq!(pushed_metrics.processed, 2);
    assert_eq!(pushed_metrics.max_batch, 2);

    let later = owner
        .process_commands_with_world_views(
            &registry,
            None,
            SimulationWorldAccess::default(),
            None,
            256,
        )
        .await;
    assert_eq!(later.processed, 1);
    assert_eq!(later.remaining_depth, 0);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    let drained_metrics = handle.snapshot();
    assert_eq!(drained_metrics.depth, 0);
    assert_eq!(drained_metrics.processed, 3);
    assert_eq!(drained_metrics.max_batch, 2);

    owner.advance_world_time(&registry, super::super::NIGHT_START_TICK);
    let mut positions = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.position.x)
        .collect::<Vec<_>>();
    positions.sort_by(f64::total_cmp);
    assert_eq!(positions, [88.5, 104.5]);
}

#[tokio::test(flavor = "current_thread")]
async fn unbound_handle_rejects_player_command_before_enqueue() {
    let (handle, _owner) = simulation_channel();

    assert!(matches!(
        handle
            .pickup_item_into_inventory(EntityId(1), 42, None, Vec::new(), 64)
            .await,
        Err(SimulationRequestError::InvalidCommand)
    ));
    assert_eq!(handle.snapshot().enqueued, 0);
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn full_simulation_channel_rejects_without_growing_depth() {
    let (handle, _owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let error = handle
        .enqueue(claim_xp(2, 11))
        .expect_err("second command must fail closed");

    assert_eq!(error, SimulationRequestError::Full);
    assert_eq!(handle.snapshot().depth, 1);
    assert_eq!(handle.snapshot().rejected_full, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_sender_is_closed_when_owner_drops() {
    let (handle, owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let session_handle = handle.for_session(7);
    let mut waiting = Box::pin(
        session_handle
            .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
    );

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the sender until capacity or closure"
        );
        std::task::Poll::Ready(())
    })
    .await;

    drop(owner);

    assert!(matches!(
        waiting.await,
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_detached_sender_is_closed_when_owner_shuts_down() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let mut waiting =
        Box::pin(handle.enqueue_detached_wait(SimulationCommand::SetWorldTime { world_time: 1 }));

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the detached sender until capacity or closure"
        );
        std::task::Poll::Ready(())
    })
    .await;

    owner.shutdown();

    assert!(matches!(
        waiting.await,
        Err(SimulationRequestError::ShuttingDown)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test]
async fn owner_drop_rejects_pending_response_and_closes_queue() {
    let (handle, owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

    drop(owner);

    assert!(matches!(
        response.await.expect("owner drop response"),
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert!(matches!(
        handle.enqueue(claim_xp(2, 11)),
        Err(SimulationRequestError::OwnerStopped)
    ));
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn full_simulation_channel_rejects_attack_without_damage() {
    let registry = SessionRegistry::new();
    let target = seed_attack_target(&registry);
    let (handle, _owner) = simulation_channel_with_capacity(1);
    let _occupant = handle.enqueue(claim_xp(1, 10)).expect("first command fits");

    let error = handle
        .enqueue(SimulationCommand::AttackServerEntity {
            entity_id: target,
            damage: 20.0,
            knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
            rewards: EntityKillRewards::default(),
        })
        .expect_err("attack must fail closed while queue is full");
    let health = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == target)
        .expect("target remains")
        .snapshot
        .health;

    assert_eq!(error, SimulationRequestError::Full);
    assert_eq!(health, 20.0);
    assert_eq!(handle.snapshot().rejected_full, 1);
}

#[test]
fn owner_drains_commands_in_monotonic_sequence_order() {
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let _first = handle.enqueue(claim_xp(1, 10)).unwrap();
    let _second = handle.enqueue(claim_xp(2, 11)).unwrap();

    let batch = owner.drain_batch(2);

    assert_eq!(
        batch
            .iter()
            .map(|command| command.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(handle.snapshot().depth, 0);
    assert_eq!(handle.snapshot().dequeued, 2);
    assert_eq!(handle.snapshot().max_batch, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn waiting_sender_is_admitted_after_earlier_command_drains() {
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let _first = handle.enqueue(claim_xp(1, 10)).expect("first command fits");
    let session_handle = handle.for_session(7);
    let mut waiting = Box::pin(
        session_handle
            .enqueue_player_command_wait(SimulationCommand::SetWorldTime { world_time: 1 }),
    );

    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
            "full queue must hold the later sender"
        );
        std::task::Poll::Ready(())
    })
    .await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let _response = waiting.await.expect("capacity release admits sender");
    let batch = owner.drain_batch(1);

    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].sequence, 1);
}

#[tokio::test]
async fn zero_budget_leaves_queued_command_unprocessed() {
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).expect("command fits");

    let report = owner.process_tick(&registry, 0);

    assert_eq!(report.processed, 0);
    assert_eq!(report.remaining_depth, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), response)
            .await
            .is_err()
    );
    assert_eq!(handle.snapshot().max_batch, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_drains_prefetched_deferred_and_receiver_commands() {
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    handle
        .ensure_chunk_herd((1, 1), Vec::new())
        .expect("background herd command fits");
    assert!(owner.drain_ready_batch(1).is_empty());

    let prefetched = handle
        .enqueue(claim_xp(1, 10))
        .expect("prefetched command fits");
    assert!(owner.wait_for_command().await);
    let queued = handle
        .enqueue(claim_xp(2, 11))
        .expect("receiver command fits");

    owner.shutdown();

    assert!(matches!(
        prefetched.await.expect("prefetched shutdown response"),
        Err(SimulationRequestError::ShuttingDown)
    ));
    assert!(matches!(
        queued.await.expect("receiver shutdown response"),
        Err(SimulationRequestError::ShuttingDown)
    ));
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.depth, 0);
    assert_eq!(snapshot.rejected_shutdown, 3);
    assert_eq!(snapshot.dequeued, 3);
}

#[test]
fn cancelled_request_is_removed_before_owner_application() {
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle.enqueue(claim_xp(1, 10)).unwrap();
    drop(response);

    assert!(owner.drain_batch(1).is_empty());
    assert_eq!(handle.snapshot().cancelled, 1);
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn stale_session_claim_is_rejected_and_reconnect_can_claim() {
    let registry = SessionRegistry::new();
    let old_session = register_test_session(&registry, "FenceAlice");
    let old_player_state =
        register_test_player_state(&registry, old_session, PlayerInventory::empty());
    let (item, _) = seed_claim_entities(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let old_handle = handle.for_session(old_session);
    let mut stale_request =
        Box::pin(old_handle.pickup_item_into_inventory(item, 42, None, Vec::new(), 64));
    assert_request_enqueued(stale_request.as_mut(), &handle).await;

    registry.unregister(old_session);
    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        stale_request.await,
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );
    assert!(
        old_player_state.lock().unwrap().inventory.slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(handle.snapshot().rejected_stale_session, 1);

    let new_session = register_test_session(&registry, "FenceAlice");
    assert_ne!(new_session, old_session);
    let new_player_state =
        register_test_player_state(&registry, new_session, PlayerInventory::empty());
    let new_handle = handle.for_session(new_session);
    let mut fresh_request =
        Box::pin(new_handle.pickup_item_into_inventory(item, 42, None, Vec::new(), 64));
    assert_request_enqueued(fresh_request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(fresh_request.await.unwrap().unwrap().credited.count, 3);
    assert_eq!(
        new_player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 3)
    );
    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
}

#[test]
fn item_pickup_credit_survives_requester_loss_after_owner_apply() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "PickupCreditAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);
    let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
    registry.unregister(session);

    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 3)
    );
    assert!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
    assert!(
        outbound
            .iter()
            .any(|command| matches!(command, OutboundCommand::TakeItemEntity { amount: 3, .. }))
    );
    assert!(outbound.iter().any(
        |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == item)
    ));
}

#[tokio::test]
async fn item_pickup_owner_wait_releases_session_and_player_locks() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let registry = Arc::new(SessionRegistry::new());
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "PickupLockAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    registry.install_item_pickup_owner_probe_for_test(entered_tx, release_rx);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");
    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

    entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("item pickup owner boundary reached");
    assert!(
        player_state.try_lock().is_ok(),
        "regional item pickup wait must not hold player persistence"
    );
    let progress_registry = Arc::clone(&registry);
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let progress = std::thread::spawn(move || {
        let dispatches = progress_registry.mark_loaded(session, (1, 0));
        progress_tx.send(()).expect("publish session progress");
        dispatches
    });
    progress_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("session registry must progress during regional item pickup wait");

    release_tx.send(()).expect("release regional item pickup");
    assert_eq!(owner_thread.join().expect("owner worker").processed, 1);
    assert!(progress.join().expect("session progress worker").is_empty());
    let SimulationResponse::ItemPickupCredit(Some(outcome)) = response.await.unwrap().unwrap()
    else {
        panic!("pickup credit response kind changed");
    };
    assert_eq!(outcome.credited.count, 1);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
}

#[tokio::test]
async fn item_pickup_claim_and_finalize_are_checkpoint_only() {
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: None,
            commits: Arc::clone(&commits),
        }),
    );
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "PickupCheckpointOnly");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let durable_before = commits.load(Ordering::Relaxed);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let SimulationResponse::ItemPickupCredit(Some(outcome)) = response.await.unwrap().unwrap()
    else {
        panic!("pickup credit response kind changed");
    };

    assert_eq!(outcome.credited.count, 1);
    assert_eq!(commits.load(Ordering::Relaxed), durable_before);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 2);

    let checkpoint = registry.persisted_entity_save_snapshot().0;
    let saved_item = checkpoint
        .records
        .iter()
        .find(|record| record.snapshot.id == item)
        .expect("save barrier captures the remaining item");
    assert_eq!(saved_item.snapshot.item_stack.as_ref().unwrap().count, 2);
    assert_eq!(saved_item.snapshot.retained.item_pickup_claim, None);
    let saved_players = registry.persisted_player_states();
    assert_eq!(saved_players.len(), 1);
    assert_eq!(
        saved_players[0].1.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    assert_eq!(commits.load(Ordering::Relaxed), durable_before);
}

#[tokio::test]
async fn item_moved_after_claim_is_revalidated_before_inventory_credit() {
    let (claimed_tx, claimed_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let registry = Arc::new(SessionRegistry::new());
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "PickupRollbackMotion");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    registry.install_item_pickup_claimed_probe_for_test(claimed_tx, resume_rx);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");
    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

    claimed_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("pickup installs its owner claim");
    let moved_position = Vec3::new(4.0, 64.0, 0.5);
    assert!(registry.relocate_claimed_item_for_test(item, moved_position));
    resume_tx.send(()).expect("release claimed pickup");

    assert_eq!(owner_thread.join().expect("owner worker").processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 63)
    );
    let remaining = registry.nearby_item_entities(moved_position, 0.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].position, moved_position);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn moving_out_of_overlap_after_item_claim_rolls_back_before_credit() {
    let (claimed_tx, claimed_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let registry = Arc::new(SessionRegistry::new());
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "PickupMoveAwayAfterClaim");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    registry.install_item_pickup_claimed_probe_for_test(claimed_tx, resume_rx);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");
    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

    claimed_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("pickup installs its owner claim");
    registry.update_pose(session, PlayerPose::new(4.0, 64.0, 0.5));
    resume_tx.send(()).expect("release claimed pickup");

    assert_eq!(owner_thread.join().expect("owner worker").processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 63)
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 0.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn stale_player_inventory_after_pickup_plan_restores_entity_without_publication() {
    let registry = Arc::new(SessionRegistry::new());
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "StalePickupPlayerInventory");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory.clone());
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let (plan_reached_tx, plan_reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_item_pickup_plan_probe_for_test(plan_reached_tx, resume_rx);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");
    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

    plan_reached_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("pickup reaches player/entity claim boundary");
    player_state.lock().unwrap().inventory.slots[9] = ItemStack::new(7, 1);
    resume_tx.send(()).expect("release pickup owner claim");
    assert_eq!(owner_thread.join().expect("owner worker").processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));

    let persisted = player_state.lock().unwrap();
    assert_eq!(persisted.inventory.slots[9], ItemStack::new(7, 1));
    assert_eq!(
        persisted.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 63)
    );
    drop(persisted);
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn item_pickup_credit_is_conservative_under_partial_capacity() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "PickupCreditBob");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let SimulationResponse::ItemPickupCredit(Some(outcome)) = response.await.unwrap().unwrap()
    else {
        panic!("pickup credit response kind changed");
    };

    assert_eq!(outcome.credited.count, 1);
    assert_eq!(
        outcome.changed_slots,
        vec![(PlayerInventory::HOTBAR_BASE, ItemStack::new(42, 64))]
    );
    assert_eq!(
        outcome.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 2);
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::UpdateEntityData(snapshot))
            if snapshot.id == item
                && snapshot.item_stack.as_ref().is_some_and(|stack| stack.count == 2)
    ));
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn full_inventory_rejects_pickup_without_removing_entity() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "PickupCreditCarol");
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    let player_state = register_test_player_state(&registry, session, inventory);
    let (item, _) = seed_claim_entities(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let SimulationResponse::ItemPickupCredit(outcome) = response.await.unwrap().unwrap() else {
        panic!("pickup credit response kind changed");
    };

    assert!(outcome.is_none());
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
}

#[tokio::test]
async fn invalid_hotbar_rejects_pickup_without_inventory_or_entity_publication() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "InvalidHotbarPickup");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let inventory = PlayerInventory::empty();
    let player_state = register_test_player_state(&registry, session, inventory.clone());
    player_state.lock().unwrap().selected_hotbar_slot = 9;
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));

    assert_eq!(
        player_state.lock().unwrap().inventory.slots,
        inventory.slots
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].item_stack.as_ref().unwrap().count, 3);
    assert!(outbound.try_recv().is_err());
}

#[tokio::test]
async fn stale_item_stack_after_pickup_plan_preserves_inventory_entity_and_publication() {
    let registry = Arc::new(SessionRegistry::new());
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "StalePickupItemStack");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 63);
    let player_state = register_test_player_state(&registry, session, inventory.clone());
    let (item, _) = seed_claim_entities_published(&registry, &mut outbound);
    let (plan_reached_tx, plan_reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    registry.install_item_pickup_plan_probe_for_test(plan_reached_tx, resume_rx);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .expect("pickup command fits");
    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || owner.process_tick(&owner_registry, 1));

    plan_reached_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("pickup reaches the post-plan CAS fence");
    let replacement = EntityItemStack {
        item_id: 42,
        count: 3,
        damage: Some(7),
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    assert!(registry.replace_item_stack_after_pickup_plan_for_test(item, replacement.clone()));
    resume_tx.send(()).expect("release pickup CAS");
    assert_eq!(owner_thread.join().unwrap().processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));

    assert_eq!(
        player_state.lock().unwrap().inventory.slots,
        inventory.slots
    );
    let remaining = registry.nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, item);
    assert_eq!(remaining[0].item_stack.as_ref(), Some(&replacement));
    assert!(outbound.try_recv().is_err());
}

async fn assert_player_state_cannot_pick_up(
    player_name: &str,
    game_mode: GameMode,
    survival: SurvivalState,
) {
    let registry = SessionRegistry::new();
    let (item, experience) = seed_claim_entities(&registry);
    let arrow = seed_grounded_arrow(&registry);
    let session = register_test_session(&registry, player_name);
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    {
        let mut state = player_state.lock().unwrap();
        state.game_mode = game_mode;
        state.survival = survival;
    }
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    let item_response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupItemIntoInventory {
            entity_id: item,
            collector_session: session,
            expected_item_id: 42,
            expected_damage: None,
            expected_enchantments: Vec::new(),
            max_stack: 64,
        })
        .unwrap();
    let experience_response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
            entity_id: experience,
            collector_session: session,
        })
        .unwrap();
    let arrow_response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: session,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 3).processed, 3);
    assert!(matches!(
        item_response.await.unwrap().unwrap(),
        SimulationResponse::ItemPickupCredit(None)
    ));
    assert!(matches!(
        experience_response.await.unwrap().unwrap(),
        SimulationResponse::ExperiencePickupCredit(None)
    ));
    assert!(matches!(
        arrow_response.await.unwrap().unwrap(),
        SimulationResponse::ArrowPickupCredit(None)
    ));

    let state = player_state.lock().unwrap();
    assert!(state.inventory.slots.iter().all(ItemStack::is_empty));
    assert_eq!(state.xp.total, 0);
    drop(state);
    assert_eq!(
        registry
            .nearby_item_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );
    assert_eq!(
        registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );
    assert!(registry.server_entity_snapshot(arrow).is_some());
}

#[tokio::test]
async fn spectator_cannot_receive_item_arrow_or_experience_pickup_credit() {
    assert_player_state_cannot_pick_up("SpectatorPickup", GameMode::Spectator, SurvivalState::FULL)
        .await;
}

#[tokio::test]
async fn dead_player_cannot_receive_item_arrow_or_experience_pickup_credit() {
    assert_player_state_cannot_pick_up(
        "DeadPickup",
        GameMode::Survival,
        SurvivalState {
            health: 0.0,
            ..SurvivalState::FULL
        },
    )
    .await;
}

#[test]
fn experience_credit_survives_requester_loss_after_owner_apply() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "XpCreditAlice");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    player_state.lock().unwrap().xp = XpState {
        level: 1,
        progress: 3.0 / 9.0,
        total: 10,
        seed: 17,
    };
    let (_, experience) = seed_claim_entities_published(&registry, &mut outbound);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
            entity_id: experience,
            collector_session: session,
        })
        .expect("experience command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);
    let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
    registry.unregister(session);

    let saved = player_state.lock().unwrap();
    assert_eq!(saved.xp.total, 15);
    assert_eq!(saved.xp.level, 1);
    assert!((saved.xp.progress - (8.0 / 9.0)).abs() < f32::EPSILON);
    assert_eq!(saved.xp.seed, 17);
    assert!(
        registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
    assert!(
        outbound
            .iter()
            .any(|command| matches!(command, OutboundCommand::TakeItemEntity { amount: 5, .. }))
    );
    assert!(outbound.iter().any(
            |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == experience)
        ));
}

#[tokio::test]
async fn concurrent_experience_credit_has_one_exact_winner() {
    let registry = SessionRegistry::new();
    let alice = register_test_session(&registry, "XpCreditBob");
    let bob = register_test_session(&registry, "XpCreditCarol");
    let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
    let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
    let (_, experience) = seed_claim_entities(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let alice_response = handle
        .for_session(alice)
        .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
            entity_id: experience,
            collector_session: alice,
        })
        .unwrap();
    let bob_response = handle
        .for_session(bob)
        .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
            entity_id: experience,
            collector_session: bob,
        })
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let mut outcomes = Vec::with_capacity(2);
    for response in [alice_response, bob_response] {
        let SimulationResponse::ExperiencePickupCredit(outcome) = response.await.unwrap().unwrap()
        else {
            panic!("experience credit response kind changed");
        };
        outcomes.push(outcome);
    }

    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_some()).count(),
        1
    );
    assert_eq!(
        alice_state.lock().unwrap().xp.total + bob_state.lock().unwrap().xp.total,
        5
    );
    assert!(
        registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .is_empty()
    );
}

#[tokio::test]
async fn stale_session_experience_credit_cannot_remove_orb_or_change_xp() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "XpCreditStale");
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (_, experience) = seed_claim_entities(&registry);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupExperienceIntoPlayer {
            entity_id: experience,
            collector_session: session,
        })
        .unwrap();
    registry.unregister(session);

    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(player_state.lock().unwrap().xp.total, 0);
    assert_eq!(
        registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );
}

#[test]
fn arrow_credit_survives_requester_loss_after_owner_apply() {
    let registry = SessionRegistry::new();
    let arrow = seed_grounded_arrow(&registry);
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "ArrowCreditAlice");
    let spawn = registry.mark_loaded(session, (0, 0));
    assert!(spawn.iter().any(|dispatch| matches!(
        &dispatch.command,
        OutboundCommand::SpawnEntity(entity) if entity.id == arrow
    )));
    dispatch_visibility_commands(spawn);
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == arrow
    ));
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: session,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .expect("arrow command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);
    let outbound = [outbound.try_recv().unwrap(), outbound.try_recv().unwrap()];
    registry.unregister(session);

    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert!(registry.server_entity_snapshot(arrow).is_none());
    assert!(
        outbound
            .iter()
            .any(|command| matches!(command, OutboundCommand::TakeItemEntity { amount: 1, .. }))
    );
    assert!(outbound.iter().any(
        |command| matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == arrow)
    ));
}

#[tokio::test]
async fn full_inventory_rejects_arrow_pickup_without_removal() {
    let registry = SessionRegistry::new();
    let arrow = seed_grounded_arrow(&registry);
    let session = register_test_session(&registry, "ArrowCreditFull");
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(42, 64);
    }
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: session,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let SimulationResponse::ArrowPickupCredit(outcome) = response.await.unwrap().unwrap() else {
        panic!("arrow credit response kind changed");
    };

    assert!(outcome.is_none());
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 64)
    );
    assert!(registry.server_entity_snapshot(arrow).is_some());
}

#[tokio::test]
async fn concurrent_arrow_credit_has_one_exact_winner() {
    let registry = SessionRegistry::new();
    let arrow = seed_grounded_arrow(&registry);
    let alice = register_test_session(&registry, "ArrowCreditBob");
    let bob = register_test_session(&registry, "ArrowCreditCarol");
    let alice_state = register_test_player_state(&registry, alice, PlayerInventory::empty());
    let bob_state = register_test_player_state(&registry, bob, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let alice_response = handle
        .for_session(alice)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: alice,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .unwrap();
    let bob_response = handle
        .for_session(bob)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: bob,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let mut outcomes = Vec::with_capacity(2);
    for response in [alice_response, bob_response] {
        let SimulationResponse::ArrowPickupCredit(outcome) = response.await.unwrap().unwrap()
        else {
            panic!("arrow credit response kind changed");
        };
        outcomes.push(outcome);
    }

    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_some()).count(),
        1
    );
    let credited = |state: &Arc<Mutex<PlayerPersistedState>>| {
        state.lock().unwrap().inventory.slots[9..=44]
            .iter()
            .filter(|stack| stack.item_id == 42)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    assert_eq!(credited(&alice_state) + credited(&bob_state), 1);
    assert!(registry.server_entity_snapshot(arrow).is_none());
}

#[tokio::test]
async fn stale_session_arrow_credit_cannot_remove_or_credit() {
    let registry = SessionRegistry::new();
    let arrow = seed_grounded_arrow(&registry);
    let session = register_test_session(&registry, "ArrowCreditStale");
    let player_state = register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::PickupArrowIntoInventory {
            entity_id: arrow,
            collector_session: session,
            arrow_item_id: 42,
            max_stack: 64,
        })
        .unwrap();
    registry.unregister(session);

    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert!(
        player_state.lock().unwrap().inventory.slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(registry.server_entity_snapshot(arrow).is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn stale_session_block_edit_cannot_mutate_world() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "FenceBuilder");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    registry.unregister(session);

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        0
    );
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(handle.snapshot().rejected_stale_session, 1);
}

#[tokio::test]
async fn queued_duplicate_lethal_attack_does_not_duplicate_rewards() {
    let registry = SessionRegistry::new();
    let target = seed_attack_target(&registry);
    let rewards = EntityKillRewards {
        items: vec![(5, EntityItemStack::new(42, 1))],
        experience: Some((6, 5)),
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let lethal_response = handle
        .enqueue(SimulationCommand::AttackServerEntity {
            entity_id: target,
            damage: 20.0,
            knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
            rewards: rewards.clone(),
        })
        .unwrap();
    let duplicate_response = handle
        .enqueue(SimulationCommand::AttackServerEntity {
            entity_id: target,
            damage: 20.0,
            knockback_origin: Some(Vec3::new(0.5, 64.0, 0.5)),
            rewards,
        })
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let killed = match lethal_response.await.unwrap().unwrap() {
        SimulationResponse::EntityAttack(Some(outcome)) => match *outcome {
            EntityAttackOutcome::Killed {
                damage,
                entity,
                dispatches: _,
                ..
            } => (damage, entity),
            other => panic!("expected lethal entity attack outcome, got {other:?}"),
        },
        other => panic!("expected lethal entity attack response, got {other:?}"),
    };
    assert!(matches!(
        duplicate_response.await.unwrap().unwrap(),
        SimulationResponse::EntityAttack(None)
    ));

    assert_eq!(killed.0.snapshot.health, 0.0);
    assert_eq!(killed.1.type_name, "minecraft:zombie");
    assert!(registry.server_entity_snapshot(target).is_some());
    assert_eq!(
        registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.item_stack.is_some())
            .count(),
        1
    );
    assert_eq!(
        registry
            .nearby_experience_entities(killed.1.position, 2.25)
            .len(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn script_entity_spawn_is_session_fenced_visible_and_saved() {
    let registry = SessionRegistry::new();
    let (actor, mut actor_rx) = register_test_session_with_outbound(&registry, "ScriptSpawner");
    registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.mark_loaded(actor, (0, 0));
    let (handle, mut owner) = simulation_channel_with_capacity(2);

    let mut spawn = Box::pin(handle.spawn_script_entity(
        actor,
        90,
        "minecraft:pig".to_owned(),
        Vec3::new(2.5, 64.0, 1.5),
    ));
    assert_request_enqueued(spawn.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(spawn.await, Ok(()));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), actor_rx.recv())
            .await
            .expect("entity spawn dispatch was not delivered"),
        Some(OutboundCommand::SpawnEntity(_))
    ));

    let mut barrier = Box::pin(handle.save_barrier(false));
    assert_request_enqueued(barrier.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let snapshot = barrier.await.unwrap();
    assert!(snapshot.entities.records.iter().any(|entity| {
        entity.snapshot.type_name == "minecraft:pig"
            && entity.snapshot.position == Vec3::new(2.5, 64.0, 1.5)
    }));

    let stale = register_test_session(&registry, "StaleScriptSpawner");
    let mut stale_spawn = Box::pin(handle.spawn_script_entity(
        stale,
        90,
        "minecraft:pig".to_owned(),
        Vec3::new(3.5, 64.0, 1.5),
    ));
    assert_request_enqueued(stale_spawn.as_mut(), &handle).await;
    registry.unregister(stale);
    let stale_report = owner.process_tick(&registry, 1);
    assert_eq!(stale_report.processed, 0);
    assert_eq!(stale_report.remaining_depth, 0);
    assert_eq!(stale_spawn.await, Err(SimulationRequestError::StaleSession));
    assert_eq!(
        registry
            .persisted_entity_records()
            .into_iter()
            .filter(|entity| entity.snapshot.type_name == "minecraft:pig")
            .count(),
        1
    );
}

#[tokio::test]
async fn single_lane_region_routes_preserve_sequence_and_spawn_outcome() {
    let positions = [Vec3::new(-0.5, 64.0, 0.5), Vec3::new(128.5, 64.0, 0.5)];
    let routed = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = positions.map(|position| {
        handle
            .enqueue(SimulationCommand::SpawnCommandEntity {
                entity_type_id: 4,
                entity_type_name: "minecraft:zombie".to_owned(),
                position,
            })
            .expect("regional spawn command fits")
    });

    assert_eq!(owner.process_tick(&routed, 2).processed, 2);
    let routes = owner.last_region_routes();
    assert_eq!(routes.len(), 2);
    assert!(routes[0].sequence < routes[1].sequence);
    assert_eq!(routes[0].lease.key, mc_entity::RegionKey::new(-1, 0));
    assert_eq!(routes[1].lease.key, mc_entity::RegionKey::new(1, 0));
    assert!(routes.iter().all(|route| route.lease.lane == 0));
    assert!(
        routes
            .iter()
            .all(|route| route.lease.epoch == mc_entity::RegionEpoch::INITIAL)
    );

    let mut routed_dispatch_counts = Vec::with_capacity(responses.len());
    for response in responses {
        let count = match response.await.unwrap().unwrap() {
            SimulationResponse::EntitySpawn(dispatches) => dispatches.len(),
            other => panic!("expected regional entity spawn response, got {other:?}"),
        };
        routed_dispatch_counts.push(count);
    }
    assert_eq!(routed_dispatch_counts, vec![0, 0]);

    let snapshots = routed.persisted_entity_records();
    assert_eq!(snapshots.len(), 2);
    assert!(
        snapshots
            .iter()
            .all(|record| record.snapshot.type_name == "minecraft:zombie")
    );
    assert!(positions.iter().all(|position| {
        snapshots
            .iter()
            .any(|record| record.snapshot.position == *position)
    }));

    let next_phase = owner
        .region_ownership
        .begin_phase()
        .expect("routed batch closed its exact phase");
    owner
        .region_ownership
        .acknowledge_lane(next_phase, 0)
        .expect("lane 0 completes test phase");
    owner
        .region_ownership
        .finish_phase(next_phase)
        .expect("test phase closes");
}

#[test]
fn block_edit_routes_only_when_every_world_position_has_one_region_owner() {
    let inside = BlockPos { x: 1, y: 64, z: 1 };
    let same_region = BlockPos {
        x: 7 * 16 + 15,
        y: 64,
        z: 1,
    };
    let other_region = BlockPos {
        x: 8 * 16,
        y: 64,
        z: 1,
    };
    let token = BlockMutationToken {
        chunk_instance_id: 1,
        version: 2,
        last_replacement_version: 0,
    };
    let command = |precondition_pos, tick_pos| SimulationCommand::ApplyBlockEdits {
        actor_session: Some(1),
        edits: vec![
            BlockEdit {
                pos: inside,
                new_state: BlockStateId(1),
            },
            BlockEdit {
                pos: same_region,
                new_state: BlockStateId(1),
            },
        ],
        preconditions: vec![BlockEditPrecondition {
            pos: precondition_pos,
            expected_state: BlockStateId(0),
            expected_token: token,
        }],
        scheduled_block_ticks: vec![ScheduledBlockTick::new(
            tick_pos,
            Identifier::parse("minecraft:stone").unwrap(),
            5,
            0,
        )],
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };

    assert_eq!(
        command_single_owner_region(&command(inside, same_region)),
        Some(RegionKey::new(0, 0))
    );
    assert_eq!(
        command_single_owner_region(&command(other_region, same_region)),
        None
    );
    assert_eq!(
        command_single_owner_region(&command(inside, other_region)),
        None
    );
    let mut cross_region_edit = command(inside, same_region);
    let SimulationCommand::ApplyBlockEdits { edits, .. } = &mut cross_region_edit else {
        unreachable!();
    };
    edits.push(BlockEdit {
        pos: other_region,
        new_state: BlockStateId(1),
    });
    assert_eq!(command_single_owner_region(&cross_region_edit), None);
}

#[tokio::test]
async fn active_region_phase_rejects_routed_command_without_mutation() {
    let registry = SessionRegistry::new();
    let position = Vec3::new(0.5, 64.0, 0.5);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let active_phase = owner
        .region_ownership
        .begin_phase()
        .expect("occupy regional phase");
    let response = handle
        .enqueue(SimulationCommand::SpawnCommandEntity {
            entity_type_id: 4,
            entity_type_name: "minecraft:zombie".to_owned(),
            position,
        })
        .expect("spawn command fits");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        response.await.expect("owner response"),
        Err(SimulationRequestError::InvalidCommand)
    ));
    assert!(registry.nearby_hostile_entities(position, 2.25).is_empty());
    owner
        .region_ownership
        .finish_phase(active_phase)
        .expect("release occupied phase");
}

#[tokio::test(flavor = "current_thread")]
async fn barrier_completes_after_every_earlier_command() {
    let registry = SessionRegistry::new();
    let position = Vec3::new(1.5, 64.0, 0.5);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let spawn = handle
        .enqueue(SimulationCommand::SpawnCommandEntity {
            entity_type_id: 4,
            entity_type_name: "minecraft:zombie".to_owned(),
            position,
        })
        .unwrap();
    let mut barrier = Box::pin(handle.save_barrier(false));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(barrier.as_mut(), cx).is_pending(),
            "barrier must wait for its owner response"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(handle.snapshot().depth, 2);

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        spawn.await.unwrap().unwrap(),
        SimulationResponse::EntitySpawn(_)
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(barrier.as_mut(), cx).is_pending(),
            "barrier must remain pending until its own ordered command runs"
        );
        std::task::Poll::Ready(())
    })
    .await;
    assert_eq!(registry.nearby_hostile_entities(position, 2.25).len(), 1);

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let snapshot = barrier.await.expect("save barrier snapshot");
    assert_eq!(snapshot.entities.records.len(), 1);
    assert_eq!(snapshot.entities.records[0].type_name, "minecraft:zombie");
}

#[test]
fn owner_tick_phase_controls_entity_lifecycle_expiry() {
    let registry = SessionRegistry::new();
    let position = Vec3::new(0.5, 64.0, 0.5);
    registry.spawn_item_drop(1, position, EntityItemStack::new(42, 1));
    let (_handle, owner) = simulation_channel_with_capacity(1);

    assert_eq!(
        owner.advance_world_time(&registry, super::super::ITEM_DESPAWN_AGE_TICKS),
        super::super::ITEM_DESPAWN_AGE_TICKS
    );
    registry.apply_entity_physics_and_dispatch(super::super::ITEM_DESPAWN_AGE_TICKS, &[]);

    assert!(registry.nearby_item_entities(position, 2.25).is_empty());
}

#[tokio::test]
async fn queued_chunk_herd_spawn_deduplicates_chunk() {
    let chunk = (1, 1);
    let position = Vec3::new(24.5, 64.0, 24.5);
    let spawns = vec![super::super::HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 4,
        entity_type_name: "minecraft:cow".to_owned(),
        position,
        hostile: false,
        sheep_color: None,
    }];
    let queued = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let first = handle
        .enqueue(SimulationCommand::EnsureChunkHerd {
            chunk,
            spawns: spawns.clone(),
        })
        .unwrap();
    let duplicate = handle
        .enqueue(SimulationCommand::EnsureChunkHerd { chunk, spawns })
        .unwrap();

    assert_eq!(owner.process_tick(&queued, 2).processed, 2);
    let first_dispatches = match first.await.unwrap().unwrap() {
        SimulationResponse::EntitySpawn(dispatches) => dispatches,
        other => panic!("expected herd spawn response, got {other:?}"),
    };
    let duplicate_dispatches = match duplicate.await.unwrap().unwrap() {
        SimulationResponse::EntitySpawn(dispatches) => dispatches,
        other => panic!("expected herd dedupe response, got {other:?}"),
    };

    assert!(first_dispatches.is_empty());
    assert!(duplicate_dispatches.is_empty());
    let persisted = queued.persisted_entity_records();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].snapshot.type_name, "minecraft:cow");
    assert_eq!(persisted[0].snapshot.position, position);
}

#[test]
fn detached_chunk_herds_share_one_owner_batch() {
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    for (chunk, position) in [
        ((1, 1), Vec3::new(24.5, 64.0, 24.5)),
        ((2, 1), Vec3::new(40.5, 64.0, 24.5)),
    ] {
        handle
            .ensure_chunk_herd(
                chunk,
                vec![super::super::HerdSpawn {
                    chunk,
                    slot: 0,
                    entity_type_id: 4,
                    entity_type_name: "minecraft:cow".to_owned(),
                    position,
                    hostile: false,
                    sheep_color: None,
                }],
            )
            .expect("queue detached herd");
    }
    registry.reset_entity_owner_requests_for_test();

    let report = owner.process_tick(&registry, 2);

    assert_eq!(report.processed, 2);
    assert_eq!(registry.entity_owner_requests_for_test(), 1);
    assert_eq!(registry.persisted_entity_records().len(), 2);
}

#[test]
fn detached_safe_herd_failure_releases_enqueue_claim_for_one_retry() {
    let chunk = (3, 2);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
            commits: Arc::clone(&commits),
        }),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let spawns = vec![super::super::HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 4,
        entity_type_name: "minecraft:cow".to_owned(),
        position: Vec3::new(56.5, 64.0, 40.5),
        hostile: false,
        sheep_color: None,
    }];

    handle
        .ensure_chunk_herd(chunk, spawns.clone())
        .expect("queue first detached herd");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(registry.persisted_entity_records().is_empty());

    handle
        .ensure_chunk_herd(chunk, spawns.clone())
        .expect("safe failure releases detached herd claim");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert_eq!(commits.load(Ordering::Relaxed), 2);

    handle
        .ensure_chunk_herd(chunk, spawns)
        .expect("committed herd remains coalesced");
    assert_eq!(handle.snapshot().enqueued, 2);
    assert_eq!(handle.snapshot().depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn world_time_handles_enforce_player_and_server_fences_and_owner_ordering() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "TimeFenceOwner");
    let (handle, mut owner) = simulation_channel_with_capacity(1);

    assert_eq!(
        handle.set_world_time(1).await,
        Err(SimulationRequestError::InvalidCommand)
    );
    assert_eq!(
        handle
            .for_session(session)
            .set_world_time_server_owned(1)
            .await,
        Err(SimulationRequestError::InvalidCommand)
    );

    let mut request = Box::pin(handle.set_world_time_server_owned(super::super::NIGHT_START_TICK));
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(registry.world_time(), 0);

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    request.await.expect("server-owned time set response");
    assert_eq!(registry.world_time(), super::super::NIGHT_START_TICK);
}

#[tokio::test(flavor = "current_thread")]
async fn queued_time_set_safe_failure_releases_pending_herd_for_exact_retry() {
    let chunk = (6, 2);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
            commits: Arc::clone(&commits),
        }),
    );
    let observer = register_test_session(&registry, "TimeSetSafeRetryObserver");
    registry.mark_loaded(observer, chunk);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let spawns = vec![super::super::HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 5,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(104.5, 64.0, 40.5),
        hostile: true,
        sheep_color: None,
    }];

    handle
        .ensure_chunk_herd(chunk, spawns.clone())
        .expect("queue daytime hostile herd");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(registry.persisted_entity_records().is_empty());

    let session_handle = handle.for_session(observer);
    let mut time_set = Box::pin(session_handle.set_world_time(super::super::NIGHT_START_TICK));
    assert_request_enqueued(time_set.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    time_set.await.expect("session-fenced time set response");
    assert!(registry.persisted_entity_records().is_empty());

    handle
        .ensure_chunk_herd(chunk, spawns)
        .expect("SAFE time-set failure releases exact herd request");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert_eq!(commits.load(Ordering::Relaxed), 2);
}

#[test]
fn natural_night_safe_failure_releases_pending_herd_for_exact_retry() {
    let chunk = (7, 2);
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: Some(mc_entity::RegionalDecisionJournalError::SAFE),
            commits: Arc::clone(&commits),
        }),
    );
    let observer = register_test_session(&registry, "NaturalNightSafeRetryObserver");
    registry.mark_loaded(observer, chunk);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let spawns = vec![super::super::HerdSpawn {
        chunk,
        slot: 0,
        entity_type_id: 5,
        entity_type_name: "minecraft:zombie".to_owned(),
        position: Vec3::new(120.5, 64.0, 40.5),
        hostile: true,
        sheep_color: None,
    }];

    handle
        .ensure_chunk_herd(chunk, spawns.clone())
        .expect("queue daytime hostile herd");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(registry.persisted_entity_records().is_empty());

    assert_eq!(
        owner.advance_world_time(&registry, super::super::NIGHT_START_TICK),
        super::super::NIGHT_START_TICK
    );
    assert!(registry.persisted_entity_records().is_empty());

    handle
        .ensure_chunk_herd(chunk, spawns)
        .expect("SAFE natural-night failure releases exact herd request");
    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert_eq!(commits.load(Ordering::Relaxed), 2);
}

#[test]
fn owner_night_transition_activates_pending_hostiles_once() {
    let chunk = (1, 1);
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(4);
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "night-observer".to_owned(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        0,
        HashSet::from([chunk]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.mark_loaded(session_id, chunk);
    let spawns = [
        super::super::HerdSpawn {
            chunk,
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(24.5, 64.0, 24.5),
            hostile: false,
            sheep_color: None,
        },
        super::super::HerdSpawn {
            chunk,
            slot: 1,
            entity_type_id: 5,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(25.5, 64.0, 24.5),
            hostile: true,
            sheep_color: None,
        },
    ];

    registry.ensure_chunk_herd_legacy_for_test(chunk, &spawns);
    let entity_types = || {
        registry
            .persisted_entity_records()
            .into_iter()
            .map(|record| record.snapshot.type_name)
            .collect::<Vec<_>>()
    };
    assert_eq!(entity_types(), ["minecraft:cow"]);

    let (_handle, owner) = simulation_channel_with_capacity(1);
    assert_eq!(
        owner.advance_world_time(&registry, super::super::NIGHT_START_TICK - 1),
        super::super::NIGHT_START_TICK - 1
    );
    assert_eq!(entity_types(), ["minecraft:cow"]);

    assert_eq!(
        owner.advance_world_time(&registry, 1),
        super::super::NIGHT_START_TICK
    );
    let mut nighttime_types = entity_types();
    nighttime_types.sort();
    assert_eq!(nighttime_types, ["minecraft:cow", "minecraft:zombie"]);

    owner.advance_world_time(&registry, 1);
    assert_eq!(entity_types().len(), 2);
}

#[tokio::test]
async fn queued_time_set_cannot_overtake_herd_admission() {
    let chunk = (5, 5);
    let registry = Arc::new(SessionRegistry::new());
    let observer = register_test_session(&registry, "OrderedNightObserver");
    registry.mark_loaded(observer, chunk);
    let (claim_open_tx, claim_open_rx) = std::sync::mpsc::sync_channel(0);
    let (release_claim_tx, release_claim_rx) = std::sync::mpsc::sync_channel(0);
    registry.install_chunk_herd_claim_probe_for_test(claim_open_tx, release_claim_rx);

    let (handle, mut owner) = simulation_channel_with_capacity(2);
    handle
        .ensure_chunk_herd(
            chunk,
            vec![super::super::HerdSpawn {
                chunk,
                slot: 0,
                entity_type_id: 5,
                entity_type_name: "minecraft:zombie".to_owned(),
                position: Vec3::new(88.5, 64.0, 88.5),
                hostile: true,
                sheep_color: None,
            }],
        )
        .expect("queue detached hostile herd");

    let owner_registry = Arc::clone(&registry);
    let owner_thread = std::thread::spawn(move || {
        let first = owner.process_tick(&owner_registry, 2);
        let second = owner.process_tick(&owner_registry, 2);
        (first.processed, second.processed)
    });
    claim_open_rx
        .recv()
        .expect("herd reached its session claim boundary");
    let time_response = handle
        .for_session(observer)
        .enqueue(SimulationCommand::SetWorldTime {
            world_time: super::super::NIGHT_START_TICK,
        })
        .expect("queue session-fenced time set behind herd insertion");
    release_claim_tx.send(()).expect("release herd admission");

    assert_eq!(owner_thread.join().expect("simulation owner"), (1, 1));
    assert!(matches!(
        time_response.await.expect("time set response"),
        Ok(SimulationResponse::WorldTimeSet)
    ));
    let records = registry.persisted_entity_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].snapshot.type_name, "minecraft:zombie");
    assert_eq!(registry.world_time(), super::super::NIGHT_START_TICK);
}

#[tokio::test]
async fn stale_session_time_set_is_rejected_without_changing_time() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleTimeSetter");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue(SimulationCommand::SetWorldTime {
            world_time: super::super::NIGHT_START_TICK,
        })
        .expect("queue fenced time set");
    registry.unregister(session);

    owner.process_tick(&registry, 1);

    assert!(matches!(
        response.await.expect("time set response"),
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(registry.world_time(), 0);
    assert_eq!(handle.snapshot().rejected_stale_session, 1);
}

#[test]
fn detached_chunk_herd_applies_without_being_counted_as_cancelled() {
    let chunk = (2, 2);
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);

    handle
        .ensure_chunk_herd(
            chunk,
            vec![super::super::HerdSpawn {
                chunk,
                slot: 0,
                entity_type_id: 4,
                entity_type_name: "minecraft:cow".to_owned(),
                position: Vec3::new(40.5, 64.0, 40.5),
                hostile: false,
                sheep_color: None,
            }],
        )
        .expect("detached herd command enqueues");

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert_eq!(registry.persisted_entity_records().len(), 1);
    assert_eq!(handle.snapshot().cancelled, 0);
    assert_eq!(handle.snapshot().processed, 1);
}

#[test]
fn detached_chunk_herd_enqueues_each_chunk_once() {
    let chunk = (3, 3);
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);

    handle
        .ensure_chunk_herd(chunk, Vec::new())
        .expect("first herd command enqueues");
    handle
        .ensure_chunk_herd(chunk, Vec::new())
        .expect("duplicate herd command coalesces");

    assert_eq!(handle.snapshot().enqueued, 1);
    assert_eq!(handle.snapshot().depth, 1);
    assert_eq!(owner.process_tick(&registry, 2).processed, 1);
    assert_eq!(handle.snapshot().depth, 0);
}

#[test]
fn concurrent_chunk_herd_waiter_observes_winning_enqueue_failure() {
    let chunk = (4, 4);
    let (handle, _owner) = simulation_channel_with_capacity(1);
    let (winner_claimed_tx, winner_claimed_rx) = std::sync::mpsc::sync_channel(0);
    let (release_winner_tx, release_winner_rx) = std::sync::mpsc::sync_channel(0);
    let (waiter_blocked_tx, waiter_blocked_rx) = std::sync::mpsc::sync_channel(0);
    handle.install_herd_enqueue_probe_for_test(
        winner_claimed_tx,
        release_winner_rx,
        waiter_blocked_tx,
    );

    let winner_handle = handle.clone();
    let winner = std::thread::spawn(move || winner_handle.ensure_chunk_herd(chunk, Vec::new()));
    winner_claimed_rx
        .recv()
        .expect("winning producer claimed herd chunk");

    handle
        .enqueue(SimulationCommand::EnsureChunkHerd {
            chunk: (9, 9),
            spawns: Vec::new(),
        })
        .expect("fill simulation queue after winner claims");

    let waiter_handle = handle.clone();
    let waiter = std::thread::spawn(move || waiter_handle.ensure_chunk_herd(chunk, Vec::new()));
    waiter_blocked_rx
        .recv()
        .expect("losing producer waits for winning enqueue result");
    release_winner_tx
        .send(())
        .expect("release winning herd producer");

    assert_eq!(
        winner.join().expect("winning producer"),
        Err(SimulationRequestError::Full)
    );
    assert_eq!(
        waiter.join().expect("waiting producer"),
        Err(SimulationRequestError::Full)
    );
    assert_eq!(handle.snapshot().enqueued, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn survival_break_transaction_commits_block_tool_and_drop_together() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let commits = Arc::new(AtomicUsize::new(0));
    let registry = SessionRegistry::new_with_entity_owner_journal(
        1,
        Box::new(FailOnceEntityCommitJournal {
            failure: None,
            commits: Arc::clone(&commits),
        }),
    );
    let session = register_test_session(&registry, "AtomicBreakMiner");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let durable_before = commits.load(Ordering::Relaxed);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let plan = test_survival_break_plan(pos, token, 42, 7);
    let mut request = Box::pin(session_handle.commit_survival_break(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request
        .await
        .expect("break response")
        .expect("matching break commits");

    assert_eq!(
        commits.load(Ordering::Relaxed),
        durable_before,
        "break item spawn must wait for the shared simulation checkpoint"
    );
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    let player_state = player_state.lock().unwrap();
    assert_eq!(player_state.inventory.slots[tool_slot].damage, Some(1));
    assert_eq!(
        committed.inventory.slots[tool_slot],
        player_state.inventory.slots[tool_slot]
    );
    drop(player_state);
    let drops = registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.item_stack.is_some())
        .collect::<Vec<_>>();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].snapshot.item_stack,
        Some(EntityItemStack::new(7, 1))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn loader_break_drop_and_pickup_preserve_exact_item_presentation() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "LoaderBreakMiner");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut plan = test_survival_block_break_plan(pos, token);
    let model = crate::loader::loader_block_item_model(0);
    let loader_item = ItemStack::new(45, 1)
        .with_custom_name("Ruby Block")
        .with_item_model(model.clone());
    plan.loader_block_drop = Some(loader_item.clone());
    let mut request = Box::pin(session_handle.commit_survival_block_break(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    request.await.unwrap().expect("Loader break commits");

    let expected_entity_stack = EntityItemStack::new(45, 1)
        .with_custom_name("Ruby Block")
        .with_item_model(model);
    let drop = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.item_stack.is_some())
        .expect("Loader drop persists");
    assert_eq!(
        drop.snapshot.item_stack.as_ref(),
        Some(&expected_entity_stack)
    );

    registry.advance_world_time(ITEM_PICKUP_DELAY_TICKS);
    registry.update_pose(session, PlayerPose::new(1.5, 64.0, 1.5));
    let mut pickup = Box::pin(session_handle.pickup_item_into_inventory(
        drop.snapshot.id,
        45,
        None,
        Vec::new(),
        64,
    ));
    assert_request_enqueued(pickup.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 2).processed, 1);
    let credited = pickup
        .await
        .unwrap()
        .expect("Loader drop is credited through simulation owner");
    assert_eq!(credited.credited, expected_entity_stack);
    assert!(
        player_state
            .lock()
            .unwrap()
            .inventory
            .slots
            .iter()
            .any(|stack| stack == &loader_item)
    );
    assert!(
        registry
            .persisted_entity_records()
            .into_iter()
            .all(|record| record.snapshot.id != drop.snapshot.id)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn survival_block_break_is_planned_and_committed_by_the_owner() {
    let (mut storage, pos, _) = test_block_storage();
    let water_neighbor = BlockPos {
        x: pos.x + 1,
        ..pos
    };
    storage
        .set_block_at(water_neighbor, BlockStateId(2))
        .unwrap();
    let token = storage
        .block_mutation_token(pos)
        .expect("root token after neighbouring fluid edit");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "OwnerPlannedMiner");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut plan = test_survival_block_break_plan(pos, token);
    plan.loot = Arc::new(mc_data::loot::LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(
            Identifier::parse("minecraft:stone").unwrap(),
            mc_data::loot::LootDrop::uniform(
                Identifier::parse("minecraft:cobblestone").unwrap(),
                4,
                9,
            ),
        )]),
    ));
    let expected_count = mc_data::loot::LootCount::UniformInclusive { min: 4, max: 9 }
        .try_sample(super::super::block_break_loot_seed(
            pos,
            BlockStateId(1),
            token,
        ))
        .unwrap();
    let mut request = Box::pin(session_handle.commit_survival_block_break(plan));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request
        .await
        .expect("break response")
        .expect("matching break commits");

    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(2))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[tool_slot].damage,
        Some(1)
    );
    assert_eq!(committed.block.applied.len(), 1);
    let drops = registry
        .persisted_entity_records()
        .into_iter()
        .filter_map(|record| record.snapshot.item_stack)
        .collect::<Vec<_>>();
    assert_eq!(
        drops,
        vec![EntityItemStack::new(
            7,
            i32::try_from(expected_count).unwrap()
        )]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_survival_break_and_relight_do_not_wait_for_world_writer() {
    let (mut storage, pos, _) = test_block_storage();
    let water_neighbor = BlockPos {
        x: pos.x + 1,
        ..pos
    };
    storage
        .set_block_at(water_neighbor, BlockStateId(2))
        .unwrap();
    let token = storage.block_mutation_token(pos).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "RegionalBreakMiner");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let light = Arc::new(BlockLightTable::from_arrays(
        "regional break publication",
        vec![0, 0, 0, 0, 0],
        vec![0, 15, 0, 0, 0],
        vec![true, false, true, true, true],
    ));
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let plan = test_survival_block_break_plan(pos, token);
    let mut request = Box::pin(session_handle.commit_survival_block_break(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&light),
                },
                Some(light.as_ref()),
                1,
            )
            .await
    });

    let committed = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("resident survival break completion event")
        .expect("resident survival break response")
        .expect("matching resident survival break commits");
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(2)));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[tool_slot].damage,
        Some(1)
    );
    assert_eq!(committed.block.applied.len(), 1);
    assert!(committed.block.precomputed_light_updates.is_some());
    let chunk = read_view
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(
        chunk
            .scheduled_fluid_ticks()
            .iter()
            .any(|tick| tick.pos == pos)
    );
    assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
    assert_eq!(persisted_item_drop_count(&registry), 1);
}

#[tokio::test]
async fn survival_block_breaks_in_distinct_regions_overlap() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let positions = [
        BlockPos { x: 1, y: 64, z: 1 },
        BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }
    let tokens = positions.map(|position| storage.block_mutation_token(position).unwrap());
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let light = Arc::new(BlockLightTable::from_arrays(
        "regional survival break",
        vec![0, 0, 0, 0, 0],
        vec![0, 15, 0, 0, 0],
        vec![true, false, true, true, true],
    ));
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actors = [
        register_test_session(&sessions, "RegionalMinerA"),
        register_test_session(&sessions, "RegionalMinerB"),
    ];
    let player_states = actors.map(|actor| {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
        register_test_player_state(&sessions, actor, inventory)
    });
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = (0..2)
        .map(|index| {
            handle
                .for_session(actors[index])
                .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
                    SurvivalBreakCommand {
                        actor_session: actors[index],
                        request: SurvivalBreakRequest::Block(test_survival_block_break_plan(
                            positions[index],
                            tokens[index],
                        )),
                    },
                )))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);

    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&light),
                },
                Some(light.as_ref()),
                2,
            ))
    });

    let first = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional survival break worker entry");
    let second = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional survival break worker enters before release");
    assert_ne!(first, second);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(worker.join().unwrap().processed, 2);
    for response in responses {
        let SimulationResponse::SurvivalBreak(Ok(Some(committed))) =
            response.await.unwrap().unwrap()
        else {
            panic!("regional survival break response mismatch");
        };
        assert!(committed.block.precomputed_light_updates.is_some());
    }
    for (position, player_state) in positions.into_iter().zip(player_states) {
        assert_eq!(
            world.lock().await.get_cached_block(position),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
            Some(1)
        );
    }
    assert_eq!(persisted_item_drop_count(&sessions), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn owner_planned_survival_break_rejects_a_stale_root_without_side_effects() {
    let (storage, pos, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleOwnerPlannedMiner");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(
        session_handle.commit_survival_block_break(test_survival_block_break_plan(pos, token)),
    );
    assert_request_enqueued(request.as_mut(), &handle).await;

    world
        .lock()
        .await
        .set_block_at(pos, BlockStateId(3))
        .unwrap();
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );

    assert!(request.await.expect("break response").is_none());
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(3))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[tool_slot].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn survival_tool_edit_honours_extra_world_preconditions_atomically() {
    let (storage, pos, token) = test_block_storage();
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let above_token = storage
        .block_mutation_token(above)
        .expect("resident block above token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "AtomicHoeUser");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut plan = test_survival_break_plan(pos, token, 42, 7);
    plan.preconditions.push(BlockEditPrecondition {
        pos: above,
        expected_state: BlockStateId(0),
        expected_token: above_token,
    });
    plan.falling_block_entity_type_id = None;
    plan.drops.clear();
    let mut request = Box::pin(session_handle.commit_survival_break(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    request
        .await
        .expect("tool edit response")
        .expect("matching world guards commit");
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[tool_slot].damage,
        Some(1)
    );
}

#[tokio::test]
async fn survival_tool_edit_rejects_stale_extra_guard_without_tool_damage() {
    let (storage, pos, token) = test_block_storage();
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let above_token = storage
        .block_mutation_token(above)
        .expect("resident block above token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleHoeUser");
    let mut inventory = PlayerInventory::empty();
    let tool_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[tool_slot] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let mut plan = test_survival_break_plan(pos, token, 42, 7);
    plan.preconditions.push(BlockEditPrecondition {
        pos: above,
        expected_state: BlockStateId(0),
        expected_token: above_token,
    });
    plan.falling_block_entity_type_id = None;
    plan.drops.clear();
    world
        .lock()
        .await
        .set_block_at(above, BlockStateId(2))
        .expect("replace block above after planning");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(plan),
            },
        )))
        .unwrap();

    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[tool_slot].damage,
        None
    );
}

#[tokio::test(flavor = "current_thread")]
async fn survival_placement_transaction_commits_block_and_inventory_debit_together() {
    let (mut storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    assert_eq!(storage.get_block(target).unwrap(), Some(BlockStateId(0)));
    let target_token = storage
        .block_mutation_token(target)
        .expect("resident placement target token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "AtomicPlacementBuilder");
    let mut inventory = PlayerInventory::empty();
    let held_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[held_slot] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.scheduled_block_ticks.push(ScheduledBlockTick::new(
        target,
        Identifier::parse("minecraft:stone").unwrap(),
        8,
        0,
    ));
    let mut request = Box::pin(session_handle.commit_survival_placement(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request
        .await
        .expect("placement response")
        .expect("matching placement commits");

    let mut world = world.lock().await;
    assert_eq!(world.get_cached_block(target), Some(BlockStateId(1)));
    let scheduled = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .expect("placement schedules owner tick");
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].pos, target);
    assert_eq!(scheduled[0].trigger_tick, 8);
    drop(world);
    let player_state = player_state.lock().unwrap();
    assert_eq!(
        player_state.inventory.slots[held_slot],
        ItemStack::new(42, 1)
    );
    assert_eq!(
        committed.inventory.slots[held_slot],
        player_state.inventory.slots[held_slot]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn creative_placement_transaction_commits_without_inventory_debit() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage
        .block_mutation_token(target)
        .expect("resident placement target token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "CreativePlacementBuilder");
    let mut inventory = PlayerInventory::empty();
    let held_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[held_slot] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    player_state.lock().unwrap().game_mode = GameMode::Creative;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.expected_game_mode = GameMode::Creative;
    let mut request = Box::pin(session_handle.commit_survival_placement(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request
        .await
        .expect("placement response")
        .expect("matching creative placement commits");

    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(42, 2)
    );
    assert_eq!(committed.inventory.slots[held_slot], ItemStack::new(42, 2));
    assert!(committed.changed_slots.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_survival_placement_does_not_wait_for_world_writer() {
    let (mut storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let water_neighbor = BlockPos {
        x: target.x + 1,
        ..target
    };
    storage
        .set_block_at(water_neighbor, BlockStateId(2))
        .unwrap();
    let target_token = storage
        .block_mutation_token(target)
        .expect("resident placement target token");
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let session = register_test_session(&registry, "RegionalPlacementBuilder");
    let mut inventory = PlayerInventory::empty();
    let held_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[held_slot] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let plan = test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    let mut request = Box::pin(session_handle.commit_survival_placement(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_read = read_view.clone();
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&owner_read),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    let committed = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("resident placement completion event")
        .expect("resident placement response")
        .expect("matching resident placement commits");
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert_eq!(read_view.get_cached_block(target), Some(BlockStateId(1)));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(42, 1)
    );
    assert_eq!(committed.block.applied.len(), 1);
    let chunk = read_view
        .snapshot_chunks(&[ChunkPos { x: 0, z: 0 }])
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(
        chunk
            .scheduled_fluid_ticks()
            .iter()
            .any(|tick| tick.pos == water_neighbor)
    );
}

#[tokio::test]
async fn survival_placements_in_distinct_regions_overlap() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let supports = [
        BlockPos { x: 1, y: 64, z: 1 },
        BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    for support in supports {
        storage.set_block_at(support, BlockStateId(1)).unwrap();
    }
    let targets = supports.map(|support| BlockPos {
        x: support.x + 1,
        ..support
    });
    let support_tokens = supports.map(|pos| storage.block_mutation_token(pos).unwrap());
    let target_tokens = targets.map(|pos| storage.block_mutation_token(pos).unwrap());
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let light = Arc::new(BlockLightTable::from_arrays(
        "regional survival placement",
        vec![0, 0, 0, 0, 0],
        vec![0, 15, 0, 0, 0],
        vec![true, false, true, true, true],
    ));
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actors = [
        register_test_session(&sessions, "RegionalBuilderA"),
        register_test_session(&sessions, "RegionalBuilderB"),
    ];
    let player_states = actors.map(|actor| {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        register_test_player_state(&sessions, actor, inventory)
    });
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = (0..2)
        .map(|index| {
            handle
                .for_session(actors[index])
                .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                    SurvivalPlacementCommand {
                        actor_session: actors[index],
                        plan: test_survival_placement_plan(
                            targets[index],
                            target_tokens[index],
                            supports[index],
                            support_tokens[index],
                            42,
                            2,
                        ),
                    },
                )))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);

    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&light),
                },
                Some(light.as_ref()),
                2,
            ))
    });

    let first = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional placement worker entry");
    let second = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional placement worker enters before release");
    assert_ne!(first, second);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(worker.join().unwrap().processed, 2);
    for response in responses {
        let SimulationResponse::SurvivalPlacement(Ok(Some(committed))) =
            response.await.unwrap().unwrap()
        else {
            panic!("regional survival placement response mismatch");
        };
        assert!(committed.block.precomputed_light_updates.is_some());
    }
    for (target, player_state) in targets.into_iter().zip(player_states) {
        assert_eq!(
            world.lock().await.get_cached_block(target),
            Some(BlockStateId(1))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 1)
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn offhand_food_use_transaction_commits_inventory_and_hunger_together() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "AtomicFoodPlayer");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let expected_survival = SurvivalState {
        food: 16,
        saturation: 1.0,
        ..SurvivalState::FULL
    };
    player_state.lock().unwrap().survival = expected_survival;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_food_use(FoodUsePlan {
        held_slot: PlayerInventory::OFFHAND_SLOT,
        expected_held: ItemStack::new(42, 2),
        expected_survival,
        food: 4,
        saturation: 2.4,
        can_always_eat: false,
        remainder: None,
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let committed = request
        .await
        .expect("food response")
        .expect("matching food use commits");

    let player_state = player_state.lock().unwrap();
    assert_eq!(
        player_state.inventory.slots[PlayerInventory::OFFHAND_SLOT],
        ItemStack::new(42, 1)
    );
    assert_eq!(player_state.survival.food, 20);
    assert!(player_state.survival.saturation > expected_survival.saturation);
    assert_eq!(committed.inventory.slots, player_state.inventory.slots);
    assert_eq!(committed.survival, player_state.survival);
}

#[tokio::test(flavor = "current_thread")]
async fn animal_feed_transaction_debits_wheat_and_enters_love_together() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "AtomicAnimalFeeder");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());

    let wheat_item_id = 42;
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
    let player_state = register_test_player_state(&registry, session, inventory);

    let position = Vec3::new(1.5, 64.0, 0.5);
    let spawns = vec![super::super::HerdSpawn {
        chunk: (0, 0),
        slot: 0,
        entity_type_id: 4,
        entity_type_name: "minecraft:cow".to_owned(),
        position,
        hostile: false,
        sheep_color: None,
    }];
    let entity_id = publish_entity_spawns(
        registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
        &mut outbound,
    )[0];

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
        entity_id,
        held_slot,
        expected_held: ItemStack::new(wheat_item_id, 2),
        food_item_id: wheat_item_id,
        targets: AnimalFeedTargets {
            cow: true,
            sheep: true,
            chicken: false,
        },
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let committed = request
        .await
        .expect("animal feed response")
        .expect("matching animal feed commits");

    {
        let player_state = player_state.lock().unwrap();
        assert_eq!(
            player_state.inventory.slots[held_slot],
            ItemStack::new(wheat_item_id, 1)
        );
        assert_eq!(committed.inventory.slots, player_state.inventory.slots);
    }
    assert_eq!(
        registry
            .server_entity_snapshot(entity_id)
            .and_then(|entity| entity.animal)
            .expect("cow breeding state")
            .love_ticks,
        mc_entity::ANIMAL_LOVE_DURATION_TICKS
    );
    assert!(matches!(
        outbound.recv().await,
        Some(OutboundCommand::EntityEvent {
            entity_id: wire_id,
            event_id: 18,
        }) if wire_id == entity_id.0
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn sheep_shear_transaction_damages_tool_marks_entity_and_spawns_wool_once() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "AtomicShearer");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());

    let shears_item_id = 42;
    let wool_item_id = 43;
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(shears_item_id, 1);
    let player_state = register_test_player_state(&registry, session, inventory);

    let position = Vec3::new(1.5, 64.0, 0.5);
    let entity_id = publish_entity_spawns(
        registry.ensure_chunk_herd_legacy_for_test(
            (0, 0),
            &[super::super::HerdSpawn {
                chunk: (0, 0),
                slot: 0,
                entity_type_id: 4,
                entity_type_name: "minecraft:sheep".to_owned(),
                position,
                hostile: false,
                sheep_color: None,
            }],
        ),
        &mut outbound,
    )[0];

    let plan = SheepShearPlan {
        entity_id,
        held_slot,
        expected_held: ItemStack::new(shears_item_id, 1),
        shears_item_id,
        shears_max_damage: 238,
        item_entity_type_id: 1,
        wool_item_ids: [wool_item_id; 16],
    };
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_sheep_shear(plan.clone()));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let committed = request
        .await
        .expect("sheep shear response")
        .expect("matching sheep shear commits");
    assert!((1..=3).contains(&committed.drop_count));
    assert_eq!(
        committed.inventory.slots[held_slot],
        ItemStack::new(shears_item_id, 1).with_damage(1)
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(shears_item_id, 1).with_damage(1)
    );
    assert!(
        registry
            .server_entity_snapshot(entity_id)
            .and_then(|entity| entity.animal)
            .and_then(|animal| animal.sheep_wool)
            .is_some_and(|wool| wool.sheared)
    );

    let mut metadata_updates = 0;
    let mut wool_spawns = 0;
    while let Ok(command) = outbound.try_recv() {
        match command {
            OutboundCommand::UpdateEntityData(entity) if entity.id == entity_id => {
                metadata_updates += 1;
            }
            OutboundCommand::SpawnEntity(entity)
                if entity
                    .item_stack
                    .as_ref()
                    .is_some_and(|stack| stack.item_id == wool_item_id && stack.count == 1) =>
            {
                wool_spawns += 1;
            }
            _ => {}
        }
    }
    assert_eq!(metadata_updates, 1);
    assert_eq!(wool_spawns, committed.drop_count);

    let second_plan = SheepShearPlan {
        expected_held: ItemStack::new(shears_item_id, 1).with_damage(1),
        ..plan
    };
    let session_handle = handle.for_session(session);
    let mut second = Box::pin(session_handle.commit_sheep_shear(second_plan));
    assert_request_enqueued(second.as_mut(), &handle).await;
    assert_eq!(owner.process_tick(&registry, 2).processed, 1);
    assert!(second.await.expect("second sheep shear response").is_none());
    assert!(outbound.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn animal_pair_breeds_on_the_sixtieth_simulation_tick() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "AnimalBreeder");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());

    let wheat_item_id = 42;
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
    register_test_player_state(&registry, session, inventory);

    let spawns = vec![
        super::super::HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(1.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        },
        super::super::HerdSpawn {
            chunk: (0, 0),
            slot: 1,
            entity_type_id: 4,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(2.5, 64.0, 0.5),
            hostile: false,
            sheep_color: None,
        },
    ];
    let parent_ids = publish_entity_spawns(
        registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
        &mut outbound,
    );
    assert_eq!(parent_ids.len(), 2);
    registry.publish_active_simulation_entities_for_test(parent_ids.iter().copied());

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    for (index, entity_id) in parent_ids.iter().copied().enumerate() {
        let expected_count = 2 - index as i32;
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
            entity_id,
            held_slot,
            expected_held: ItemStack::new(wheat_item_id, expected_count),
            food_item_id: wheat_item_id,
            targets: AnimalFeedTargets {
                cow: true,
                sheep: true,
                chicken: false,
            },
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(request.await.unwrap().is_some());
        assert!(matches!(
            outbound.recv().await,
            Some(OutboundCommand::EntityEvent { event_id: 18, .. })
        ));
    }

    for _ in 0..(mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS - 1) {
        assert_eq!(owner.tick_animal_breeding(&registry, 1), 0);
    }
    assert_eq!(
        registry
            .persisted_entity_records()
            .into_iter()
            .filter(|record| record.snapshot.type_name == "minecraft:cow")
            .count(),
        2
    );

    assert_eq!(owner.tick_animal_breeding(&registry, 1), 1);
    let cows = registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:cow")
        .collect::<Vec<_>>();
    assert_eq!(cows.len(), 3);
    assert_eq!(
        cows.iter()
            .filter_map(|record| record.snapshot.animal)
            .filter(|animal| animal.age_ticks == mc_entity::PARENT_BREEDING_COOLDOWN_TICKS)
            .count(),
        2
    );
    assert_eq!(
        cows.iter()
            .filter_map(|record| record.snapshot.animal)
            .filter(|animal| animal.age_ticks == mc_entity::BABY_START_AGE_TICKS)
            .count(),
        1
    );
    assert!(matches!(
        outbound.recv().await,
        Some(OutboundCommand::SpawnEntity(entity))
            if entity.animal.is_some_and(|animal| animal.is_baby())
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn red_and_yellow_sheep_breed_an_orange_ecs_child() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "SheepColorBreeder");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());

    let wheat_item_id = 42;
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(wheat_item_id, 2);
    register_test_player_state(&registry, session, inventory);

    let spawns = vec![
        super::super::HerdSpawn {
            chunk: (0, 0),
            slot: 0,
            entity_type_id: 4,
            entity_type_name: "minecraft:sheep".to_owned(),
            position: Vec3::new(1.5, 64.0, 0.5),
            hostile: false,
            sheep_color: Some(mc_entity::SheepColor::Red),
        },
        super::super::HerdSpawn {
            chunk: (0, 0),
            slot: 1,
            entity_type_id: 4,
            entity_type_name: "minecraft:sheep".to_owned(),
            position: Vec3::new(2.5, 64.0, 0.5),
            hostile: false,
            sheep_color: Some(mc_entity::SheepColor::Yellow),
        },
    ];
    let parent_ids = publish_entity_spawns(
        registry.ensure_chunk_herd_legacy_for_test((0, 0), &spawns),
        &mut outbound,
    );
    assert_eq!(parent_ids.len(), 2);
    registry.publish_active_simulation_entities_for_test(parent_ids.iter().copied());

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    for (index, entity_id) in parent_ids.iter().copied().enumerate() {
        let expected_count = 2 - index as i32;
        let session_handle = handle.for_session(session);
        let mut request = Box::pin(session_handle.commit_animal_feed(AnimalFeedPlan {
            entity_id,
            held_slot,
            expected_held: ItemStack::new(wheat_item_id, expected_count),
            food_item_id: wheat_item_id,
            targets: AnimalFeedTargets {
                cow: true,
                sheep: true,
                chicken: false,
            },
        }));
        assert_request_enqueued(request.as_mut(), &handle).await;
        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(request.await.unwrap().is_some());
        assert!(matches!(
            outbound.recv().await,
            Some(OutboundCommand::EntityEvent { event_id: 18, .. })
        ));
    }

    for _ in 0..mc_entity::ANIMAL_BREEDING_COURTSHIP_TICKS {
        owner.tick_animal_breeding(&registry, 1);
    }
    let child = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot)
        .find(|entity| {
            entity
                .animal
                .is_some_and(|animal| animal.age_ticks == mc_entity::BABY_START_AGE_TICKS)
        })
        .expect("bred sheep child");
    let child_wool = child
        .animal
        .and_then(|animal| animal.sheep_wool)
        .expect("bred sheep child wool state");

    assert_eq!(child_wool.color, mc_entity::SheepColor::Orange);
    assert!(matches!(
        outbound.recv().await,
        Some(OutboundCommand::SpawnEntity(entity))
            if entity.id == child.id
                && entity
                    .animal
                    .and_then(|animal| animal.sheep_wool)
                    .is_some_and(|wool| wool.color == mc_entity::SheepColor::Orange)
    ));
}

#[test]
fn hostile_melee_is_pushed_on_its_simulation_tick() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "HostileTarget");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    let entity_id = publish_entity_spawns(
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.5, 64.0, 0.0),
        ),
        &mut outbound,
    )[0];
    let (_, owner) = simulation_channel();
    let phase = u64::from(entity_id.0.unsigned_abs()) % HOSTILE_MELEE_PERIOD_TICKS;
    let due_tick = if phase == 0 {
        HOSTILE_MELEE_PERIOD_TICKS
    } else {
        HOSTILE_MELEE_PERIOD_TICKS - phase
    };

    assert_eq!(
        owner.tick_hostile_attacks(&registry, due_tick - 1, BlockStateId(0)),
        0
    );
    assert!(outbound.try_recv().is_err());
    assert_eq!(
        owner.tick_hostile_attacks(&registry, due_tick, BlockStateId(0)),
        1
    );

    let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::DamagePlayer {
            damage: super::super::PlayerDamageRequest {
                kind: super::super::PlayerDamageKind::MobAttack,
                amount,
                source_origin: Some(origin),
            },
            ..
        } if (*amount - 3.0).abs() < f32::EPSILON
            && *origin == Vec3::new(0.5, 64.0, 0.0)
    )));
}

#[test]
fn skeleton_shoots_a_real_arrow_on_its_simulation_tick() {
    let registry = SessionRegistry::new();
    registry.configure_arrow_kill_rewards(
        Some(2),
        Some(3),
        Some(77),
        Arc::new(mc_data::items::solaris_required_items()),
        Arc::new(mc_data::item_components::ItemFactsTable::default()),
        Arc::new(mc_data::loot::LootTables::default()),
    );
    let (session, mut outbound) = register_test_session_with_outbound(&registry, "SkeletonTarget");
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    publish_entity_spawns(
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:skeleton".to_owned(),
            Vec3::new(0.5, 64.0, 6.5),
        ),
        &mut outbound,
    );
    let (_, owner) = simulation_channel();
    let draw_tick = 11;
    assert_eq!(
        owner.tick_hostile_attacks(&registry, draw_tick, BlockStateId(0)),
        0
    );
    assert_eq!(
        owner.tick_hostile_attacks(
            &registry,
            draw_tick + SKELETON_BOW_DRAW_TICKS,
            BlockStateId(0)
        ),
        1
    );

    let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
    assert!(commands.iter().any(|command| matches!(
        command,
        OutboundCommand::SpawnEntity(entity)
            if entity.type_id == 77
                && entity.type_name == "minecraft:arrow"
                && entity.velocity.z < 0.0
                && !entity.on_ground
    )));
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, OutboundCommand::DamagePlayer { .. }))
    );
}

#[tokio::test]
async fn food_use_transaction_rejects_stale_stack_and_survival_state() {
    for stale_survival in [false, true] {
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "StaleFoodPlayer");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        let expected_survival = SurvivalState {
            food: 16,
            saturation: 1.0,
            ..SurvivalState::FULL
        };
        {
            let mut state = player_state.lock().unwrap();
            state.survival = expected_survival;
            if stale_survival {
                state.survival.food = 15;
            } else {
                state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
            }
        }
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
                actor_session: session,
                plan: FoodUsePlan {
                    held_slot: PlayerInventory::HOTBAR_BASE,
                    expected_held: ItemStack::new(42, 2),
                    expected_survival,
                    food: 4,
                    saturation: 2.4,
                    can_always_eat: false,
                    remainder: None,
                },
            }))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::FoodUse(Ok(None))
        ));
        let state = player_state.lock().unwrap();
        assert_ne!(state.survival.food, 20);
    }
}

#[test]
fn food_use_owner_apply_survives_requester_loss() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "LostFoodRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let expected_survival = SurvivalState {
        food: 16,
        saturation: 1.0,
        ..SurvivalState::FULL
    };
    player_state.lock().unwrap().survival = expected_survival;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
            actor_session: session,
            plan: FoodUsePlan {
                held_slot: PlayerInventory::HOTBAR_BASE,
                expected_held: ItemStack::new(42, 2),
                expected_survival,
                food: 4,
                saturation: 2.4,
                can_always_eat: false,
                remainder: None,
            },
        }))
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);

    let state = player_state.lock().unwrap();
    assert_eq!(state.survival.food, 20);
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
}

#[tokio::test]
async fn food_use_transaction_rejects_stale_session_before_mutation() {
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleFoodSession");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let expected_survival = SurvivalState {
        food: 16,
        saturation: 1.0,
        ..SurvivalState::FULL
    };
    player_state.lock().unwrap().survival = expected_survival;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitFoodUse(FoodUseCommand {
            actor_session: session,
            plan: FoodUsePlan {
                held_slot: PlayerInventory::HOTBAR_BASE,
                expected_held: ItemStack::new(42, 2),
                expected_survival,
                food: 4,
                saturation: 2.4,
                can_always_eat: false,
                remainder: None,
            },
        }))
        .unwrap();
    registry.unregister(session);

    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    let state = player_state.lock().unwrap();
    assert_eq!(state.survival, expected_survival);
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

const BOW_TEST_ARROW_SLOT: usize = 10;

fn test_bow_release_plan(expected_bow: ItemStack, expected_arrow: ItemStack) -> BowReleasePlan {
    BowReleasePlan {
        bow_slot: PlayerInventory::HOTBAR_BASE,
        expected_bow,
        arrow_slot: BOW_TEST_ARROW_SLOT,
        expected_arrow,
        bow_max_damage: 384,
        entity_type_id: 3,
        position: Vec3::new(0.5, 65.5, 0.5),
        velocity: Vec3::new(0.0, 0.1, 2.0),
        rotation: Rotation::ZERO,
    }
}

fn bow_release_inventory(bow: ItemStack, arrow_count: i32) -> PlayerInventory {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = bow;
    inventory.slots[BOW_TEST_ARROW_SLOT] = ItemStack::new(43, arrow_count);
    inventory
}

fn arrow_entity_count(registry: &SessionRegistry) -> usize {
    registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:arrow")
        .count()
}

#[tokio::test(flavor = "current_thread")]
async fn bow_release_transaction_commits_arrow_bow_and_projectile_together() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "AtomicBow");
    let bow = ItemStack::new(42, 1);
    let arrows = ItemStack::new(43, 3);
    let player_state = register_test_player_state(
        &registry,
        session,
        bow_release_inventory(bow.clone(), arrows.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request =
        Box::pin(session_handle.commit_bow_release(test_bow_release_plan(bow, arrows)));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let committed = request
        .await
        .expect("bow response")
        .expect("matching bow release commits");

    let state = player_state.lock().unwrap();
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1).with_damage(1)
    );
    assert_eq!(
        state.inventory.slots[BOW_TEST_ARROW_SLOT],
        ItemStack::new(43, 2)
    );
    assert_eq!(committed.inventory.slots, state.inventory.slots);
    assert_eq!(arrow_entity_count(&registry), 1);
}

#[tokio::test]
async fn bow_release_transaction_rejects_stale_bow_or_arrow_without_projectile() {
    for stale_bow in [false, true] {
        let registry = SessionRegistry::new();
        let (session, _outbound) = register_test_session_with_outbound(&registry, "StaleBow");
        let bow = ItemStack::new(42, 1);
        let arrows = ItemStack::new(43, 3);
        let player_state = register_test_player_state(
            &registry,
            session,
            bow_release_inventory(bow.clone(), arrows.count),
        );
        {
            let mut state = player_state.lock().unwrap();
            let slot = if stale_bow {
                PlayerInventory::HOTBAR_BASE
            } else {
                BOW_TEST_ARROW_SLOT
            };
            state.inventory.slots[slot].count -= 1;
        }
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                actor_session: session,
                plan: test_bow_release_plan(bow, arrows),
            }))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::BowRelease(Ok(None))
        ));
        assert_eq!(arrow_entity_count(&registry), 0);
    }
}

#[tokio::test]
async fn duplicate_bow_release_commits_exactly_one_projectile() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "DoubleBow");
    let bow = ItemStack::new(42, 1);
    let arrows = ItemStack::new(43, 2);
    let player_state = register_test_player_state(
        &registry,
        session,
        bow_release_inventory(bow.clone(), arrows.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let mut responses = Vec::new();
    for _ in 0..2 {
        responses.push(
            handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
                    actor_session: session,
                    plan: test_bow_release_plan(bow.clone(), arrows.clone()),
                }))
                .unwrap(),
        );
    }

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let mut committed = 0;
    for response in responses {
        if matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::BowRelease(Ok(Some(_)))
        ) {
            committed += 1;
        }
    }

    assert_eq!(committed, 1);
    assert_eq!(arrow_entity_count(&registry), 1);
    let state = player_state.lock().unwrap();
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1).with_damage(1)
    );
    assert_eq!(
        state.inventory.slots[BOW_TEST_ARROW_SLOT],
        ItemStack::new(43, 1)
    );
}

#[test]
fn bow_release_owner_apply_survives_requester_loss_and_breaks_spent_bow() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "LostBow");
    let bow = ItemStack::new(42, 1).with_damage(383);
    let arrows = ItemStack::new(43, 1);
    let player_state = register_test_player_state(
        &registry,
        session,
        bow_release_inventory(bow.clone(), arrows.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
            actor_session: session,
            plan: test_bow_release_plan(bow, arrows),
        }))
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);

    let state = player_state.lock().unwrap();
    assert!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());
    assert!(state.inventory.slots[BOW_TEST_ARROW_SLOT].is_empty());
    assert_eq!(arrow_entity_count(&registry), 1);
}

#[tokio::test]
async fn bow_release_transaction_rejects_stale_session_before_mutation() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "GoneBow");
    let bow = ItemStack::new(42, 1);
    let arrows = ItemStack::new(43, 1);
    let player_state = register_test_player_state(
        &registry,
        session,
        bow_release_inventory(bow.clone(), arrows.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitBowRelease(BowReleaseCommand {
            actor_session: session,
            plan: test_bow_release_plan(bow, arrows),
        }))
        .unwrap();
    registry.unregister(session);

    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(arrow_entity_count(&registry), 0);
    let state = player_state.lock().unwrap();
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert_eq!(
        state.inventory.slots[BOW_TEST_ARROW_SLOT],
        ItemStack::new(43, 1)
    );
}

fn test_selected_item_drop_plan(expected_held: ItemStack, drop_count: i32) -> SelectedItemDropPlan {
    SelectedItemDropPlan {
        held_hotbar_slot: 0,
        expected_held,
        drop_count,
        entity_type_id: 2,
        position: Vec3::new(0.5, 65.0, 2.6),
    }
}

fn selected_item_drop_inventory(count: i32) -> PlayerInventory {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, count);
    inventory
}

fn persisted_item_drop_stacks(registry: &SessionRegistry) -> Vec<EntityItemStack> {
    registry
        .persisted_entity_records()
        .into_iter()
        .filter_map(|record| record.snapshot.item_stack)
        .collect()
}

#[tokio::test]
async fn lethal_player_survival_transition_commits_state_and_drops_once() {
    let registry = SessionRegistry::new();
    let (session, mut outbound) =
        register_test_session_with_outbound(&registry, "AtomicPlayerDeath");
    registry.mark_loaded(session, (0, 0));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[5] = ItemStack::new(42, 1).with_damage(3);
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 4);
    let player_state = register_test_player_state(&registry, session, inventory.clone());
    {
        let mut state = player_state.lock().unwrap();
        state.carried_item = ItemStack::new(44, 2);
        state.xp = XpState {
            level: 12,
            progress: 0.5,
            total: 87,
            seed: 7,
        };
    }

    let expected_survival = SurvivalState::FULL;
    let mut updated_survival = expected_survival;
    updated_survival.apply_damage(mc_entity::player_survival_26_1_2::MAX_HEALTH);
    let plan = PlayerSurvivalPlan {
        hook_approval: None,
        expected_survival,
        updated_survival,
        expected_inventory: inventory.clone(),
        updated_inventory: inventory,
        expected_carried_item: ItemStack::new(44, 2),
        expected_xp: XpState {
            level: 12,
            progress: 0.5,
            total: 87,
            seed: 7,
        },
        updated_xp: XpState {
            level: 12,
            progress: 0.5,
            total: 87,
            seed: 7,
        },
        active_shield: None,
        enchanting_table_input: None,
        item_entity_type_id: Some(1),
        xp_orb_entity_type_id: Some(2),
        keep_inventory: false,
        position: Vec3::new(0.5, 64.0, 0.5),
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = (0..2)
        .map(|_| {
            handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitPlayerSurvival(Box::new(
                    PlayerSurvivalCommand {
                        actor_session: session,
                        plan: Box::new(plan.clone()),
                    },
                )))
                .unwrap()
        })
        .collect::<Vec<_>>();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let mut committed = Vec::new();
    for response in responses {
        match response.await.unwrap().unwrap() {
            SimulationResponse::PlayerSurvival(Ok(Some(outcome))) => match *outcome {
                PlayerSurvivalCommitOutcome::Committed(outcome) => committed.push(*outcome),
                PlayerSurvivalCommitOutcome::Rejected(_) => {}
            },
            SimulationResponse::PlayerSurvival(Ok(None)) => {}
            other => panic!("expected player survival response, got {other:?}"),
        }
    }
    assert_eq!(
        committed.len(),
        1,
        "duplicate lethal transition must not duplicate drops"
    );
    assert!(
        std::iter::from_fn(|| outbound.try_recv().ok()).any(|command| {
            matches!(
                command,
                OutboundCommand::PickupCandidates(candidates)
                    if candidates
                        .iter()
                        .any(|entity| entity.experience_value == Some(84))
            )
        })
    );

    let state = player_state.lock().unwrap();
    assert!(state.survival.is_dead());
    assert!(state.inventory.slots[1..].iter().all(ItemStack::is_empty));
    assert!(state.carried_item.is_empty());
    assert_eq!(state.xp.level, 0);
    assert_eq!(state.xp.total, 0);
    drop(state);

    let mut drops = persisted_item_drop_stacks(&registry);
    drops.sort_by_key(|stack| stack.item_id);
    assert_eq!(
        drops,
        vec![
            EntityItemStack {
                item_id: 42,
                count: 1,
                damage: Some(3),
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
                stew_effects: Vec::new(),
            },
            EntityItemStack::new(43, 4),
            EntityItemStack::new(44, 2),
        ]
    );
    let experience = registry.nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25);
    assert_eq!(experience.len(), 1);
    assert_eq!(experience[0].experience_value, Some(84));
}

#[tokio::test(flavor = "current_thread")]
async fn selected_item_drop_transaction_commits_debit_and_entity_together() {
    let registry = SessionRegistry::new();
    let (session, _outbound) = register_test_session_with_outbound(&registry, "AtomicSelectedDrop");
    let expected = ItemStack::new(42, 3);
    let player_state = register_test_player_state(
        &registry,
        session,
        selected_item_drop_inventory(expected.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(
        session_handle.commit_selected_item_drop(test_selected_item_drop_plan(expected, 1)),
    );
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    let committed = request
        .await
        .expect("selected item drop response")
        .expect("matching selected item drop commits");

    let state = player_state.lock().unwrap();
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
    assert_eq!(committed.inventory.slots, state.inventory.slots);
    assert_eq!(
        persisted_item_drop_stacks(&registry),
        vec![EntityItemStack::new(42, 1)]
    );
    drop(state);

    let item_entity_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.item_stack.is_some())
        .expect("selected item drop entity")
        .snapshot
        .id;
    let collector = register_test_session(&registry, "SelectedDropCollector");
    let pickup_pose = PlayerPose::new(0.5, 65.0, 2.6);
    let _ = registry.update_pose(session, pickup_pose);
    let _ = registry.update_pose(collector, pickup_pose);
    registry.advance_world_time(super::super::ITEM_PICKUP_DELAY_TICKS);
    assert!(
        registry
            .claim_item_pickup_for_test(item_entity_id, session, 1)
            .is_none(),
        "drop owner must remain blocked after the generic pickup delay"
    );
    assert!(
        registry
            .claim_item_pickup_for_test(item_entity_id, collector, 1)
            .is_some(),
        "another session may collect the dropped item"
    );
}

#[tokio::test]
async fn selected_item_drop_all_commits_empty_slot_and_complete_stack() {
    let registry = SessionRegistry::new();
    let (session, _outbound) =
        register_test_session_with_outbound(&registry, "AtomicSelectedDropAll");
    let expected = ItemStack::new(42, 3).with_damage(7);
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = expected.clone();
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
            SelectedItemDropCommand {
                actor_session: session,
                plan: test_selected_item_drop_plan(expected, 3),
            },
        ))
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SelectedItemDrop(Ok(Some(_)))
    ));
    assert!(player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());
    assert_eq!(
        persisted_item_drop_stacks(&registry),
        vec![EntityItemStack {
            item_id: 42,
            count: 3,
            damage: Some(7),
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
            stew_effects: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn selected_item_drop_rejects_stale_slot_or_stack_without_entity() {
    for stale_selection in [false, true] {
        let registry = SessionRegistry::new();
        let (session, _outbound) =
            register_test_session_with_outbound(&registry, "StaleSelectedDrop");
        let expected = ItemStack::new(42, 2);
        let player_state = register_test_player_state(
            &registry,
            session,
            selected_item_drop_inventory(expected.count),
        );
        if stale_selection {
            player_state.lock().unwrap().selected_hotbar_slot = 1;
        } else {
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].count = 1;
        }
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                SelectedItemDropCommand {
                    actor_session: session,
                    plan: test_selected_item_drop_plan(expected, 1),
                },
            ))
            .unwrap();

        assert_eq!(owner.process_tick(&registry, 1).processed, 1);
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::SelectedItemDrop(Ok(None))
        ));
        assert!(persisted_item_drop_stacks(&registry).is_empty());
    }
}

#[tokio::test]
async fn duplicate_selected_item_drop_commits_exactly_one_entity() {
    let registry = SessionRegistry::new();
    let (session, _outbound) =
        register_test_session_with_outbound(&registry, "DuplicateSelectedDrop");
    let expected = ItemStack::new(42, 2);
    let player_state = register_test_player_state(
        &registry,
        session,
        selected_item_drop_inventory(expected.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = (0..2)
        .map(|_| {
            handle
                .for_session(session)
                .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
                    SelectedItemDropCommand {
                        actor_session: session,
                        plan: test_selected_item_drop_plan(expected.clone(), 1),
                    },
                ))
                .unwrap()
        })
        .collect::<Vec<_>>();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    let mut committed = 0;
    for response in responses {
        if matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::SelectedItemDrop(Ok(Some(_)))
        ) {
            committed += 1;
        }
    }

    assert_eq!(committed, 1);
    assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
}

#[test]
fn selected_item_drop_owner_apply_survives_requester_loss() {
    let registry = SessionRegistry::new();
    let (session, _outbound) =
        register_test_session_with_outbound(&registry, "LostSelectedDropRequester");
    let expected = ItemStack::new(42, 2);
    let player_state = register_test_player_state(
        &registry,
        session,
        selected_item_drop_inventory(expected.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
            SelectedItemDropCommand {
                actor_session: session,
                plan: test_selected_item_drop_plan(expected, 1),
            },
        ))
        .unwrap();

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(response);

    assert_eq!(persisted_item_drop_stacks(&registry).len(), 1);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
}

#[tokio::test]
async fn selected_item_drop_rejects_stale_session_before_mutation() {
    let registry = SessionRegistry::new();
    let (session, _outbound) =
        register_test_session_with_outbound(&registry, "StaleSelectedDropSession");
    let expected = ItemStack::new(42, 2);
    let player_state = register_test_player_state(
        &registry,
        session,
        selected_item_drop_inventory(expected.count),
    );
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
            SelectedItemDropCommand {
                actor_session: session,
                plan: test_selected_item_drop_plan(expected, 1),
            },
        ))
        .unwrap();
    registry.unregister(session);

    assert_eq!(owner.process_tick(&registry, 1).processed, 0);
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert!(persisted_item_drop_stacks(&registry).is_empty());
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test]
async fn survival_placement_transaction_debits_the_offhand_slot() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "OffhandPlacement");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.held.inventory_slot = PlayerInventory::OFFHAND_SLOT;
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan,
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(Some(_)))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::OFFHAND_SLOT],
        ItemStack::new(42, 1)
    );
}

#[tokio::test]
async fn survival_placement_transaction_rejects_stale_support_without_inventory_debit() {
    let (mut storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    storage
        .set_block_at(support, BlockStateId(0))
        .expect("replace support before stale placement");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StalePlacementSupport");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test]
async fn creative_placement_transaction_rejects_stale_support_without_mutation() {
    let (mut storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    storage
        .set_block_at(support, BlockStateId(0))
        .expect("replace support before stale creative placement");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleCreativePlacementSupport");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    player_state.lock().unwrap().game_mode = GameMode::Creative;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.expected_game_mode = GameMode::Creative;
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan,
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test]
async fn placement_transaction_rejects_game_mode_change_before_commit() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "PlacementModeChanged");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    player_state.lock().unwrap().game_mode = GameMode::Creative;
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.expected_game_mode = GameMode::Creative;
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan,
            },
        )))
        .unwrap();
    player_state.lock().unwrap().game_mode = GameMode::Survival;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test]
async fn placement_transaction_rejects_non_building_game_modes() {
    for game_mode in [GameMode::Adventure, GameMode::Spectator] {
        let (storage, support, support_token) = test_block_storage();
        let target = BlockPos {
            x: support.x + 1,
            ..support
        };
        let target_token = storage.block_mutation_token(target).unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let registry = SessionRegistry::new();
        let session = register_test_session(&registry, "PlacementNonBuildingMode");
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
        let player_state = register_test_player_state(&registry, session, inventory);
        player_state.lock().unwrap().game_mode = game_mode;
        let (handle, mut owner) = simulation_channel_with_capacity(1);
        let mut plan =
            test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
        plan.expected_game_mode = game_mode;
        let response = handle
            .for_session(session)
            .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
                SurvivalPlacementCommand {
                    actor_session: session,
                    plan,
                },
            )))
            .unwrap();

        owner.process_tick_with_world(&registry, Some(&world), None, 1);
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::SurvivalPlacement(Ok(None))
        ));
        assert_eq!(
            world.lock().await.get_cached_block(target),
            Some(BlockStateId(0))
        );
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(42, 2)
        );
    }
}

#[tokio::test]
async fn survival_placement_transaction_rejects_held_stack_mismatch_without_mutation() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "MismatchedPlacementStack");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            },
        )))
        .unwrap();

    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(43, 2)
    );
}

#[test]
fn survival_placement_transaction_survives_requester_loss_after_apply() {
    let (mut storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let water = BlockPos {
        x: target.x + 1,
        ..target
    };
    storage
        .set_block_at(water, BlockStateId(2))
        .expect("place adjacent water");
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "LostPlacementRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let block_light = BlockLightTable::from_arrays(
        "test",
        vec![0; 5],
        vec![0, 15, 1, 15, 15],
        vec![true, false, false, false, false],
    );
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            },
        )))
        .unwrap();

    owner.process_tick_with_world(&registry, Some(&world), Some(&block_light), 1);
    drop(response);
    assert_eq!(
        world.blocking_lock().get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    let mut storage = world.blocking_lock();
    let chunk = storage
        .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
    let ticks = storage
        .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .unwrap();
    assert!(ticks.iter().any(|tick| tick.pos == water));
}

#[tokio::test]
async fn survival_placement_transaction_rejects_stale_session_without_mutation() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "DisconnectedPlacementBuilder");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: session,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            },
        )))
        .unwrap();
    registry.unregister(session);

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        0
    );
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test]
async fn concurrent_survival_placement_transactions_have_one_exact_winner() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let first_session = register_test_session(&registry, "FirstPlacementBuilder");
    let second_session = register_test_session(&registry, "SecondPlacementBuilder");
    let mut first_inventory = PlayerInventory::empty();
    first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let first_state = register_test_player_state(&registry, first_session, first_inventory);
    let mut second_inventory = PlayerInventory::empty();
    second_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let second_state = register_test_player_state(&registry, second_session, second_inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let command = |actor_session| {
        SimulationCommand::CommitSurvivalPlacement(Box::new(SurvivalPlacementCommand {
            actor_session,
            plan: test_survival_placement_plan(target, target_token, support, support_token, 42, 2),
        }))
    };
    let first = handle
        .for_session(first_session)
        .enqueue_player_command(command(first_session))
        .unwrap();
    let second = handle
        .for_session(second_session)
        .enqueue_player_command(command(second_session))
        .unwrap();

    owner.process_tick_with_world(&registry, Some(&world), None, 2);
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(Some(_)))
    ));
    assert!(matches!(
        second.await.unwrap().unwrap(),
        SimulationResponse::SurvivalPlacement(Ok(None))
    ));
    assert_eq!(
        first_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert_eq!(
        second_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bucket_use_transaction_survives_requester_loss_after_owner_apply() {
    let (storage, support, _) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = Arc::new(SessionRegistry::new());
    let (session, _actor_rx) =
        register_test_session_with_outbound(&registry, "LostBucketRequester");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&registry, "BucketObserver");
    registry.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.mark_loaded(session, (0, 0));
    registry.mark_loaded(observer, (0, 0));
    while observer_rx.try_recv().is_ok() {}
    registry.clear_closing_sessions_for_test();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
            BucketUseCommand {
                actor_session: session,
                plan: test_bucket_use_plan(target, target_token),
            },
        )))
        .unwrap();

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_registry = Arc::clone(&registry);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_registry,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });
    let report = tokio::time::timeout(std::time::Duration::from_secs(1), owner_task)
        .await
        .expect("regional bucket owner completion event")
        .unwrap();
    assert_eq!(report.processed, 1);
    drop(response);
    drop(writer);

    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(2))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(60, 1)
    );
    let message = tokio::time::timeout(std::time::Duration::from_secs(1), observer_rx.recv())
        .await
        .expect("observer receives the bucket block deltas in time");
    assert!(matches!(message, Some(OutboundCommand::BlockDeltas(_))));
    let mut storage = world.lock().await;
    let ticks = storage
        .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .unwrap();
    assert!(ticks.iter().any(|tick| tick.pos == target));
}

#[tokio::test]
async fn bucket_uses_in_distinct_regions_overlap() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let targets = [
        BlockPos { x: 2, y: 64, z: 2 },
        BlockPos {
            x: 8 * 16 + 2,
            y: 64,
            z: 2,
        },
    ];
    let tokens = targets.map(|target| storage.block_mutation_token(target).unwrap());
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let light = Arc::new(BlockLightTable::from_arrays(
        "regional bucket use",
        vec![0, 0, 1, 0, 0],
        vec![0, 15, 1, 0, 0],
        vec![true, false, false, true, true],
    ));
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actors = [
        register_test_session(&sessions, "RegionalBucketA"),
        register_test_session(&sessions, "RegionalBucketB"),
    ];
    let player_states = actors.map(|actor| {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
        register_test_player_state(&sessions, actor, inventory)
    });
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = (0..2)
        .map(|index| {
            handle
                .for_session(actors[index])
                .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
                    BucketUseCommand {
                        actor_session: actors[index],
                        plan: test_bucket_use_plan(targets[index], tokens[index]),
                    },
                )))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    owner.install_regional_block_edit_probe(entered_tx, release_rx);

    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&light),
                },
                Some(light.as_ref()),
                2,
            ))
    });

    let first = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional bucket worker entry");
    let second = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional bucket worker enters before release");
    assert_ne!(first, second);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(worker.join().unwrap().processed, 2);
    for response in responses {
        let SimulationResponse::BucketUse(Ok(Some(committed))) = response.await.unwrap().unwrap()
        else {
            panic!("regional bucket response mismatch");
        };
        assert!(committed.block.precomputed_light_updates.is_some());
    }
    let mut storage = world.lock().await;
    for (target, player_state) in targets.into_iter().zip(player_states) {
        assert_eq!(storage.get_cached_block(target), Some(BlockStateId(2)));
        assert_eq!(
            player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
            ItemStack::new(60, 1)
        );
        let ticks = storage
            .scheduled_fluid_ticks(ChunkPos {
                x: target.x.div_euclid(16),
                z: target.z.div_euclid(16),
            })
            .unwrap()
            .unwrap();
        assert!(ticks.iter().any(|tick| tick.pos == target));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bucket_use_transaction_rejects_stale_block_without_inventory_change() {
    let (mut storage, support, _) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    storage
        .set_block_at(target, BlockStateId(1))
        .expect("replace bucket target before stale commit");
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleBucketRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(61, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitBucketUse(Box::new(
            BucketUseCommand {
                actor_session: session,
                plan: test_bucket_use_plan(target, target_token),
            },
        )))
        .unwrap();

    owner
        .process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            1,
        )
        .await;

    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BucketUse(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(61, 1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn survival_placement_world_busy_returns_without_retry_or_mutation() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "RetryingPlacementBuilder");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let plan = test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    let mut request = Box::pin(session_handle.commit_survival_placement(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;
    let guard = world.try_lock().unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::WorldBusy)
    ));
    assert_eq!(guard.get_cached_block(target), Some(BlockStateId(0)));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 2)
    );
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.enqueued, 1);
    assert_eq!(snapshot.processed, 1);
    assert_eq!(snapshot.rejected_world_busy, 1);
    assert_eq!(snapshot.depth, 0);
}

#[test]
fn survival_placement_owner_dispatches_peer_block_after_requester_loss() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (actor, _actor_rx) = register_test_session_with_outbound(&registry, "PlacementEventActor");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&registry, "PlacementEventObserver");
    registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.mark_loaded(actor, (0, 0));
    registry.mark_loaded(observer, (0, 0));
    while observer_rx.try_recv().is_ok() {}
    registry.clear_closing_sessions_for_test();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    register_test_player_state(&registry, actor, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let block_light = BlockLightTable::from_arrays(
        "test",
        vec![0; 5],
        vec![0, 15, 1, 15, 15],
        vec![true, false, false, false, false],
    );
    let response = handle
        .for_session(actor)
        .enqueue_player_command(SimulationCommand::CommitSurvivalPlacement(Box::new(
            SurvivalPlacementCommand {
                actor_session: actor,
                plan: test_survival_placement_plan(
                    target,
                    target_token,
                    support,
                    support_token,
                    42,
                    2,
                ),
            },
        )))
        .unwrap();

    owner.process_tick_with_world(&registry, Some(&world), Some(&block_light), 1);
    drop(response);
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::BlockDeltas(_))
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::LightUpdates(_))
    ));
    assert_eq!(
        world.blocking_lock().get_cached_block(target),
        Some(BlockStateId(1))
    );
}

#[tokio::test]
async fn survival_break_transaction_rejects_stale_block_without_tool_or_drop_mutation() {
    let (mut storage, pos, token) = test_block_storage();
    storage
        .set_block_at(pos, BlockStateId(0))
        .expect("replace target before stale request");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "StaleBreakMiner");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn survival_break_ejects_furnace_fuel_as_item_drops() {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(BlockRegistry::from_report(&report).unwrap());
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let furnace_id = blocks
        .block(&Identifier::parse("minecraft:furnace").unwrap())
        .unwrap()
        .default;
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
    let pos = BlockPos { x: 1, y: 64, z: 1 };
    storage
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            Chunk::empty(
                ChunkPos { x: 0, z: 0 },
                air,
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage
        .set_block_at(pos, furnace_id)
        .expect("place furnace");
    let token = storage
        .block_mutation_token(pos)
        .expect("furnace mutation token");
    let coal = mc_data::items::solaris_required_items()
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .expect("coal in item registry");
    let mut furnace = mc_world::FurnaceBlockEntity::default();
    furnace.slots[1] = mc_world::FurnaceSlot {
        item_id: coal,
        count: 15,
        ..Default::default()
    };
    assert!(
        storage
            .set_furnace_block_entity(pos, furnace)
            .expect("fuel the furnace")
    );
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "FurnaceBreakMiner");
    register_test_player_state(&registry, session, PlayerInventory::empty());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let plan = SurvivalBreakPlan {
        edits: vec![BlockEdit {
            pos,
            new_state: air,
        }],
        preconditions: vec![BlockEditPrecondition {
            pos,
            expected_state: furnace_id,
            expected_token: token,
        }],
        blocks: Arc::clone(&blocks),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &report,
        )),
        falling_block_entity_type_id: None,
        held: SurvivalBreakHeldItem {
            hotbar_slot: 0,
            expected: ItemStack::EMPTY,
            max_damage: None,
        },
        drops: vec![SurvivalBreakDrop {
            entity_type_id: 7,
            position: Vec3::new(1.5, 64.5, 1.5),
            stack: EntityItemStack::new(42, 1),
        }],
        hook_approval: None,
        zone_fence: None,
    };
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(plan),
            },
        )))
        .unwrap();
    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(Some(_)))
    ));
    let dropped: Vec<_> = registry
        .persisted_entity_records()
        .into_iter()
        .filter_map(|record| record.snapshot.item_stack)
        .collect();
    assert!(
        dropped
            .iter()
            .any(|stack| *stack == EntityItemStack::new(coal, 15)),
        "furnace fuel must drop on break, got {dropped:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn survival_break_transaction_survives_requester_loss_after_apply() {
    let (mut storage, pos, token) = test_block_storage();
    let water = BlockPos {
        x: pos.x + 1,
        ..pos
    };
    let sand = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    storage
        .set_block_at(water, BlockStateId(2))
        .expect("place adjacent water");
    storage
        .set_block_at(sand, BlockStateId(3))
        .expect("place unsupported sand");
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "LostBreakRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let block_light = Arc::new(BlockLightTable::from_arrays(
        "test",
        vec![0; 5],
        vec![0, 15, 1, 15, 15],
        vec![true, false, false, false, false],
    ));
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: Some(&block_light),
                },
                Some(block_light.as_ref()),
                1,
            )
            .await
            .processed,
        1
    );
    drop(response);

    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        Some(1)
    );
    assert_eq!(persisted_item_drop_count(&registry), 1);
    let mut storage = world.lock().await;
    assert_eq!(storage.get_cached_block(sand), Some(BlockStateId(0)));
    let chunk = storage
        .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some());
    let ticks = storage
        .scheduled_fluid_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .unwrap();
    assert!(ticks.iter().any(|tick| tick.pos == water));
    drop(storage);
    assert!(registry.persisted_entity_records().iter().any(|record| {
        record.snapshot.type_name == "minecraft:falling_block"
            && record.snapshot.block_state == Some(3)
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn survival_break_clears_campfire_state_before_requester_response() {
    let (mut storage, pos, _) = test_block_storage();
    storage
        .set_block_at(pos, BlockStateId(4))
        .expect("replace target with campfire");
    let token = storage.block_mutation_token(pos).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    assert!(
        registry
            .insert_campfire_cooking(pos, ItemStack::new(10, 1), ItemStack::new(11, 1), 20,)
            .is_some()
    );
    let session = register_test_session(&registry, "LostCampfireBreakRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    register_test_player_state(&registry, session, inventory);
    let mut plan = test_survival_break_plan(pos, token, 42, 7);
    plan.preconditions[0].expected_state = BlockStateId(4);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(plan),
            },
        )))
        .unwrap();

    owner
        .process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            1,
        )
        .await;

    assert!(registry.campfire_cooking_state(pos).is_empty());
    drop(response);
}

#[tokio::test]
async fn survival_break_transaction_rejects_stale_session_without_mutation() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "DisconnectedBreakMiner");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();
    registry.unregister(session);

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        0
    );
    assert!(matches!(
        response.await.unwrap(),
        Err(SimulationRequestError::StaleSession)
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test]
async fn concurrent_survival_break_transactions_have_one_exact_winner() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let first_session = register_test_session(&registry, "FirstBreakMiner");
    let second_session = register_test_session(&registry, "SecondBreakMiner");
    let mut first_inventory = PlayerInventory::empty();
    first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let first_state = register_test_player_state(&registry, first_session, first_inventory);
    let mut second_inventory = PlayerInventory::empty();
    second_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 1);
    let second_state = register_test_player_state(&registry, second_session, second_inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let first = handle
        .for_session(first_session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: first_session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();
    let second = handle
        .for_session(second_session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: second_session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 43, 7,
                )),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 2)
            .processed,
        2
    );
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(Some(_)))
    ));
    assert!(matches!(
        second.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(None))
    ));
    assert_eq!(
        first_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        Some(1)
    );
    assert_eq!(
        second_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 1);
}

#[tokio::test]
async fn survival_break_transaction_rejects_held_stack_mismatch_without_mutation() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "MismatchedBreakTool");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(43, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(session)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: session,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(None))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(43, 1)
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn survival_break_world_busy_returns_without_retry_or_mutation() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "RetryingBreakMiner");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let plan = test_survival_break_plan(pos, token, 42, 7);
    let mut request = Box::pin(session_handle.commit_survival_break(plan));
    assert_request_enqueued(request.as_mut(), &handle).await;
    let guard = world.try_lock().expect("test owns world lock");

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::WorldBusy)
    ));
    assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.enqueued, 1);
    assert_eq!(snapshot.processed, 1);
    assert_eq!(snapshot.rejected_world_busy, 1);
    assert_eq!(snapshot.depth, 0);
}

#[tokio::test]
async fn survival_break_owner_dispatches_peer_block_before_drop_spawn() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (actor, mut actor_rx) = register_test_session_with_outbound(&registry, "BreakEventActor");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&registry, "BreakEventObserver");
    registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    dispatch_visibility_commands(registry.mark_loaded(actor, (0, 0)));
    assert!(matches!(
        actor_rx.try_recv(),
        Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == observer
    ));
    dispatch_visibility_commands(registry.mark_loaded(observer, (0, 0)));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
    ));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    register_test_player_state(&registry, actor, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .for_session(actor)
        .enqueue_player_command(SimulationCommand::CommitSurvivalBreak(Box::new(
            SurvivalBreakCommand {
                actor_session: actor,
                request: SurvivalBreakRequest::Prepared(test_survival_break_plan(
                    pos, token, 42, 7,
                )),
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::SurvivalBreak(Ok(Some(_)))
    ));
    let first = observer_rx.try_recv();
    assert!(
        matches!(first, Ok(OutboundCommand::BlockDeltas(_))),
        "first break event was {first:?}"
    );
    let second = observer_rx.try_recv();
    assert!(
        matches!(second, Ok(OutboundCommand::SpawnEntity(_))),
        "second break event was {second:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn queued_conditional_block_edit_matches_direct_storage_commit() {
    let (mut direct_storage, pos, direct_token) = test_block_storage();
    let direct = apply_block_edit_batch_to_storage_conditionally(
        &mut direct_storage,
        None,
        &[BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        &[BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: direct_token,
        }],
    )
    .expect("direct conditional edit");

    let (queued_storage, queued_pos, queued_token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "QueuedBlockEditor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos: queued_pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos: queued_pos,
            expected_state: BlockStateId(1),
            expected_token: queued_token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let queued = request
        .await
        .expect("block edit response")
        .expect("queued conditional edit");

    assert_eq!(queued.applied, direct.applied);
    assert_eq!(direct_storage.get_cached_block(pos), Some(BlockStateId(0)));
    assert_eq!(
        world.lock().await.get_cached_block(queued_pos),
        Some(BlockStateId(0))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_transaction_commits_edit_and_drop_in_owner_order() {
    let (storage, pos, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (actor, mut actor_rx) = register_test_session_with_outbound(&registry, "BlockDropActor");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&registry, "BlockDropObserver");
    registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    dispatch_visibility_commands(registry.mark_loaded(actor, (0, 0)));
    assert!(matches!(
        actor_rx.try_recv(),
        Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == observer
    ));
    dispatch_visibility_commands(registry.mark_loaded(observer, (0, 0)));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
    ));

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_block_drops(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        vec![SurvivalBreakDrop {
            entity_type_id: 7,
            position: Vec3::new(0.5, 64.5, 0.5),
            stack: EntityItemStack::new(42, 2),
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

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
    let outcome = request.await.unwrap().expect("matching block transaction");
    assert_eq!(outcome.applied.len(), 1);
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    let drops = registry.persisted_entity_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item_stack, Some(EntityItemStack::new(42, 2)));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::BlockDeltas(_))
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::SpawnEntity(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn block_drop_transaction_rejects_stale_token_without_drop() {
    let (mut storage, pos, stale_token) = test_block_storage();
    storage.set_block_at(pos, BlockStateId(0)).unwrap();
    storage.set_block_at(pos, BlockStateId(1)).unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let actor = register_test_session(&registry, "StaleBlockDropActor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_block_drops(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: stale_token,
        }],
        vec![SurvivalBreakDrop {
            entity_type_id: 7,
            position: Vec3::new(0.5, 64.5, 0.5),
            stack: EntityItemStack::new(42, 2),
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

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
    assert!(request.await.unwrap().is_none());
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_block_edit_schedules_tick_only_after_matching_commit() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "ScheduledBlockEditor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let scheduled_tick =
        mc_world::ScheduledBlockTick::new(pos, Identifier::parse("minecraft:air").unwrap(), 20, 0);

    let mut committed = Box::pin(session_handle.apply_block_edits_with_scheduled_ticks(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        vec![scheduled_tick.clone()],
    ));
    assert_request_enqueued(committed.as_mut(), &handle).await;
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    assert!(committed.await.unwrap().is_some());

    let mut stale = Box::pin(session_handle.apply_block_edits_with_scheduled_ticks(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(1),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        vec![mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone").unwrap(),
            21,
            0,
        )],
    ));
    assert_request_enqueued(stale.as_mut(), &handle).await;
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    assert!(stale.await.unwrap().is_none());

    let mut storage = world.lock().await;
    let ticks = storage
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .unwrap();
    assert_eq!(ticks, &[scheduled_tick]);
}

#[tokio::test(flavor = "current_thread")]
async fn generic_block_edit_owner_dispatches_peer_after_requester_loss() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (actor, _actor_rx) = register_test_session_with_outbound(&registry, "BlockEditEventActor");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&registry, "BlockEditEventObserver");
    registry.replace_view(actor, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    registry.mark_loaded(actor, (0, 0));
    registry.mark_loaded(observer, (0, 0));
    while observer_rx.try_recv().is_ok() {}
    registry.clear_closing_sessions_for_test();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let block_light = BlockLightTable::from_arrays(
        "test",
        vec![0; 5],
        vec![0, 15, 1, 15, 15],
        vec![true, false, false, false, false],
    );
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    owner
        .process_commands_with_world(&registry, Some(&world), Some(&block_light), 1)
        .await;
    drop(request);

    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::BlockDeltas(_))
    ));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::LightUpdates(_))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn server_owned_loader_block_edit_dispatches_to_every_loaded_session() {
    let (mut storage, pos, _) = test_block_storage();
    storage.set_block_at(pos, BlockStateId(0)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (first, mut first_rx) = register_test_session_with_outbound(&registry, "LoaderBlockFirst");
    let (second, mut second_rx) =
        register_test_session_with_outbound(&registry, "LoaderBlockSecond");
    for session in [first, second] {
        registry.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
        registry.mark_loaded(session, (0, 0));
    }
    while first_rx.try_recv().is_ok() {}
    while second_rx.try_recv().is_ok() {}
    registry.clear_closing_sessions_for_test();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let block_light = BlockLightTable::from_arrays(
        "test",
        vec![0; 5],
        vec![0, 15, 1, 15, 15],
        vec![true, false, false, false, false],
    );
    let mut request =
        Box::pin(handle.place_loader_block_server_owned("test", pos, BlockStateId(4), None));
    assert_request_enqueued(request.as_mut(), &handle).await;

    owner
        .process_commands_with_world(&registry, Some(&world), Some(&block_light), 1)
        .await;

    assert!(request.await.unwrap());
    assert!(matches!(
        first_rx.try_recv(),
        Ok(OutboundCommand::BlockDeltas(_))
    ));
    assert!(matches!(
        first_rx.try_recv(),
        Ok(OutboundCommand::LightUpdates(_))
    ));
    assert!(matches!(
        second_rx.try_recv(),
        Ok(OutboundCommand::BlockDeltas(_))
    ));
    assert!(matches!(
        second_rx.try_recv(),
        Ok(OutboundCommand::LightUpdates(_))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(4))
    );
}

#[tokio::test]
async fn queued_conditional_block_edits_commit_only_first_matching_token() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let command = || SimulationCommand::ApplyBlockEdits {
        actor_session: Some(0),
        edits: vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        preconditions: vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
        scheduled_block_ticks: Vec::new(),
        leaf_trigger: true,
        hook_approval: None,
        zone_fence: None,
        plugin_receipt: None,
    };
    let first = handle.enqueue(command()).unwrap();
    let stale = handle.enqueue(command()).unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 2)
            .processed,
        2
    );
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(matches!(
        stale.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_none()
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    assert_ne!(world.lock().await.block_mutation_token(pos), Some(token));
}

#[tokio::test]
async fn queued_conditional_block_edit_rejects_busy_world_without_mutation() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            preconditions: vec![BlockEditPrecondition {
                pos,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();
    let guard = world.try_lock().expect("test owns world lock");

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Err(SimulationRequestError::WorldBusy))
    ));
    assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
    drop(guard);
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.processed, 1);
    assert_eq!(snapshot.rejected_world_busy, 1);
    assert_eq!(snapshot.rejected_world_unavailable, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn world_busy_response_is_not_blindly_retried() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "SingleAttemptBlockEditor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    let guard = world.try_lock().expect("test owns world lock");

    assert_eq!(
        owner
            .process_tick_with_world(&registry, Some(&world), None, 1)
            .processed,
        1
    );
    let outcome =
        std::future::poll_fn(|cx| match std::future::Future::poll(request.as_mut(), cx) {
            std::task::Poll::Ready(outcome) => std::task::Poll::Ready(outcome),
            std::task::Poll::Pending => {
                panic!("WorldBusy must be returned instead of scheduling a blind retry")
            }
        })
        .await;

    assert!(matches!(outcome, Err(SimulationRequestError::WorldBusy)));
    assert_eq!(guard.get_cached_block(pos), Some(BlockStateId(1)));
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.enqueued, 1);
    assert_eq!(snapshot.processed, 1);
    assert_eq!(snapshot.depth, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn async_owner_wakes_on_world_unlock_and_commits_once() {
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "ReactiveBlockEditor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.apply_block_edits(
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0),
        }],
        vec![BlockEditPrecondition {
            pos,
            expected_state: BlockStateId(1),
            expected_token: token,
        }],
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    let guard = world.lock().await;
    let mut processing =
        Box::pin(owner.process_commands_with_world(&registry, Some(&world), None, 1));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(processing.as_mut(), cx).is_pending(),
            "owner must wait for the world mutex release event"
        );
        std::task::Poll::Ready(())
    })
    .await;

    drop(guard);
    assert_eq!(processing.as_mut().await.processed, 1);
    drop(processing);
    assert!(request.await.unwrap().is_some());
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(0))
    );
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.enqueued, 1);
    assert_eq!(snapshot.processed, 1);
    assert_eq!(snapshot.rejected_world_busy, 0);
}

#[tokio::test]
async fn queued_unconditional_block_edits_apply_in_owner_sequence_order() {
    let (storage, pos, _) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let remove = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos,
                new_state: BlockStateId(0),
            }],
            preconditions: Vec::new(),
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();
    let restore = handle
        .enqueue(SimulationCommand::ApplyBlockEdits {
            actor_session: Some(0),
            edits: vec![BlockEdit {
                pos,
                new_state: BlockStateId(1),
            }],
            preconditions: Vec::new(),
            scheduled_block_ticks: Vec::new(),
            leaf_trigger: true,
            hook_approval: None,
            zone_fence: None,
            plugin_receipt: None,
        })
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 2)
            .processed,
        2
    );
    assert!(matches!(
        remove.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert!(matches!(
        restore.await.unwrap().unwrap(),
        SimulationResponse::BlockEdits(Ok(outcome)) if outcome.is_some()
    ));
    assert_eq!(
        world.lock().await.get_cached_block(pos),
        Some(BlockStateId(1))
    );
    assert_eq!(handle.snapshot().block_edits_processed, 2);
}

#[tokio::test(flavor = "current_thread")]
async fn chest_commit_rejects_actor_without_open_view() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    let (mut storage, pos) = test_container_storage();
    storage
        .set_chest_block_entity(pos, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actor = register_test_session(&sessions, "RemoteChestActor");
    let player = empty_container_player_plan();
    let persisted = register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_chest(
        pos,
        vec![pos],
        1,
        vec![initial.clone()],
        vec![updated],
        player,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        request.await.unwrap(),
        SharedContainerCommit::Rejected { .. }
    ));
    assert_eq!(
        world.lock().await.chest_block_entity(pos).unwrap(),
        Some(initial)
    );
    let persisted = persisted.lock().unwrap();
    assert!(persisted.inventory.slots.iter().all(ItemStack::is_empty));
    assert!(persisted.carried_item.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn chest_commit_resyncs_when_a_warehouse_reservation_floor_would_be_consumed() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    let (mut storage, position) = test_container_storage();
    storage
        .set_chest_block_entity(position, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let floors = Arc::new(arc_swap::ArcSwap::from_pointee(HashMap::from([(
        position,
        std::collections::BTreeMap::from([(42_u32, 2_u64)]),
    )])));
    sessions.install_warehouse_reservation_floors(floors);
    let actor = register_test_session(&sessions, "ReservedChestActor");
    assert_eq!(sessions.register_chest_viewer(actor, position), 1);
    let player = empty_container_player_plan();
    let persisted = register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_chest(
        position,
        vec![position],
        1,
        vec![initial.clone()],
        vec![updated],
        player,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        request.await.unwrap(),
        SharedContainerCommit::Rejected { .. }
    ));
    assert_eq!(
        world.lock().await.chest_block_entity(position).unwrap(),
        Some(initial)
    );
    assert!(
        persisted
            .lock()
            .unwrap()
            .inventory
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn regional_chest_commit_resyncs_when_a_warehouse_floor_would_be_consumed() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    let (mut storage, position) = test_container_storage();
    storage
        .set_chest_block_entity(position, initial.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    sessions.install_warehouse_reservation_floors(Arc::new(arc_swap::ArcSwap::from_pointee(
        HashMap::from([(
            position,
            std::collections::BTreeMap::from([(42_u32, 2_u64)]),
        )]),
    )));
    let actor = register_test_session(&sessions, "RegionalReservedChestActor");
    assert_eq!(sessions.register_chest_viewer(actor, position), 1);
    let player = empty_container_player_plan();
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_chest(
        position,
        vec![position],
        1,
        vec![initial.clone()],
        vec![updated],
        player,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert!(matches!(
        request.await.unwrap(),
        SharedContainerCommit::Rejected { .. }
    ));
    assert_eq!(
        world.lock().await.chest_block_entity(position).unwrap(),
        Some(initial)
    );
}

#[test]
fn server_owned_chest_commit_preserves_the_warehouse_reservation_floor() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    let (mut storage, position) = test_container_storage();
    storage
        .set_chest_block_entity(position, initial.clone())
        .unwrap();
    let mutation = storage.mutation_view();
    let registry = SessionRegistry::new();
    registry.install_warehouse_reservation_floors(Arc::new(arc_swap::ArcSwap::from_pointee(
        HashMap::from([(
            position,
            std::collections::BTreeMap::from([(42_u32, 2_u64)]),
        )]),
    )));
    let transaction = registry
        .prepare_chest_transaction(None, position)
        .expect("server-owned chest transaction");

    assert!(matches!(
        transaction
            .commit_server_owned(
                &mutation,
                ChestTransactionRequest {
                    primary_position: position,
                    positions: &[position],
                    expected_state_id: registry.chest_state_id(position),
                    expected: &[initial.clone()],
                    updated: &[updated],
                    player: None,
                },
            )
            .unwrap(),
        ServerOwnedChestCommit::StaleContainer
    ));
    assert_eq!(storage.chest_block_entity(position).unwrap(), Some(initial));
}

#[tokio::test(flavor = "current_thread")]
async fn structure_portion_journals_block_after_image_with_its_receipt() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
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
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (journal, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    registry.install_world_chunk_journal(journal);
    let batch: crate::script::storage::PreparedStorageBatch =
        serde_json::from_value(serde_json::json!({
            "transaction_id": 1,
            "plugin_id": "settlement",
            "mutations": [{
                "kind": "compare_and_swap",
                "key": "structure-progress",
                "expected_version": null,
                "value": "1"
            }]
        }))
        .unwrap();
    let receipt = batch.encode_world_decision().unwrap();
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let mut result = Box::pin(handle.commit_server_owned_block_edits(
        "settlement",
        vec![BlockEdit::new(position, BlockStateId(1))],
        None,
        receipt,
    ));
    // Receipt-bearing portions capture exact block tokens before their
    // authoritative write, then enqueue the decided mutation.
    assert_request_enqueued(result.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert_request_enqueued(result.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert_eq!(result.await.unwrap(), Some(1));
    assert_eq!(
        world.lock().await.get_cached_block(position),
        Some(BlockStateId(1))
    );
    let journal = registry.world_chunk_journal().unwrap();
    let mut recovered = 0;
    journal
        .recover_inventory_decisions(|decision_id, batch| {
            assert_eq!(decision_id, 1);
            assert_eq!(
                serde_json::to_value(batch).unwrap()["mutations"][0]["key"],
                "structure-progress"
            );
            recovered += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        recovered, 1,
        "the block decision carries its plugin receipt"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn queued_chest_commit_matches_direct_state_and_viewer_version() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;

    let (mut direct_storage, pos) = test_container_storage();
    direct_storage
        .set_chest_block_entity(pos, initial.clone())
        .unwrap();
    let direct_sessions = SessionRegistry::new();
    let (direct_state_id, _) = direct_sessions
        .try_chest_slot_dispatches(
            pos,
            1,
            1,
            7,
            super::super::chest_slot_stacks(&super::super::ChestView {
                chests: vec![updated.clone()],
            }),
        )
        .unwrap();
    direct_storage
        .set_chest_block_entity(pos, updated.clone())
        .unwrap();

    let (mut queued_storage, queued_pos) = test_container_storage();
    queued_storage
        .set_chest_block_entity(queued_pos, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
    let queued_sessions = Arc::new(SessionRegistry::new());
    let session = register_test_session(&queued_sessions, "QueuedChestActor");
    assert_eq!(
        queued_sessions.register_chest_viewer(session, queued_pos),
        1
    );
    let player = empty_container_player_plan();
    register_test_player_state(&queued_sessions, session, player.expected_inventory.clone());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_chest(
        queued_pos,
        vec![queued_pos],
        1,
        vec![initial.clone()],
        vec![updated.clone()],
        player,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&queued_sessions, Some(&world), None, 1)
            .processed,
        1
    );
    let outcome = request.await.unwrap();
    let queued_state_id = match outcome {
        SharedContainerCommit::Committed {
            state_id,
            dispatches,
            ..
        } => {
            assert!(dispatches.is_empty());
            state_id
        }
        other => panic!("expected committed chest, got {other:?}"),
    };

    assert_eq!(queued_state_id, direct_state_id);
    assert_eq!(queued_sessions.chest_state_id(queued_pos), direct_state_id);
    assert_eq!(
        world.lock().await.chest_block_entity(queued_pos).unwrap(),
        direct_storage.chest_block_entity(pos).unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resident_chest_commit_completes_while_global_world_writer_is_held() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    let (mut storage, position) = test_container_storage();
    storage
        .set_chest_block_entity(position, initial.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actor = register_test_session(&sessions, "RegionalChestActor");
    assert_eq!(sessions.register_chest_viewer(actor, position), 1);
    let player = empty_container_player_plan();
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request = Box::pin(session_handle.commit_chest(
        position,
        vec![position],
        1,
        vec![initial],
        vec![updated.clone()],
        player,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("resident chest completion event")
        .expect("resident chest response");
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert!(matches!(
        outcome,
        SharedContainerCommit::Committed { state_id: 2, .. }
    ));
    assert_eq!(
        world.lock().await.chest_block_entity(position).unwrap(),
        Some(updated)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resident_stale_chest_commit_returns_current_authoritative_state() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut first_update = initial.clone();
    first_update.slots[0].count = 1;
    let mut stale_update = initial.clone();
    stale_update.slots[0] = mc_world::FurnaceSlot::EMPTY;
    let (mut storage, position) = test_container_storage();
    storage
        .set_chest_block_entity(position, initial.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let actor = register_test_session(&sessions, "RegionalStaleChestActor");
    assert_eq!(sessions.register_chest_viewer(actor, position), 1);
    let player = empty_container_player_plan();
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let first = handle
        .enqueue(SimulationCommand::CommitChest {
            primary_position: position,
            positions: vec![position],
            expected_state_id: 1,
            actor_session: Some(actor),
            expected: vec![initial.clone()],
            updated: vec![first_update.clone()],
            player: Some(Box::new(player.clone())),
            plugin_receipt: None,
        })
        .unwrap();
    let stale = handle
        .enqueue(SimulationCommand::CommitChest {
            primary_position: position,
            positions: vec![position],
            expected_state_id: 1,
            actor_session: Some(actor),
            expected: vec![initial],
            updated: vec![stale_update],
            player: Some(Box::new(player)),
            plugin_receipt: None,
        })
        .unwrap();

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                2,
            )
            .await
            .processed,
        2
    );
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::ChestCommit(Ok(outcome))
            if matches!(*outcome, SharedContainerCommit::Committed { state_id: 2, .. })
    ));
    let SimulationResponse::ChestCommit(Ok(outcome)) = stale.await.unwrap().unwrap() else {
        panic!("regional stale chest response mismatch");
    };
    let SharedContainerCommit::Rejected {
        state_id,
        authoritative,
        ..
    } = *outcome
    else {
        panic!("regional stale chest commit unexpectedly applied");
    };
    assert_eq!(state_id, 2);
    assert_eq!(authoritative, vec![first_update]);
}

#[tokio::test]
async fn chest_commits_in_distinct_regions_overlap() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(blocks);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let chunks = [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 8, z: 0 }];
    for chunk in chunks {
        storage
            .insert_generated_chunk(chunk, Chunk::empty(chunk, BlockStateId(0), biome.clone()))
            .unwrap();
    }
    let positions = [
        BlockPos { x: 1, y: 64, z: 1 },
        BlockPos {
            x: 8 * 16 + 1,
            y: 64,
            z: 1,
        },
    ];
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[0].count = 1;
    for position in positions {
        storage.set_block_at(position, BlockStateId(1)).unwrap();
        storage
            .set_chest_block_entity(position, initial.clone())
            .unwrap();
    }
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actors = [
        register_test_session(&sessions, "RegionalChestActorA"),
        register_test_session(&sessions, "RegionalChestActorB"),
    ];
    for (actor, position) in actors.into_iter().zip(positions) {
        assert_eq!(sessions.register_chest_viewer(actor, position), 1);
        register_test_player_state(&sessions, actor, PlayerInventory::empty());
    }
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let responses = actors
        .into_iter()
        .zip(positions)
        .map(|(actor, position)| {
            handle
                .enqueue(SimulationCommand::CommitChest {
                    primary_position: position,
                    positions: vec![position],
                    expected_state_id: 1,
                    actor_session: Some(actor),
                    expected: vec![initial.clone()],
                    updated: vec![updated.clone()],
                    player: Some(Box::new(empty_container_player_plan())),
                    plugin_receipt: None,
                })
                .unwrap()
        })
        .collect::<Vec<_>>();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    sessions.install_container_commit_probe(entered_tx, release_rx);

    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(owner.process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                2,
            ))
    });

    let first_region = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("first regional chest metadata lock");
    let second_region = entered_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("second regional chest metadata lock before release");
    assert_ne!(first_region, second_region);
    release_tx.send(()).unwrap();
    release_tx.send(()).unwrap();

    assert_eq!(worker.join().unwrap().processed, 2);
    for response in responses {
        assert!(matches!(
            response.await.unwrap().unwrap(),
            SimulationResponse::ChestCommit(Ok(outcome))
                if matches!(*outcome, SharedContainerCommit::Committed { state_id: 2, .. })
        ));
    }
    for position in positions {
        assert_eq!(
            world.lock().await.chest_block_entity(position).unwrap(),
            Some(updated.clone())
        );
    }
}

#[tokio::test]
async fn queued_chest_commit_moves_container_player_cursor_and_drop_once() {
    let mut initial = mc_world::ChestBlockEntity::default();
    initial.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut first_update = initial.clone();
    first_update.slots[0].count = 1;
    let mut stale_update = initial.clone();
    stale_update.slots[0] = mc_world::FurnaceSlot::EMPTY;
    let (mut storage, pos) = test_container_storage();
    storage
        .set_chest_block_entity(pos, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let actor = register_test_session(&sessions, "AtomicChestActor");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&sessions, "AtomicChestObserver");
    dispatch_visibility_commands(sessions.mark_loaded(observer, (0, 0)));
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::SpawnPlayer(player)) if player.session_id == actor
    ));
    assert_eq!(sessions.register_chest_viewer(actor, pos), 1);
    assert_eq!(sessions.register_chest_viewer(observer, pos), 1);
    let before_inventory = PlayerInventory::empty();
    let player_state = register_test_player_state(&sessions, actor, before_inventory.clone());
    let before_carried_item = ItemStack::new(99, 2);
    player_state.lock().unwrap().carried_item = before_carried_item.clone();
    let mut updated_inventory = before_inventory.clone();
    updated_inventory.slots[9] = ItemStack::new(42, 1);
    let updated_carried_item = ItemStack::new(99, 1);
    let drop_position = Vec3::new(0.5, 65.0, 0.5);
    let player = ContainerPlayerPlan {
        expected_inventory: before_inventory,
        expected_carried_item: before_carried_item,
        updated_inventory: updated_inventory.clone(),
        updated_carried_item: updated_carried_item.clone(),
        crafting_table_input: None,
        enchanting_table_input: None,
        merchant_input: None,
        drops: vec![ContainerDropPlan {
            entity_type_id: 1,
            position: drop_position,
            stack: EntityItemStack::new(99, 1),
        }],
        xp_orb: None,
    };
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let first = handle
        .enqueue(SimulationCommand::CommitChest {
            primary_position: pos,
            positions: vec![pos],
            expected_state_id: 1,
            actor_session: Some(actor),
            expected: vec![initial.clone()],
            updated: vec![first_update.clone()],
            player: Some(Box::new(player.clone())),
            plugin_receipt: None,
        })
        .unwrap();
    let stale = handle
        .enqueue(SimulationCommand::CommitChest {
            primary_position: pos,
            positions: vec![pos],
            expected_state_id: 1,
            actor_session: Some(actor),
            expected: vec![initial],
            updated: vec![stale_update],
            player: Some(Box::new(player)),
            plugin_receipt: None,
        })
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 2)
            .processed,
        2
    );
    assert!(matches!(
        observer_rx.recv().await,
        Some(OutboundCommand::ChestSlots { state_id: 4, .. })
    ));
    assert!(matches!(
        observer_rx.recv().await,
        Some(OutboundCommand::SpawnEntity(entity))
            if entity.item_stack == Some(EntityItemStack::new(99, 1))
    ));
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::ChestCommit(Ok(outcome))
            if matches!(
                *outcome,
                SharedContainerCommit::Committed {
                    state_id: 4,
                    ref inventory,
                    ref carried_item,
                    ..
                } if inventory.slots == updated_inventory.slots
                    && carried_item == &updated_carried_item
            )
    ));
    let (authoritative, rejected_inventory, rejected_carried_item) =
        match stale.await.unwrap().unwrap() {
            SimulationResponse::ChestCommit(Ok(outcome)) => match *outcome {
                SharedContainerCommit::Rejected {
                    state_id,
                    authoritative,
                    inventory,
                    carried_item,
                } => {
                    assert_eq!(state_id, 4);
                    (authoritative, inventory, carried_item)
                }
                other => panic!("expected stale chest rejection, got {other:?}"),
            },
            other => panic!("expected chest response, got {other:?}"),
        };

    assert_eq!(authoritative, vec![first_update.clone()]);
    assert_eq!(rejected_inventory.slots, updated_inventory.slots);
    assert_eq!(rejected_carried_item, updated_carried_item);
    assert_eq!(
        world.lock().await.chest_block_entity(pos).unwrap(),
        Some(first_update)
    );
    let persisted = player_state.lock().unwrap();
    assert_eq!(persisted.inventory.slots, updated_inventory.slots);
    assert_eq!(persisted.carried_item, updated_carried_item);
    drop(persisted);
    let dropped = sessions.persisted_entity_records();
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].position, drop_position);
    assert_eq!(dropped[0].item_stack, Some(EntityItemStack::new(99, 1)));
}

#[tokio::test(flavor = "current_thread")]
async fn queued_furnace_commit_matches_direct_state_and_viewer_version() {
    let mut initial = mc_world::FurnaceBlockEntity::default();
    initial.slots[1] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[1].count = 1;

    let (mut direct_storage, pos) = test_container_storage();
    direct_storage
        .set_furnace_block_entity(pos, initial.clone())
        .unwrap();
    let direct_sessions = SessionRegistry::new();
    let (direct_state_id, _) = direct_sessions
        .try_furnace_slot_dispatches(pos, 1, 7, super::super::furnace_slot_stacks(&updated))
        .unwrap();
    direct_storage
        .set_furnace_block_entity(pos, updated.clone())
        .unwrap();

    let (mut queued_storage, queued_pos) = test_container_storage();
    queued_storage
        .set_furnace_block_entity(queued_pos, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
    let queued_sessions = Arc::new(SessionRegistry::new());
    let session = register_test_session(&queued_sessions, "QueuedFurnaceActor");
    queued_sessions.register_furnace_viewer(session, queued_pos);
    let player = empty_container_player_plan();
    register_test_player_state(&queued_sessions, session, player.expected_inventory.clone());
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request =
        Box::pin(session_handle.commit_furnace(queued_pos, 1, initial, updated, player));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&queued_sessions, Some(&world), None, 1)
            .processed,
        1
    );
    let outcome = request.await.unwrap();
    let queued_state_id = match outcome {
        SharedContainerCommit::Committed {
            state_id,
            dispatches,
            ..
        } => {
            assert!(dispatches.is_empty());
            state_id
        }
        other => panic!("expected committed furnace, got {other:?}"),
    };

    assert_eq!(queued_state_id, direct_state_id);
    assert_eq!(
        world.lock().await.furnace_block_entity(queued_pos).unwrap(),
        direct_storage.furnace_block_entity(pos).unwrap()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn resident_furnace_commit_completes_while_global_world_writer_is_held() {
    let mut initial = mc_world::FurnaceBlockEntity::default();
    initial.slots[1] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut updated = initial.clone();
    updated.slots[1].count = 1;
    let (mut storage, position) = test_container_storage();
    storage
        .set_furnace_block_entity(position, initial.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actor = register_test_session(&sessions, "RegionalFurnaceActor");
    assert_eq!(sessions.register_furnace_viewer(actor, position), 1);
    let player = empty_container_player_plan();
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request =
        Box::pin(session_handle.commit_furnace(position, 1, initial, updated.clone(), player));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("resident furnace completion event")
        .expect("resident furnace response");
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert!(matches!(
        outcome,
        SharedContainerCommit::Committed { state_id: 2, .. }
    ));
    assert_eq!(
        world.lock().await.furnace_block_entity(position).unwrap(),
        Some(updated)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn furnace_output_take_clears_used_recipes_and_spawns_xp_in_one_owner_commit() {
    let mut initial = mc_world::FurnaceBlockEntity::default();
    initial.slots[2] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    initial
        .recipes_used
        .insert("minecraft:test_smelting".to_string(), 1);
    let mut updated = initial.clone();
    updated.slots[2] = mc_world::FurnaceSlot::default();
    updated.recipes_used.clear();

    let (mut storage, position) = test_container_storage();
    storage
        .set_furnace_block_entity(position, initial.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actor = register_test_session(&sessions, "FurnaceXpActor");
    sessions.register_furnace_viewer(actor, position);
    let mut player = empty_container_player_plan();
    player.updated_inventory.slots[9] = ItemStack::new(42, 1);
    let xp_position = Vec3::new(0.5, 64.0, 0.5);
    player.xp_orb = Some(ContainerXpPlan {
        entity_type_id: 49,
        position: xp_position,
        value: 1,
    });
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let mut request =
        Box::pin(session_handle.commit_furnace(position, 1, initial, updated, player));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert!(matches!(
        request.await.unwrap(),
        SharedContainerCommit::Committed { ref inventory, .. }
            if inventory.slots[9] == ItemStack::new(42, 1)
    ));

    let furnace = world
        .lock()
        .await
        .furnace_block_entity(position)
        .unwrap()
        .unwrap();
    assert!(furnace.slots[2].is_empty());
    assert!(furnace.recipes_used.is_empty());
    let entities = sessions.persisted_entity_records();
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].type_name, "minecraft:experience_orb");
    assert_eq!(entities[0].type_id, 49);
    assert_eq!(entities[0].position, xp_position);
    assert_eq!(entities[0].experience_value, Some(1));
}

#[tokio::test(flavor = "current_thread")]
async fn furnace_click_merges_slots_with_newer_owner_tick_data() {
    let mut expected = mc_world::FurnaceBlockEntity {
        burn_remaining: 10,
        burn_total: 10,
        ..mc_world::FurnaceBlockEntity::default()
    };
    expected.slots[1] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let mut current = expected.clone();
    current.burn_remaining = 9;
    current.cook_progress = 1;
    let mut updated = expected.clone();
    updated.slots[1].count = 1;

    let (mut storage, position) = test_container_storage();
    storage
        .set_furnace_block_entity(position, current.clone())
        .unwrap();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let actor = register_test_session(&sessions, "FurnaceTickMerge");
    sessions.register_furnace_viewer(actor, position);
    let player = empty_container_player_plan();
    register_test_player_state(&sessions, actor, player.expected_inventory.clone());
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(actor);
    let request = session_handle.commit_furnace(position, 1, expected, updated.clone(), player);
    tokio::pin!(request);
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_commands_with_world_views(
                &sessions,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    assert!(matches!(
        request.await.unwrap(),
        SharedContainerCommit::Committed { .. }
    ));

    let persisted = world
        .lock()
        .await
        .furnace_block_entity(position)
        .unwrap()
        .unwrap();
    assert_eq!(persisted.slots, updated.slots);
    assert_eq!(persisted.burn_remaining, current.burn_remaining);
    assert_eq!(persisted.cook_progress, current.cook_progress);
}

#[tokio::test]
async fn queued_container_commit_rejects_busy_world_without_mutation() {
    let initial = mc_world::FurnaceBlockEntity::default();
    let mut updated = initial.clone();
    updated.slots[0] = mc_world::FurnaceSlot {
        item_id: 42,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
    };
    let (mut storage, pos) = test_container_storage();
    storage
        .set_furnace_block_entity(pos, initial.clone())
        .unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::CommitFurnace {
            position: pos,
            expected_state_id: 1,
            actor_session: 7,
            expected: initial.clone(),
            updated: Box::new(updated),
            player: Box::new(empty_container_player_plan()),
        })
        .unwrap();
    let mut guard = world.try_lock().unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::FurnaceCommit(Err(SimulationRequestError::WorldBusy))
    ));
    assert_eq!(guard.furnace_block_entity(pos).unwrap(), Some(initial));
}

#[tokio::test(flavor = "current_thread")]
async fn queued_opaque_block_entity_commit_matches_direct_storage_write() {
    let bytes = vec![10, 0, 0, 0];
    let (mut direct_storage, pos, direct_token) = test_block_storage();
    assert!(
        direct_storage
            .commit_opaque_block_entity_conditionally(
                pos,
                BlockStateId(1),
                direct_token,
                bytes.clone(),
            )
            .unwrap()
    );

    let (queued_storage, queued_pos, queued_token) = test_block_storage();
    let read_view = queued_storage.read_view();
    let mutation_view = queued_storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(queued_storage));
    let sessions = Arc::new(SessionRegistry::new());
    let session = register_test_session(&sessions, "QueuedSignActor");
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_opaque_block_entity(
        queued_pos,
        BlockStateId(1),
        queued_token,
        bytes.clone(),
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), request)
            .await
            .expect("resident opaque block-entity completion event")
            .expect("resident opaque block-entity response")
    );
    drop(writer);

    assert_eq!(owner_task.await.unwrap().processed, 1);
    assert_eq!(handle.snapshot().block_entity_commits_processed, 1);
    let queued = world.lock().await;
    let direct = direct_storage
        .cached_chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities
        .get(&pos)
        .cloned();
    let queued = queued
        .cached_chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities
        .get(&queued_pos)
        .cloned();
    assert_eq!(queued, direct);
    assert_eq!(queued, Some(bytes));
}

#[tokio::test]
async fn queued_opaque_block_entity_rejects_stale_token_without_write() {
    let (mut storage, pos, stale_token) = test_block_storage();
    storage.set_block_at(pos, BlockStateId(0)).unwrap();
    storage.set_block_at(pos, BlockStateId(1)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let response = handle
        .enqueue(SimulationCommand::CommitOpaqueBlockEntity {
            position: pos,
            expected_state: BlockStateId(1),
            expected_token: stale_token,
            bytes: vec![10, 0, 0, 0],
        })
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(matches!(
        response.await.unwrap().unwrap(),
        SimulationResponse::OpaqueBlockEntity(Ok(false))
    ));
    assert!(
        !world
            .lock()
            .await
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .contains_key(&pos)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn campfire_use_transaction_survives_requester_loss_after_owner_apply() {
    let input = ItemStack::new(42, 1);
    let result = ItemStack::new(43, 1);
    let expected = super::super::CampfireCookingState::default();
    let mut updated = expected.clone();
    assert!(updated.insert(input, result, 20));
    let bytes = vec![10, 0, 0, 0];
    let client_nbt = mc_nbt::Tag::Compound(Vec::new());
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let (session, _actor_rx) =
        register_test_session_with_outbound(&sessions, "LostCampfireUseRequester");
    let (observer, mut observer_rx) =
        register_test_session_with_outbound(&sessions, "CampfireUseObserver");
    sessions.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
    sessions.replace_view(observer, (0, 0), 2, HashSet::from([(0, 0)]));
    sessions.mark_loaded(session, (0, 0));
    sessions.mark_loaded(observer, (0, 0));
    while observer_rx.try_recv().is_ok() {}
    sessions.clear_closing_sessions_for_test();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 2);
    let player_state = register_test_player_state(&sessions, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
        position: pos,
        expected_state: BlockStateId(1),
        expected_token: token,
        expected_cooking: expected,
        updated_cooking: updated.clone(),
        persistent_bytes: bytes.clone(),
        client_nbt: client_nbt.clone(),
        held_slot: PlayerInventory::HOTBAR_BASE,
        expected_held: ItemStack::new(42, 2),
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    drop(request);

    assert_eq!(sessions.campfire_cooking_state(pos), updated);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert_eq!(
        world
            .lock()
            .await
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .get(&pos),
        Some(&bytes)
    );
    assert!(matches!(
        observer_rx.try_recv(),
        Ok(OutboundCommand::BlockEntityData {
            position: observed_position,
            nbt,
            ..
        }) if observed_position == pos && nbt == client_nbt
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn campfire_use_transaction_rejects_stale_held_stack_without_world_change() {
    let expected = super::super::CampfireCookingState::default();
    let mut updated = expected.clone();
    assert!(updated.insert(ItemStack::new(42, 1), ItemStack::new(43, 1), 20));
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let session = register_test_session(&sessions, "StaleCampfireUseRequester");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&sessions, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
        position: pos,
        expected_state: BlockStateId(1),
        expected_token: token,
        expected_cooking: expected,
        updated_cooking: updated,
        persistent_bytes: vec![10, 0, 0, 0],
        client_nbt: mc_nbt::Tag::Compound(Vec::new()),
        held_slot: PlayerInventory::HOTBAR_BASE,
        expected_held: ItemStack::new(42, 2),
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );

    assert!(request.await.unwrap().is_none());
    assert!(sessions.campfire_cooking_state(pos).is_empty());
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(42, 1)
    );
    assert!(
        !world
            .lock()
            .await
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .contains_key(&pos)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resident_campfire_use_does_not_wait_for_world_writer() {
    let expected = super::super::CampfireCookingState::default();
    let mut updated = expected.clone();
    assert!(updated.insert(ItemStack::new(42, 1), ItemStack::new(43, 1), 20));
    let bytes = vec![10, 0, 0, 0];
    let (storage, pos, token) = test_block_storage();
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = Arc::new(SessionRegistry::new());
    let session = register_test_session(&sessions, "RegionalCampfireActor");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&sessions, session, inventory);
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_campfire_use(CampfireUsePlan {
        position: pos,
        expected_state: BlockStateId(1),
        expected_token: token,
        expected_cooking: expected,
        updated_cooking: updated.clone(),
        persistent_bytes: bytes.clone(),
        client_nbt: mc_nbt::Tag::Compound(Vec::new()),
        held_slot: PlayerInventory::HOTBAR_BASE,
        expected_held: ItemStack::new(42, 1),
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    let writer = world.lock().await;
    let owner_world = Arc::clone(&world);
    let owner_sessions = Arc::clone(&sessions);
    let owner_task = tokio::spawn(async move {
        owner
            .process_commands_with_world_views(
                &owner_sessions,
                Some(&owner_world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
    });

    let completion = tokio::time::timeout(std::time::Duration::from_millis(300), request).await;
    drop(writer);
    assert_eq!(owner_task.await.unwrap().processed, 1);
    let committed = completion
        .expect("resident campfire completion event")
        .expect("campfire response")
        .expect("matching resident campfire use commits");

    assert_eq!(committed.changed_slots.len(), 1);
    assert_eq!(sessions.campfire_cooking_state(pos), updated);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
    let chunk = world
        .lock()
        .await
        .cached_chunk(ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert_eq!(chunk.block_entities.get(&pos), Some(&bytes));
}

#[tokio::test(flavor = "current_thread")]
async fn survival_tnt_ignition_expires_after_exactly_eighty_ticks() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
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
    let tnt = BlockPos { x: 1, y: 64, z: 1 };
    let chained_tnt = BlockPos { x: 2, y: 64, z: 1 };
    let protected = BlockPos { x: 1, y: 64, z: 2 };
    storage.set_block_at(tnt, BlockStateId(5)).unwrap();
    storage.set_block_at(protected, BlockStateId(6)).unwrap();
    let tnt_token = storage.block_mutation_token(tnt).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let sessions = SessionRegistry::new();
    let session = register_test_session(&sessions, "TntIgnitionActor");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let player_state = register_test_player_state(&sessions, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_tnt_ignition(TntIgnitionPlan {
        tnt: BlockEditPrecondition {
            pos: tnt,
            expected_state: BlockStateId(5),
            expected_token: tnt_token,
        },
        air: BlockStateId(0),
        game_mode: GameMode::Survival,
        held_slot: PlayerInventory::HOTBAR_BASE,
        expected_held: ItemStack::new(42, 1),
        flint_and_steel_max_damage: 64,
        tnt_entity_type_id: 132,
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request.await.unwrap().expect("matching ignition commits");
    assert_eq!(committed.block.applied.len(), 1);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE].damage,
        Some(1)
    );
    let saved = sessions.persisted_entity_save_snapshot().0;
    assert_eq!(saved.records.len(), 1, "primed TNT is retained by ECS");
    assert!(saved.records[0].snapshot.retained.primed_tnt.is_some());
    let restored = SessionRegistry::new();
    assert_eq!(restored.restore_persisted_entities(saved), 1);
    assert_eq!(restored.persisted_entity_records().len(), 1);
    assert!(
        restored
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 79)
            .is_empty()
    );
    assert_eq!(
        restored
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 80)
            .len(),
        1,
        "restored TNT must be scheduled in the deadline index"
    );

    world
        .lock()
        .await
        .set_block_at(chained_tnt, BlockStateId(5))
        .unwrap();

    let mut explosion_resistance = vec![0.0; 29_873];
    explosion_resistance[6] = 0.5;
    let block_facts = BlockFactsTable::default().with_explosion_table(
        mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
            .unwrap(),
    );
    let materials = mc_physics::BlockMaterialIds::new(0, None, None);

    owner.advance_world_time(&sessions, 79);
    assert_eq!(
        owner
            .tick_primed_tnt(
                &sessions,
                Some(&world),
                None,
                &block_facts,
                ExplosionRegistries::new(
                    &blocks,
                    &mc_data::items::solaris_required_items(),
                    &mc_data::entity_types::solaris_required_entity_types(),
                ),
                Some(&materials),
                || panic!("protection snapshot must stay lazy without a due explosion"),
            )
            .await,
        0
    );
    assert_eq!(
        world.lock().await.get_block(chained_tnt).unwrap(),
        Some(BlockStateId(5))
    );
    owner.advance_world_time(&sessions, 1);
    let zone_protection = crate::script::ZoneProtectionSnapshot::from_zones(vec![
        ScriptAxisAlignedZone::try_new(
            "protected-test",
            "minecraft:overworld",
            ScriptPosition::try_new(
                f64::from(protected.x),
                f64::from(protected.y),
                f64::from(protected.z),
            )
            .unwrap(),
            ScriptPosition::try_new(
                f64::from(protected.x),
                f64::from(protected.y),
                f64::from(protected.z),
            )
            .unwrap(),
        )
        .unwrap(),
    ]);
    assert_eq!(
        owner
            .tick_primed_tnt(
                &sessions,
                Some(&world),
                None,
                &block_facts,
                ExplosionRegistries::new(
                    &blocks,
                    &mc_data::items::solaris_required_items(),
                    &mc_data::entity_types::solaris_required_entity_types(),
                ),
                Some(&materials),
                || Some(zone_protection),
            )
            .await,
        1
    );
    assert_eq!(
        world.lock().await.get_block(chained_tnt).unwrap(),
        Some(BlockStateId(0))
    );
    assert_eq!(
        world.lock().await.get_block(protected).unwrap(),
        Some(BlockStateId(6)),
        "protection snapshot must remove protected blocks from explosion candidates"
    );
    let chained_fuses = sessions.primed_tnt_fuses_for_test();
    assert_eq!(chained_fuses.len(), 1);
    assert!((90..=109).contains(&chained_fuses[0].1));
    assert_eq!(sessions.persisted_entity_records().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn simultaneous_tnt_explosions_publish_each_drop_before_its_packet() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
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
    let first_dirt = BlockPos { x: 2, y: 64, z: 1 };
    let second_dirt = BlockPos { x: 13, y: 64, z: 1 };
    storage.set_block_at(first_dirt, BlockStateId(6)).unwrap();
    storage.set_block_at(second_dirt, BlockStateId(6)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("TwoTntObserver"),
        name: "TwoTntObserver".to_owned(),
    };
    let (tx, mut outbound) = mpsc::channel(64);
    let session = sessions
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(7.5, 64.0, 8.5),
        )
        .0;
    sessions.replace_view(session, (0, 0), 2, HashSet::from([(0, 0)]));
    assert!(sessions.mark_loaded(session, (0, 0)).is_empty());

    let (_, mut owner) = simulation_channel();
    let mut tnt_ids = Vec::new();
    for position in [Vec3::new(1.5, 64.0, 1.5), Vec3::new(14.5, 64.0, 1.5)] {
        let spawn = sessions.spawn_chained_primed_tnt(
            &owner.authority,
            132,
            position,
            Vec3::ZERO,
            80,
            BlockStateId(0),
        );
        tnt_ids.push(match &spawn[0].command {
            OutboundCommand::SpawnEntity(entity) => entity.id,
            other => panic!("expected TNT spawn, got {other:?}"),
        });
        super::super::dispatch_visibility_commands(spawn);
    }
    while outbound.try_recv().is_ok() {}
    let delayed_spawn = sessions.spawn_command_entity(
        &SimulationAuthority::for_test(),
        1,
        "minecraft:zombie".to_owned(),
        Vec3::new(7.5, 64.0, 7.5),
    );
    assert!(matches!(
        delayed_spawn.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::SpawnEntity(_),
            ..
        }]
    ));

    let mut explosion_resistance = vec![0.0; 29_873];
    explosion_resistance[6] = 0.5;
    let block_facts = BlockFactsTable::default().with_explosion_table(
        mc_data::block_explosion::BlockExplosionTable::from_resistances(explosion_resistance)
            .unwrap(),
    );
    let materials = mc_physics::BlockMaterialIds::new(0, None, None);
    owner.advance_world_time(&sessions, 80);
    for index in 0..2 {
        if index != 0 {
            owner.advance_world_time(&sessions, 1);
        }
        assert_eq!(
            owner
                .tick_primed_tnt(
                    &sessions,
                    Some(&world),
                    None,
                    &block_facts,
                    ExplosionRegistries::new(
                        &blocks,
                        &mc_data::items::solaris_required_items(),
                        &mc_data::entity_types::solaris_required_entity_types(),
                    ),
                    Some(&materials),
                    || None,
                )
                .await,
            1
        );
    }
    assert!(
        outbound.try_recv().is_err(),
        "all TNT world and entity publication must wait for older ordered work"
    );
    dispatch_visibility_commands(delayed_spawn);

    let commands = std::iter::from_fn(|| outbound.try_recv().ok()).collect::<Vec<_>>();
    let explosion_indexes = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            matches!(command, OutboundCommand::Explosion(_)).then_some(index)
        })
        .collect::<Vec<_>>();
    let block_delta_indexes = [first_dirt, second_dirt].map(|position| {
        commands
            .iter()
            .position(|command| match command {
                OutboundCommand::BlockDeltas(deltas) => deltas.iter().any(|delta| {
                    (delta.x, delta.y, delta.z) == (position.x, position.y, position.z)
                }),
                _ => false,
            })
            .expect("matching TNT block delta")
    });
    let drop_indexes = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| match command {
            OutboundCommand::SpawnEntity(entity) if entity.type_name == "minecraft:item" => {
                Some(index)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let despawn_indexes = tnt_ids
            .iter()
            .map(|tnt_id| {
                commands
                    .iter()
                    .position(|command| {
                        matches!(command, OutboundCommand::DespawnEntity(entity) if entity.id == *tnt_id)
                    })
                    .expect("expired TNT despawn")
            })
            .collect::<Vec<_>>();
    assert_eq!(explosion_indexes.len(), 2, "one packet per expired TNT");
    assert_eq!(drop_indexes.len(), 2, "one dirt drop per explosion");
    assert!(block_delta_indexes[0] < drop_indexes[0]);
    assert!(drop_indexes[0] < despawn_indexes[0]);
    assert!(despawn_indexes[0] < explosion_indexes[0]);
    assert!(
        explosion_indexes[0] < block_delta_indexes[1],
        "the second TNT transaction must not begin before the first explosion packet"
    );
    assert!(block_delta_indexes[1] < drop_indexes[1]);
    assert!(drop_indexes[1] < despawn_indexes[1]);
    assert!(despawn_indexes[1] < explosion_indexes[1]);
}

#[tokio::test(flavor = "current_thread")]
async fn expired_tnt_waits_for_its_delayed_spawn_publication() {
    let blocks = BlockRegistry::from_report(&test_block_reports()).unwrap();
    let sessions = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DelayedTntObserver"),
        name: "DelayedTntObserver".to_owned(),
    };
    let (tx, mut outbound) = mpsc::channel(16);
    let session = sessions
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0;
    assert!(sessions.mark_loaded(session, (0, 0)).is_empty());

    let (_, mut owner) = simulation_channel();
    let spawn = sessions.spawn_chained_primed_tnt(
        &owner.authority,
        132,
        Vec3::new(1.5, 64.0, 1.5),
        Vec3::ZERO,
        1,
        BlockStateId(0),
    );
    let tnt_id = match &spawn[0].command {
        OutboundCommand::SpawnEntity(entity) => entity.id,
        other => panic!("expected TNT spawn, got {other:?}"),
    };
    owner.advance_world_time(&sessions, 1);

    assert_eq!(
        owner
            .tick_primed_tnt(
                &sessions,
                None,
                None,
                &BlockFactsTable::default(),
                ExplosionRegistries::new(
                    &blocks,
                    &mc_data::items::solaris_required_items(),
                    &mc_data::entity_types::solaris_required_entity_types(),
                ),
                None,
                || None,
            )
            .await,
        1
    );
    assert!(
        outbound.try_recv().is_err(),
        "TNT terminal packets must wait for its required spawn publication"
    );

    dispatch_visibility_commands(spawn);
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::SpawnEntity(entity)) if entity.id == tnt_id
    ));
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::DespawnEntity(entity)) if entity.id == tnt_id
    ));
    assert!(matches!(
        outbound.try_recv(),
        Ok(OutboundCommand::Explosion(_))
    ));
    assert!(outbound.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn creative_offhand_tnt_ignition_does_not_mutate_inventory() {
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
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
    let tnt = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(tnt, BlockStateId(5)).unwrap();
    let tnt_token = storage.block_mutation_token(tnt).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let sessions = SessionRegistry::new();
    let session = register_test_session(&sessions, "CreativeOffhandTntActor");
    let mut inventory = PlayerInventory::empty();
    let mut flint_and_steel = ItemStack::new(42, 1);
    flint_and_steel.damage = Some(7);
    inventory.slots[PlayerInventory::OFFHAND_SLOT] = flint_and_steel.clone();
    let player_state = register_test_player_state(&sessions, session, inventory.clone());
    player_state.lock().unwrap().game_mode = GameMode::Creative;

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session);
    let mut request = Box::pin(session_handle.commit_tnt_ignition(TntIgnitionPlan {
        tnt: BlockEditPrecondition {
            pos: tnt,
            expected_state: BlockStateId(5),
            expected_token: tnt_token,
        },
        air: BlockStateId(0),
        game_mode: GameMode::Creative,
        held_slot: PlayerInventory::OFFHAND_SLOT,
        expected_held: flint_and_steel,
        flint_and_steel_max_damage: 64,
        tnt_entity_type_id: 132,
    }));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    let committed = request.await.unwrap().expect("matching ignition commits");

    assert!(committed.changed_slots.is_empty());
    assert_eq!(committed.inventory.slots, inventory.slots);
    assert_eq!(
        player_state.lock().unwrap().inventory.slots,
        inventory.slots
    );
    assert_eq!(
        world.lock().await.get_block(tnt).unwrap(),
        Some(BlockStateId(0))
    );
}

#[tokio::test]
async fn queued_campfire_commits_accept_only_first_matching_snapshot() {
    let expected = super::super::CampfireCookingState::default();
    let mut first_update = expected.clone();
    assert!(first_update.insert(
        super::super::ItemStack::new(42, 1),
        super::super::ItemStack::new(43, 1),
        20,
    ));
    let mut stale_update = expected.clone();
    assert!(stale_update.insert(
        super::super::ItemStack::new(44, 1),
        super::super::ItemStack::new(45, 1),
        20,
    ));
    let (storage, pos, token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let first_actor = register_test_session(&sessions, "FirstCampfireActor");
    let stale_actor = register_test_session(&sessions, "StaleCampfireActor");
    let mut first_inventory = PlayerInventory::empty();
    first_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 1);
    let first_player = register_test_player_state(&sessions, first_actor, first_inventory);
    let mut stale_inventory = PlayerInventory::empty();
    stale_inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(44, 1);
    let stale_player = register_test_player_state(&sessions, stale_actor, stale_inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(2);
    let first = handle
        .for_session(first_actor)
        .enqueue_player_command(SimulationCommand::CommitCampfireUse(Box::new(
            CampfireUseCommand {
                actor_session: first_actor,
                plan: CampfireUsePlan {
                    position: pos,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                    expected_cooking: expected.clone(),
                    updated_cooking: first_update.clone(),
                    persistent_bytes: vec![1],
                    client_nbt: mc_nbt::Tag::Compound(Vec::new()),
                    held_slot: PlayerInventory::HOTBAR_BASE,
                    expected_held: ItemStack::new(42, 1),
                },
            },
        )))
        .unwrap();
    let stale = handle
        .for_session(stale_actor)
        .enqueue_player_command(SimulationCommand::CommitCampfireUse(Box::new(
            CampfireUseCommand {
                actor_session: stale_actor,
                plan: CampfireUsePlan {
                    position: pos,
                    expected_state: BlockStateId(1),
                    expected_token: token,
                    expected_cooking: expected,
                    updated_cooking: stale_update,
                    persistent_bytes: vec![2],
                    client_nbt: mc_nbt::Tag::Compound(Vec::new()),
                    held_slot: PlayerInventory::HOTBAR_BASE,
                    expected_held: ItemStack::new(44, 1),
                },
            },
        )))
        .unwrap();

    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 2)
            .processed,
        2
    );
    assert!(matches!(
        first.await.unwrap().unwrap(),
        SimulationResponse::CampfireUse(Ok(Some(_)))
    ));
    assert!(matches!(
        stale.await.unwrap().unwrap(),
        SimulationResponse::CampfireUse(Ok(None))
    ));
    assert_eq!(sessions.campfire_cooking_state(pos), first_update);
    assert_eq!(
        first_player.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
    assert_eq!(
        stale_player.lock().unwrap().inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(44, 1)
    );
    assert_eq!(
        world
            .lock()
            .await
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .get(&pos)
            .cloned(),
        Some(vec![1])
    );
}

#[tokio::test]
async fn owner_budget_carries_remaining_commands_to_next_tick() {
    let registry = SessionRegistry::new();
    let position = Vec3::new(0.5, 64.0, 0.5);
    for value in 1..=3 {
        registry.spawn_xp_orb(2, position, value);
    }
    let ids = registry
        .nearby_experience_entities(position, 2.25)
        .into_iter()
        .map(|entity| entity.id)
        .collect::<Vec<_>>();
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    let mut responses = ids
        .into_iter()
        .map(|entity_id| {
            handle
                .enqueue(SimulationCommand::ClaimExperiencePickup {
                    entity_id,
                    collector_session: 7,
                })
                .unwrap()
        })
        .collect::<Vec<_>>();

    assert_eq!(owner.process_tick(&registry, 2).processed, 2);
    assert_eq!(handle.snapshot().depth, 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), &mut responses[2])
            .await
            .is_err()
    );
    assert_eq!(owner.process_tick(&registry, 2).processed, 1);
    assert_eq!(handle.snapshot().depth, 0);
    assert_eq!(handle.snapshot().processed, 3);
}

#[tokio::test]
async fn cancelled_and_shutdown_commands_do_not_mutate_entities() {
    let cancelled_registry = SessionRegistry::new();
    let (_, cancelled_xp) = seed_claim_entities(&cancelled_registry);
    let (cancelled_handle, mut cancelled_owner) = simulation_channel_with_capacity(1);
    let cancelled_response = cancelled_handle
        .enqueue(SimulationCommand::ClaimExperiencePickup {
            entity_id: cancelled_xp,
            collector_session: 7,
        })
        .unwrap();
    drop(cancelled_response);
    assert_eq!(
        cancelled_owner
            .process_tick(&cancelled_registry, 1)
            .processed,
        0
    );
    assert_eq!(
        cancelled_registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );

    let shutdown_registry = SessionRegistry::new();
    let (_, shutdown_xp) = seed_claim_entities(&shutdown_registry);
    let (shutdown_handle, mut shutdown_owner) = simulation_channel_with_capacity(1);
    let shutdown_response = shutdown_handle
        .enqueue(SimulationCommand::ClaimExperiencePickup {
            entity_id: shutdown_xp,
            collector_session: 7,
        })
        .unwrap();
    shutdown_owner.shutdown();
    assert_eq!(
        shutdown_response.await.unwrap().unwrap_err(),
        SimulationRequestError::ShuttingDown
    );
    assert_eq!(
        shutdown_registry
            .nearby_experience_entities(Vec3::new(0.5, 64.0, 0.5), 2.25)
            .len(),
        1
    );
}

/// One server-owned warehouse deposit commits the container and the actor's
/// inventory, journals the container's own after-image together with the
/// encoded plugin receipt in ONE decision - appended BEFORE the container's
/// slots are published - and leaves both participants recoverable: the
/// images rebuild the container and the batch rebuilds the plugin ledger and
/// the player's file, and only then is the decision acknowledged.
#[tokio::test(flavor = "current_thread")]
async fn server_owned_warehouse_deposit_appends_before_publishing_and_recovers_both() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let item_id = items
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("the baseline items hold the moved stack");
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk_position = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_position,
            Chunk::empty(
                chunk_position,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        count: 12,
        item_id,
        ..mc_world::FurnaceSlot::EMPTY
    };
    assert!(
        storage
            .set_chest_block_entity(position, chest.clone())
            .unwrap()
    );
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let registry = SessionRegistry::new();
    let (depositor, mut depositor_outbound) =
        register_test_session_with_outbound(&registry, "WarehouseDepositor");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(item_id, 10);
    let carried_item = ItemStack::new(item_id, 1);
    let state = register_test_player_state(&registry, depositor, inventory.clone());
    state.lock().unwrap().carried_item = carried_item.clone();
    // The actor is a viewer of the container, and a second client watches it.
    let (observer, mut observer_outbound) =
        register_test_session_with_outbound(&registry, "WarehouseObserver");
    assert_eq!(registry.register_chest_viewer(depositor, position), 1);
    assert_eq!(registry.register_chest_viewer(observer, position), 1);

    let (journal, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    registry.install_world_chunk_journal(journal);

    // The plugin receipt the deposit journals beside the container image.
    let recovery = crate::play::persistence::inventory_recovery::PlayerInventoryRecovery::capture(
        crate::login::offline_uuid("WarehouseDepositor"),
        &state.lock().unwrap(),
        &{
            let mut planned = PlayerInventory::empty();
            planned.slots[9] = ItemStack::new(item_id, 6);
            planned
        },
        &items,
    )
    .unwrap();
    let batch: crate::script::storage::PreparedStorageBatch =
        serde_json::from_value(serde_json::json!({
            "transaction_id": 1,
            "plugin_id": "settlement",
            "mutations": [{
                "kind": "compare_and_swap",
                "key": "balance",
                "expected_version": null,
                "value": "10"
            }],
            "inventory": recovery
        }))
        .unwrap();
    let receipt = batch.encode_world_inventory().unwrap();

    let mut updated_chest = chest.clone();
    updated_chest.slots[1] = mc_world::FurnaceSlot {
        count: 4,
        item_id,
        ..mc_world::FurnaceSlot::EMPTY
    };
    let mut updated_inventory = PlayerInventory::empty();
    updated_inventory.slots[9] = ItemStack::new(item_id, 6);
    let request = WarehouseTransferRequest {
        position,
        expected_state_id: registry.chest_state_id(position),
        expected_container: chest
            .slots
            .iter()
            .map(crate::play::owned_inventory::container_slot_to_item)
            .collect(),
        updated_container: updated_chest
            .slots
            .iter()
            .map(crate::play::owned_inventory::container_slot_to_item)
            .collect(),
        player: Some(crate::play::owned_inventory::WarehousePlayerParticipant {
            actor_id: depositor,
            expected_inventory: inventory.slots.to_vec(),
            expected_carried_item: carried_item.clone(),
            updated_inventory: updated_inventory.slots.to_vec(),
            updated_carried_item: carried_item,
        }),
        receipt,
    };

    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let (result, report) = tokio::join!(
        handle.commit_warehouse_transfer(request),
        owner.process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            1,
        )
    );
    assert_eq!(report.processed, 1);
    let decision_id = match result.expect("the deposit reached the owner") {
        WarehouseTransferOutcome::Committed { decision_id } => decision_id,
        other => panic!("expected a committed deposit, got {other:?}"),
    };

    // The container moved, its state id advanced, and every viewer — the
    // actor included — received its slots.
    let live = read_view.snapshot_chunks(&[chunk_position]);
    let live_chest = live.chunk(chunk_position).unwrap().chests[&position].clone();
    assert_eq!(live_chest.slots[0].count, 12);
    assert_eq!(live_chest.slots[1].item_id, item_id);
    assert_eq!(live_chest.slots[1].count, 4);
    let published_state_id = registry.chest_state_id(position);
    assert!(
        published_state_id > 1,
        "the deposit advanced the container's state id"
    );
    for (name, outbound) in [
        ("depositor", &mut depositor_outbound),
        ("observer", &mut observer_outbound),
    ] {
        let mut published = None;
        while let Ok(command) = outbound.try_recv() {
            if let OutboundCommand::ChestSlots {
                state_id,
                slots: sent,
                ..
            } = command
            {
                published = Some((state_id, sent));
                break;
            }
        }
        let (state_id, slots) =
            published.unwrap_or_else(|| panic!("{name} received the container's slots"));
        assert_eq!(
            state_id, published_state_id,
            "{name} sees the advanced state id"
        );
        assert_eq!(slots[1].count, 4, "{name} sees the deposit");
    }
    let live_state = state.lock().unwrap();
    assert_eq!(live_state.inventory.slots[9], ItemStack::new(item_id, 6));

    // ONE decision carries the container's own after-image and the receipt.
    let live_journal = registry.world_chunk_journal().unwrap();
    // The append-before-publication rule: the observation is taken inside
    // the publication path, where the container's `ChestSlots` command
    // leaves the run, and it records that this decision was already
    // appended. An inverted order records `false` here.
    assert_eq!(
        live_journal.warehouse_publications_for_test(),
        vec![(decision_id, true)],
        "the deposit's decision must be appended before its slots are published"
    );
    let decisions = live_journal.pending_decisions_for_test();
    assert_eq!(decisions.len(), 1, "one deposit, one decision");
    assert_eq!(decisions[0].id(), decision_id);
    assert!(
        decisions[0].inventory_batch().unwrap().is_some(),
        "the encoded receipt rides the container's decision"
    );
    let images = live_journal.decode_pending(&decisions).unwrap();
    assert_eq!(images.len(), 1);
    let image_chest = images[0].chests[&position].clone();
    assert_eq!(
        image_chest.slots[1].count, 4,
        "the decision carries the POST-deposit container image"
    );
    assert!(
        registry
            .world_chunk_journal()
            .unwrap()
            .watermark()
            .is_none(),
        "an unprojected decision blocks the checkpoint cutoff"
    );
    drop(live_state);

    // Reopen the journal as a restart does: the images rebuild the
    // container and the batch replays both participants, which is what lets
    // the decision be acknowledged.
    drop(live_journal);
    drop(registry);
    let (reopened, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    let restored = reopened.decode_pending(&pending).unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored[0].chests[&position].slots[1].count, 4,
        "the container recovers from the decision's own image"
    );
    assert_eq!(restored[0].chests[&position].slots[1].item_id, item_id);
    // The restarted runtime replays the decision's participants: the plugin
    // ledger batch installs and the player's own after-image is written, so
    // the decision is projected and the checkpoint cutoff can pass it.
    let registry = Arc::new(SessionRegistry::new());
    registry.install_world_chunk_journal(reopened);
    let runtime = crate::script::storage::world_inventory::InventoryRuntime::new(
        Some(temp.path()),
        &crate::server::ShutdownHandle::default(),
        Arc::clone(&registry),
        Arc::clone(&items),
        Arc::new(mc_data::item_components::solaris_required_item_facts()),
    );
    // The plugin ledger is its own store, not the world's own directory.
    let ledger_root = tempfile::tempdir().unwrap();
    let mut ledger = crate::script::storage::PluginStorage::open(ledger_root.path()).unwrap();
    runtime
        .recover(&mut ledger)
        .expect("both participants of the decision replay");
    let journal = registry.world_chunk_journal().unwrap();
    assert_eq!(
        journal.watermark(),
        Some(decision_id),
        "a replayed decision is projected and can be checkpointed"
    );
}

/// A stale container fence, a stale player fence and a rejected conditional
/// commit each answer their own typed refusal, leave the container and the
/// actor untouched and close the reserved decision with no participant.
#[tokio::test(flavor = "current_thread")]
async fn server_owned_warehouse_deposit_refuses_stale_fences_without_mutating() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("region")).unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let item_id = items
        .id_of(&Identifier::parse("minecraft:birch_log").unwrap())
        .expect("the baseline items hold the moved stack");
    let mut storage = WorldStorage::open(temp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_item_registry(Arc::clone(&items));
    let chunk_position = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            chunk_position,
            Chunk::empty(
                chunk_position,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let position = BlockPos { x: 1, y: 64, z: 1 };
    storage.set_block_at(position, BlockStateId(1)).unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        count: 12,
        item_id,
        ..mc_world::FurnaceSlot::EMPTY
    };
    assert!(
        storage
            .set_chest_block_entity(position, chest.clone())
            .unwrap()
    );
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));

    let registry = SessionRegistry::new();
    let (depositor, _outbound) = register_test_session_with_outbound(&registry, "WarehouseRefused");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(item_id, 10);
    let state = register_test_player_state(&registry, depositor, inventory.clone());
    assert_eq!(registry.register_chest_viewer(depositor, position), 1);
    let (journal, pending) = crate::play::world_journal::WorldChunkJournal::open_for_test(
        temp.path(),
        Arc::clone(&blocks),
        Arc::clone(&items),
    )
    .unwrap();
    assert!(pending.is_empty());
    registry.install_world_chunk_journal(journal);

    let mut updated_chest = chest.clone();
    updated_chest.slots[1] = mc_world::FurnaceSlot {
        count: 4,
        item_id,
        ..mc_world::FurnaceSlot::EMPTY
    };
    let mut updated_inventory = PlayerInventory::empty();
    updated_inventory.slots[9] = ItemStack::new(item_id, 6);
    let receipt = |registry: &SessionRegistry| {
        let recovery =
            crate::play::persistence::inventory_recovery::PlayerInventoryRecovery::capture(
                crate::login::offline_uuid("WarehouseRefused"),
                &state.lock().unwrap(),
                &updated_inventory,
                &items,
            )
            .unwrap();
        let batch: crate::script::storage::PreparedStorageBatch =
            serde_json::from_value(serde_json::json!({
                "transaction_id": 1,
                "plugin_id": "settlement",
                "mutations": [{
                    "kind": "compare_and_swap",
                    "key": "balance",
                    "expected_version": null,
                    "value": "10"
                }],
                "inventory": recovery
            }))
            .unwrap();
        let _ = registry;
        batch.encode_world_inventory().unwrap()
    };
    let container_items = |entity: &ChestBlockEntity| {
        entity
            .slots
            .iter()
            .map(crate::play::owned_inventory::container_slot_to_item)
            .collect::<Vec<_>>()
    };

    // A stale container state-id fence, then a conditional commit the world
    // itself rejects, then a stale player fence.
    let request = |expected_state_id: i32,
                   expected_container: Vec<ItemStack>,
                   expected_inventory: Vec<ItemStack>,
                   expected_carried_item: ItemStack| WarehouseTransferRequest {
        position,
        expected_state_id,
        expected_container,
        updated_container: container_items(&updated_chest),
        player: Some(crate::play::owned_inventory::WarehousePlayerParticipant {
            actor_id: depositor,
            expected_inventory,
            expected_carried_item,
            updated_inventory: updated_inventory.slots.to_vec(),
            updated_carried_item: ItemStack::EMPTY,
        }),
        receipt: receipt(&registry),
    };
    let live_state_id = registry.chest_state_id(position);
    let stale_state_id = request(
        live_state_id + 1,
        container_items(&chest),
        inventory.slots.to_vec(),
        ItemStack::EMPTY,
    );
    let rejected_commit = request(
        live_state_id,
        container_items(&updated_chest),
        inventory.slots.to_vec(),
        ItemStack::EMPTY,
    );
    let stale_player = request(
        live_state_id,
        container_items(&chest),
        PlayerInventory::empty().slots.to_vec(),
        ItemStack::EMPTY,
    );

    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let (handle, mut owner) = simulation_channel_with_capacity(3);
    let (first, second, third, report) = tokio::join!(
        handle.commit_warehouse_transfer(stale_state_id),
        handle.commit_warehouse_transfer(rejected_commit),
        handle.commit_warehouse_transfer(stale_player),
        owner.process_commands_with_world_views(
            &registry,
            Some(&world),
            SimulationWorldAccess {
                read: Some(&read_view),
                mutation: Some(&mutation_view),
                cpu: Some(&resources),
                light: None,
            },
            None,
            3,
        )
    );
    assert_eq!(report.processed, 3);
    assert_eq!(first.unwrap(), WarehouseTransferOutcome::StaleContainer);
    assert_eq!(second.unwrap(), WarehouseTransferOutcome::StaleContainer);
    assert_eq!(third.unwrap(), WarehouseTransferOutcome::StalePlayer);

    // Nothing moved, nothing published, and each reserved id was closed
    // with no participant so the run's append stayed contiguous.
    let live = read_view.snapshot_chunks(&[chunk_position]);
    let live_chest = live.chunk(chunk_position).unwrap().chests[&position].clone();
    assert_eq!(live_chest.slots[0].count, 12);
    assert!(live_chest.slots[1].is_empty());
    assert_eq!(registry.chest_state_id(position), 1);
    assert_eq!(
        state.lock().unwrap().inventory.slots[9],
        ItemStack::new(item_id, 10)
    );
    let decisions = registry
        .world_chunk_journal()
        .unwrap()
        .pending_decisions_for_test();
    assert_eq!(decisions.len(), 3);
    for decision in &decisions {
        assert!(
            decision.inventory_batch().unwrap().is_none(),
            "a refused deposit journals no receipt"
        );
    }
}
