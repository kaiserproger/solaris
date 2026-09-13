use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_data::ItemStack;
use mc_data::item_components::{ItemFactsTable, solaris_required_item_facts};
use mc_data::items::{ItemRegistry, solaris_required_items};
use mc_entity::Vec3;
use mc_script::{
    ScriptBlockPosition, ScriptFormation, ScriptFormationKind, ScriptInventoryEndpoint,
    ScriptInventoryExpectedRevision, ScriptInventoryFence, ScriptInventoryMaterial,
    ScriptInventoryReservationSnapshot, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOperationState, ScriptOwnedInventoryOperation,
    ScriptOwnedInventoryResult, ScriptOwnedInventorySnapshot, ScriptOwnedItemTransfer,
    ScriptResidentOrder, ScriptResidentOrderOperation, resident_generation_id,
    resident_handle_for_generation,
};
use tokio::sync::mpsc as tokio_mpsc;

use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::play::inventory::PlayerInventory;
use crate::play::owned_inventory::owned_inventory_snapshot;
use crate::play::persistence::PlayerPersistedState;
use crate::play::persistence::{load_player_state, save_player_state};
use crate::play::session::SessionRegistry;
use crate::script::storage::PluginStorage;
use crate::script::storage::PluginStorageMutationError;
use crate::script::storage::StorageFaultPoint;
use crate::script::storage::world_inventory::InventoryRuntime;

use super::owned_inventory_endpoint::reservation_stock_survives;

const OWNER: &str = "settlement";

struct Harness {
    _outbound: tokio_mpsc::Receiver<crate::play::session::outbound::OutboundCommand>,
    _root: tempfile::TempDir,
    runtime: InventoryRuntime,
    storage: PluginStorage,
    player_id: u64,
    state: Arc<Mutex<PlayerPersistedState>>,
    items: Arc<ItemRegistry>,
    _facts: Arc<ItemFactsTable>,
}

impl Harness {
    fn new(inventory: PlayerInventory) -> Self {
        let root = tempfile::tempdir().unwrap();
        let items = Arc::new(solaris_required_items());
        let facts = Arc::new(solaris_required_item_facts());
        let state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        )));
        state.lock().unwrap().inventory = inventory;
        let storage = PluginStorage::open(root.path()).unwrap();
        let sessions = Arc::new(SessionRegistry::new());
        let runtime = InventoryRuntime::player_only_for_test(
            root.path(),
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let journal = sessions.world_chunk_journal().unwrap();
        journal.reserve_decision_ids(1).unwrap();
        journal
            .record_reserved_snapshot_groups(0, vec![(1, Vec::new())])
            .unwrap();
        let (outbound, receiver) = tokio_mpsc::channel(8);
        let (player_id, _) = sessions.register(
            &LoggedInProfile {
                uuid: crate::login::offline_uuid("OwnedInventory"),
                name: "OwnedInventory".to_owned(),
            },
            (0, 0),
            2,
            HashSet::new(),
            outbound,
            PlayerPose::new(0.5, 64.0, 0.5),
        );
        sessions.register_player_persistence(player_id, Arc::clone(&state));
        Self {
            _outbound: receiver,
            _root: root,
            runtime,
            storage,
            player_id,
            state,
            items,
            _facts: facts,
        }
    }

    fn endpoint(&self) -> ScriptInventoryEndpoint {
        ScriptInventoryEndpoint::PlayerInventory {
            player_id: self.player_id,
        }
    }

    fn fence(&self) -> ScriptInventoryFence {
        let state = self.state.lock().unwrap();
        owned_inventory_snapshot(
            self.endpoint(),
            state.inventory_operation_revision,
            &state.inventory.slots,
            &self.items,
        )
        .unwrap()
        .fence
    }

    async fn execute(
        &mut self,
        request: &ScriptOperationRequest,
    ) -> mc_script::ScriptOperationOutcome {
        self.runtime
            .execute_owned_inventory(&mut self.storage, OWNER, request)
            .await
            .unwrap()
    }

    fn stack(&self, resource: &str, count: i32) -> ItemStack {
        let id = self
            .items
            .id_of(&Identifier::parse(resource.to_owned()).unwrap())
            .unwrap();
        ItemStack::new(id, count)
    }

    /// Materialise one durable resident under `plugin` and return its handle.
    async fn materialize(&mut self, plugin: &str, slot: u32) -> String {
        let generation = resident_generation_id("owned-inventory-world", "site-1", slot).unwrap();
        let handle = resident_handle_for_generation(plugin, &generation).unwrap();
        let snapshot = self
            .runtime
            .materialize_resident(
                &mut self.storage,
                plugin,
                &generation,
                Vec3::new(0.5, 64.0, 0.5),
            )
            .await
            .expect("the resident materialises");
        assert_eq!(snapshot.handle, handle);
        handle
    }

    async fn query(&mut self, endpoint: ScriptInventoryEndpoint) -> ScriptOwnedInventorySnapshot {
        let request = ScriptOperationRequest::try_new(
            "query-request",
            ScriptOperation::Inventory {
                operation: ScriptOwnedInventoryOperation::Query {
                    endpoint,
                    expected_revision: None,
                },
            },
        )
        .unwrap();
        let outcome = self.execute(&request).await;
        assert_eq!(outcome.failure(), None, "query: {outcome:?}");
        match outcome.payload() {
            ScriptOperationPayload::OwnedInventory { result } => match &**result {
                ScriptOwnedInventoryResult::Snapshot { inventory } => inventory.clone(),
                other => panic!("expected a snapshot, got {other:?}"),
            },
            other => panic!("expected an owned inventory payload, got {other:?}"),
        }
    }

    async fn execute_order(&mut self, request: &ScriptOperationRequest) -> ScriptOperationOutcome {
        self.runtime
            .execute_resident_order_operation(&mut self.storage, OWNER, request)
            .await
            .expect("resident order reaches the durable boundary")
    }
}

fn resident_endpoint(handle: &str, equipment: bool) -> ScriptInventoryEndpoint {
    if equipment {
        ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.to_owned(),
        }
    } else {
        ScriptInventoryEndpoint::ResidentCarry {
            handle: handle.to_owned(),
        }
    }
}

fn order_request(
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
    order: ScriptResidentOrder,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "order-request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::IssueOrder {
                operation_id: operation_id.to_owned(),
                handles: vec![handle.to_owned()],
                expected_order_revisions: vec![expected_revision],
                order,
            },
        },
    )
    .unwrap()
}

fn transfer_request(
    actor_id: u64,
    transfers: Vec<ScriptOwnedItemTransfer>,
    fences: Vec<(ScriptInventoryEndpoint, ScriptInventoryFence)>,
) -> ScriptOperationRequest {
    transfer_request_id("transfer-operation", actor_id, transfers, fences)
}

fn transfer_request_id(
    operation_id: &str,
    actor_id: u64,
    transfers: Vec<ScriptOwnedItemTransfer>,
    fences: Vec<(ScriptInventoryEndpoint, ScriptInventoryFence)>,
) -> ScriptOperationRequest {
    let expected_revisions = fences
        .into_iter()
        .map(|(endpoint, fence)| ScriptInventoryExpectedRevision::new(endpoint, fence))
        .collect();
    ScriptOperationRequest::try_new(
        "transfer-request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Transfer {
                operation_id: operation_id.to_owned(),
                actor_id,
                transfers,
                expected_revisions,
            },
        },
    )
    .unwrap()
}

fn reserve_request(
    endpoint: ScriptInventoryEndpoint,
    plan: ScriptInventoryResourcePlan,
    fence: ScriptInventoryFence,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "reserve-request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "reserve-operation".to_owned(),
                endpoint,
                resource_plan: plan,
                expected_revision: fence,
            },
        },
    )
    .unwrap()
}

fn release_request(
    operation_id: &str,
    reservation_ref: &str,
    expected_revision: u64,
) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "release-request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Release {
                operation_id: operation_id.to_owned(),
                reservation_ref: reservation_ref.to_owned(),
                expected_revision,
            },
        },
    )
    .unwrap()
}

fn status_request(reservation_ref: &str) -> ScriptOperationRequest {
    ScriptOperationRequest::try_new(
        "status-request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::ReservationStatus {
                reservation_ref: reservation_ref.to_owned(),
            },
        },
    )
    .unwrap()
}

fn transfer_fences(
    outcome: &mc_script::ScriptOperationOutcome,
) -> Vec<ScriptInventoryExpectedRevision> {
    match outcome.payload() {
        ScriptOperationPayload::OwnedInventory { result }
            if matches!(&**result, ScriptOwnedInventoryResult::Transfer { .. }) =>
        {
            let ScriptOwnedInventoryResult::Transfer { inventories } = &**result else {
                unreachable!("guarded by the match arm")
            };
            inventories.clone()
        }
        payload => panic!("expected transfer payload, got {payload:?}"),
    }
}

fn reservation_of(
    outcome: &mc_script::ScriptOperationOutcome,
) -> ScriptInventoryReservationSnapshot {
    match outcome.payload() {
        ScriptOperationPayload::OwnedInventory { result } => {
            let ScriptOwnedInventoryResult::Reservation { reservation } = &**result else {
                panic!("expected reservation payload, got {result:?}")
            };
            reservation.clone()
        }
        payload => panic!("expected reservation payload, got {payload:?}"),
    }
}

#[tokio::test]
async fn owned_transfer_moves_exactly_the_requested_items_and_preserves_components() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let sword = {
        let items = solaris_required_items();
        ItemStack::new(
            items
                .id_of(&Identifier::parse("minecraft:iron_sword").unwrap())
                .unwrap(),
            1,
        )
        .with_damage(17)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 2)
        .with_custom_name("Settler's blade")
        .with_item_model(Identifier::parse("minecraft:diamond_sword").unwrap())
    };
    harness.state.lock().unwrap().inventory.slots[9] = sword.clone();
    let player = harness.endpoint();
    let fence = harness.fence();
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            player.clone(),
            10,
            1,
        )],
        vec![(player.clone(), fence)],
    );

    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    {
        let state = harness.state.lock().unwrap();
        assert_eq!(state.inventory.slots[9], ItemStack::EMPTY);
        assert_eq!(state.inventory.slots[10], sword);
        assert_eq!(
            state.inventory_operation_revision,
            result_revision(&outcome)
        );
    }
    let fences = transfer_fences(&outcome);
    assert_eq!(fences.len(), 1);
    assert_eq!(fences[0].endpoint, player);
    let watermark = harness.state.lock().unwrap().inventory_operation_revision;
    assert_eq!(fences[0].fence.revision, watermark);

    // Replaying the same operation id returns the stored result and moves nothing twice.
    let replay = harness.execute(&request).await;
    assert_eq!(replay, outcome);
    let state = harness.state.lock().unwrap();
    assert_eq!(state.inventory.slots[9], ItemStack::EMPTY);
    assert_eq!(state.inventory.slots[10], sword);
}

#[tokio::test]
async fn owned_transfer_rejects_stale_foreign_and_unavailable_endpoints() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let apple = harness.stack("minecraft:apple", 4);
    harness.state.lock().unwrap().inventory.slots[9] = apple.clone();
    let player = harness.endpoint();
    let fence = harness.fence();

    let stale =
        ScriptInventoryFence::try_new(fence.revision + 1, fence.snapshot_hash.clone()).unwrap();
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            player.clone(),
            10,
            1,
        )],
        vec![(player.clone(), stale)],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);

    let foreign = ScriptInventoryEndpoint::PlayerInventory { player_id: 9_999 };
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            foreign.clone(),
            9,
            player.clone(),
            10,
            1,
        )],
        vec![(foreign, fence.clone()), (player.clone(), fence.clone())],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);

    let warehouse = ScriptInventoryEndpoint::Warehouse {
        handle: "completed-container".to_owned(),
    };
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            warehouse.clone(),
            0,
            1,
        )],
        vec![(player.clone(), fence), (warehouse, harness.fence())],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);
}

/// The warehouse read path landing must not open the reservation path: a
/// warehouse endpoint still refuses exactly as before.
#[tokio::test]
async fn owned_reservation_refuses_a_warehouse_endpoint() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let warehouse = ScriptInventoryEndpoint::Warehouse {
        handle: "completed-container".to_owned(),
    };
    let name = harness
        .items
        .name_of(harness.stack("minecraft:apple", 1).item_id)
        .unwrap()
        .as_str()
        .to_owned();
    let plan = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        1,
        vec![ScriptInventoryMaterial::new(name, 1)],
    )]);
    let request = reserve_request(warehouse, plan, harness.fence());

    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Unloaded));
}

#[tokio::test]
async fn owned_reservation_release_returns_only_the_unconsumed_remainder() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let apples = harness.stack("minecraft:apple", 8);
    harness.state.lock().unwrap().inventory.slots[9] = apples;
    let player = harness.endpoint();
    let name = harness
        .items
        .name_of(harness.stack("minecraft:apple", 1).item_id)
        .unwrap()
        .as_str()
        .to_owned();
    let plan = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        4,
        vec![ScriptInventoryMaterial::new(name, 5)],
    )]);
    let request = reserve_request(player.clone(), plan.clone(), harness.fence());

    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.state(), ScriptOperationState::Committed);
    let reservation = reservation_of(&outcome);
    assert_eq!(reservation.endpoint, player);
    assert_eq!(reservation.quantities.len(), 1);
    let quantity = &reservation.quantities[0];
    assert_eq!(
        (
            quantity.reserved,
            quantity.consumed,
            quantity.returned,
            quantity.remaining
        ),
        (5, 0, 0, 5)
    );
    assert!(!reservation.released);

    // The same operation id replays the stored reservation instead of doubling it.
    let replay = harness.execute(&request).await;
    assert_eq!(replay, outcome);
    let reservation_ref = reservation.reservation_ref.clone();
    let status = harness.execute(&status_request(&reservation_ref)).await;
    assert_eq!(reservation_of(&status), reservation);

    // Reusing the operation id with a different canonical plan conflicts.
    let conflicting = reserve_request(
        player,
        ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
            4,
            vec![ScriptInventoryMaterial::new(
                harness
                    .items
                    .name_of(harness.stack("minecraft:apple", 1).item_id)
                    .unwrap()
                    .as_str()
                    .to_owned(),
                4,
            )],
        )]),
        harness.fence(),
    );
    let outcome = harness.execute(&conflicting).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );

    // Release is serialized with consumption and returns only the un-consumed part.
    let release = release_request(
        "release-operation",
        &reservation_ref,
        outcome_revision(&replay),
    );
    let outcome = harness.execute(&release).await;
    let released = reservation_of(&outcome);
    assert!(released.released);
    let quantity = &released.quantities[0];
    assert_eq!(
        (
            quantity.reserved,
            quantity.consumed,
            quantity.returned,
            quantity.remaining
        ),
        (5, 0, 5, 0)
    );
    assert_eq!(
        quantity.consumed + quantity.returned + quantity.remaining,
        quantity.reserved
    );
    let status = harness.execute(&status_request(&reservation_ref)).await;
    assert!(reservation_of(&status).released);
    // Replaying the released operation returns its stored result without a
    // second remainder, and a fresh release attempt fails closed.
    let replay_release = harness.execute(&release).await;
    assert_eq!(replay_release, outcome);
    let again = harness
        .execute(&release_request(
            "release-operation-2",
            &reservation_ref,
            outcome_revision(&replay),
        ))
        .await;
    assert_eq!(again.failure(), Some(ScriptOperationFailure::StaleRevision));
}

#[tokio::test]
async fn owned_reservation_rejects_more_than_the_endpoint_holds() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let apples = harness.stack("minecraft:apple", 2);
    harness.state.lock().unwrap().inventory.slots[9] = apples;
    let player = harness.endpoint();
    let name = harness
        .items
        .name_of(harness.stack("minecraft:apple", 1).item_id)
        .unwrap()
        .as_str()
        .to_owned();
    let plan = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        4,
        vec![ScriptInventoryMaterial::new(name, 3)],
    )]);
    let request = reserve_request(player, plan, harness.fence());
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InsufficientItems)
    );
}

fn outcome_revision(outcome: &mc_script::ScriptOperationOutcome) -> u64 {
    outcome
        .revision()
        .expect("committed outcome carries a revision")
}

/// Resulting fence revision of a committed transfer, i.e. the durable world
/// watermark the player inventory now carries.
fn result_revision(outcome: &mc_script::ScriptOperationOutcome) -> u64 {
    transfer_fences(outcome)
        .first()
        .expect("transfer outcome carries its resulting fences")
        .fence
        .revision
}

#[tokio::test]
async fn owned_transfer_reports_typed_capacity_and_insufficient_failures() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let sword = harness.stack("minecraft:iron_sword", 1);
    harness.state.lock().unwrap().inventory.slots[9] = sword.clone();
    harness.state.lock().unwrap().inventory.slots[10] = sword.clone();
    let player = harness.endpoint();
    let fence = harness.fence();

    // A full destination is a capacity failure, not a partial move.
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            player.clone(),
            10,
            1,
        )],
        vec![(player.clone(), fence.clone())],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Capacity));
    {
        let state = harness.state.lock().unwrap();
        assert_eq!(state.inventory.slots[9], sword);
        assert_eq!(state.inventory.slots[10], sword);
    }

    // More than the source holds is an insufficient-items failure.
    let request = transfer_request(
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            player.clone(),
            11,
            2,
        )],
        vec![(player, fence)],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::InsufficientItems)
    );
}

#[test]
fn owned_reservation_blocks_a_drawdown_of_the_reserved_quantity() {
    let items = solaris_required_items();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let player = ScriptInventoryEndpoint::PlayerInventory { player_id: 7 };
    let warehouse = ScriptInventoryEndpoint::Warehouse {
        handle: "completed-container".to_owned(),
    };
    let mut player_slots = vec![ItemStack::EMPTY; 46];
    player_slots[9] = ItemStack::new(apple, 8);
    let mut warehouse_slots = vec![ItemStack::EMPTY; 27];
    warehouse_slots[0] = ItemStack::new(apple, 8);
    let mut planned = BTreeMap::new();
    planned.insert(player, player_slots);
    planned.insert(warehouse.clone(), warehouse_slots);
    let mut reserved = BTreeMap::new();
    reserved.insert(
        warehouse.clone(),
        BTreeMap::from([("minecraft:apple".to_owned(), 6_u64)]),
    );
    assert!(reservation_stock_survives(&planned, &reserved, &items));

    planned
        .get_mut(&warehouse)
        .unwrap()
        .iter_mut()
        .for_each(|stack| *stack = ItemStack::EMPTY);
    assert!(!reservation_stock_survives(&planned, &reserved, &items));
}

#[tokio::test]
async fn owned_transfer_recovers_exactly_once_across_a_process_crash() {
    const CHILD_WORLD: &str = "SOLARIS_C1_OWNED_INVENTORY_WORLD";
    const BOUNDARY: &str = "SOLARIS_C1_OWNED_INVENTORY_BOUNDARY";
    const COMMITTED: &str = "C1_OWNED_INVENTORY_DECISION_SYNCED";
    let items = Arc::new(solaris_required_items());
    let facts = Arc::new(solaris_required_item_facts());
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let uuid = crate::login::offline_uuid("OwnedInventoryCrash");
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    if let Some(root) = std::env::var_os(CHILD_WORLD) {
        let root = Path::new(&root);
        let mut saved = PlayerPersistedState::new_default(pose);
        saved.inventory.slots[9] = ItemStack::new(apple, 3);
        save_player_state(root, uuid, &items, &saved).unwrap();
        let mut storage = PluginStorage::open(root).unwrap();
        let sessions = Arc::new(SessionRegistry::new());
        let runtime = InventoryRuntime::player_only_for_test(
            root,
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let journal = sessions.world_chunk_journal().unwrap();
        journal.reserve_decision_ids(2).unwrap();
        journal
            .record_reserved_snapshot_groups(0, vec![(1, Vec::new()), (2, Vec::new())])
            .unwrap();
        let (outbound, _receiver) = tokio_mpsc::channel(8);
        let (player_id, _) = sessions.register(
            &LoggedInProfile {
                uuid,
                name: "OwnedInventoryCrash".to_owned(),
            },
            (0, 0),
            2,
            HashSet::new(),
            outbound,
            pose,
        );
        sessions.register_player_persistence(player_id, Arc::new(Mutex::new(saved)));
        let player = ScriptInventoryEndpoint::PlayerInventory { player_id };
        let fence = owned_inventory_snapshot(
            player.clone(),
            0,
            &{
                let mut slots = vec![ItemStack::EMPTY; 46];
                slots[9] = ItemStack::new(apple, 3);
                slots
            },
            &items,
        )
        .unwrap()
        .fence;
        let request = transfer_request(
            player_id,
            vec![ScriptOwnedItemTransfer::new(
                player.clone(),
                9,
                player.clone(),
                10,
                1,
            )],
            vec![(player, fence)],
        );
        let boundary = std::env::var(BOUNDARY).unwrap();
        match boundary.as_str() {
            "world_synced" => storage.inject_fault_for_test(StorageFaultPoint::Write),
            "storage_appended" => storage.inject_fault_for_test(StorageFaultPoint::Sync),
            "fully_projected" => {}
            _ => panic!("unknown crash boundary"),
        }
        let result = runtime
            .execute_owned_inventory(&mut storage, OWNER, &request)
            .await;
        if boundary == "fully_projected" {
            assert_eq!(result.unwrap().state(), ScriptOperationState::Committed);
        } else {
            assert!(matches!(
                result,
                Err(PluginStorageMutationError::DurabilityUnknown(_))
            ));
        }
        println!("{COMMITTED}");
        std::io::stdout().flush().unwrap();
        let mut release = [0];
        std::io::stdin().read_exact(&mut release).unwrap();
        panic!("parent must terminate at the selected durable boundary");
    }

    for boundary in ["world_synced", "storage_appended", "fully_projected"] {
        let world = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "play::session::owned_inventory_endpoint_tests::owned_transfer_recovers_exactly_once_across_a_process_crash",
                "--nocapture",
            ])
            .env(CHILD_WORLD, world.path())
            .env(BOUNDARY, boundary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).unwrap() == 0 {
                let result = child.wait_with_output().unwrap();
                panic!("crash child exited before {boundary}: {result:?}");
            }
            if line.trim() == COMMITTED {
                break;
            }
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());

        let sessions = Arc::new(SessionRegistry::new());
        let runtime = InventoryRuntime::player_only_for_test(
            world.path(),
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let mut storage = PluginStorage::open(world.path()).unwrap();
        runtime.recover(&mut storage).unwrap();
        let recovered = load_player_state(
            world.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(pose),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            recovered.inventory.slots[9],
            ItemStack::new(apple, 2),
            "{boundary}: the moved item left the source exactly once"
        );
        assert_eq!(
            recovered.inventory.slots[10],
            ItemStack::new(apple, 1),
            "{boundary}: the moved item arrived exactly once"
        );
        assert_eq!(recovered.inventory_operation_revision, 3);
        let recovered_player_id = 1;

        // Retry the exact canonical pre-crash operation: the durable receipt
        // must replay it without touching the world again.
        let player = ScriptInventoryEndpoint::PlayerInventory {
            player_id: recovered_player_id,
        };
        let mut pre_crash = vec![ItemStack::EMPTY; 46];
        pre_crash[9] = ItemStack::new(apple, 3);
        let request = transfer_request(
            recovered_player_id,
            vec![ScriptOwnedItemTransfer::new(
                player.clone(),
                9,
                player.clone(),
                10,
                1,
            )],
            vec![(
                player,
                owned_inventory_snapshot(
                    ScriptInventoryEndpoint::PlayerInventory {
                        player_id: recovered_player_id,
                    },
                    0,
                    &pre_crash,
                    &items,
                )
                .unwrap()
                .fence,
            )],
        );
        let replay = runtime
            .execute_owned_inventory(&mut storage, OWNER, &request)
            .await
            .unwrap();
        assert_eq!(
            replay.state(),
            ScriptOperationState::Committed,
            "{boundary}: the durable receipt replays as committed"
        );
        let after = load_player_state(
            world.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(pose),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            after.inventory.slots[9],
            ItemStack::new(apple, 2),
            "{boundary}: replay must not move the item again"
        );
        assert_eq!(
            after.inventory.slots[10],
            ItemStack::new(apple, 1),
            "{boundary}"
        );
    }
}

/// One resident endpoint transfer preserves item components and durability in
/// both directions, exactly the canonical stack the player endpoint carries.
#[tokio::test]
async fn resident_transfer_preserves_components_and_durability() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let sword = {
        let items = solaris_required_items();
        ItemStack::new(
            items
                .id_of(&Identifier::parse("minecraft:iron_sword").unwrap())
                .unwrap(),
            1,
        )
        .with_damage(17)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 2)
        .with_custom_name("Settler's blade")
        .with_item_model(Identifier::parse("minecraft:diamond_sword").unwrap())
    };
    harness.state.lock().unwrap().inventory.slots[9] = sword.clone();
    let handle = harness.materialize(OWNER, 1).await;
    let equipment = resident_endpoint(&handle, true);
    let resident_fence = harness.query(equipment.clone()).await.fence;
    let player = harness.endpoint();
    let request = transfer_request_id(
        "resident-transfer-1",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            equipment.clone(),
            0,
            1,
        )],
        vec![
            (player.clone(), harness.fence()),
            (equipment.clone(), resident_fence),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.state(),
        ScriptOperationState::Committed,
        "{outcome:?}"
    );
    assert_eq!(
        harness.state.lock().unwrap().inventory.slots[9],
        ItemStack::EMPTY
    );

    let snapshot = harness.query(equipment.clone()).await;
    let item = snapshot.slots[0].item.as_ref().expect("the sword arrived");
    assert_eq!(item.resource_id, "minecraft:iron_sword");
    assert_eq!(item.count, 1);
    assert_eq!(item.damage, Some(17), "durability survives");
    assert_eq!(item.custom_name.as_deref(), Some("Settler's blade"));
    assert_eq!(item.item_model.as_deref(), Some("minecraft:diamond_sword"));
    assert!(
        item.enchantments
            .iter()
            .any(
                |enchantment| enchantment.resource_id == "minecraft:sharpness"
                    && enchantment.level == 2
            ),
        "enchantments survive: {item:?}"
    );

    // A resident fence from the transfer round-trips as the next expected
    // revision, and taking the sword back restores the components.
    let resident_fence = snapshot.fence;
    let request = transfer_request_id(
        "resident-transfer-2",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            equipment.clone(),
            0,
            player.clone(),
            9,
            1,
        )],
        vec![
            (player.clone(), harness.fence()),
            (equipment.clone(), resident_fence),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.state(),
        ScriptOperationState::Committed,
        "{outcome:?}"
    );
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], sword);
    let snapshot = harness.query(equipment).await;
    assert!(
        snapshot.slots.iter().all(|slot| slot.item.is_none()),
        "the resident holds nothing after the round trip"
    );
}

/// An absent, foreign or stale resident endpoint transfers nothing.
#[tokio::test]
async fn resident_transfer_rejects_absent_foreign_and_stale_endpoints() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let apple = harness.stack("minecraft:apple", 4);
    harness.state.lock().unwrap().inventory.slots[9] = apple.clone();
    let player = harness.endpoint();
    let player_fence = harness.fence();

    // An absent resident is not found.
    let absent = resident_endpoint("missing-resident", true);
    let request = transfer_request_id(
        "resident-transfer-3",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            absent.clone(),
            0,
            player.clone(),
            10,
            1,
        )],
        vec![
            (player.clone(), player_fence.clone()),
            (
                absent,
                ScriptInventoryFence::try_new(0, "0".repeat(64)).unwrap(),
            ),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::NotFound));
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);

    // A resident owned by another plugin is forbidden.
    let foreign_handle = harness.materialize("other-plugin", 2).await;
    let foreign = resident_endpoint(&foreign_handle, true);
    let request = transfer_request_id(
        "resident-transfer-4",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            foreign.clone(),
            0,
            player.clone(),
            10,
            1,
        )],
        vec![
            (player.clone(), player_fence.clone()),
            (
                foreign,
                ScriptInventoryFence::try_new(0, "0".repeat(64)).unwrap(),
            ),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(outcome.failure(), Some(ScriptOperationFailure::Forbidden));
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);

    // A stale resident fence transfers nothing.
    let handle = harness.materialize(OWNER, 1).await;
    let equipment = resident_endpoint(&handle, true);
    let fence = harness.query(equipment.clone()).await.fence;
    let stale =
        ScriptInventoryFence::try_new(fence.revision + 1, fence.snapshot_hash.clone()).unwrap();
    let request = transfer_request_id(
        "resident-transfer-5",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            equipment.clone(),
            0,
            1,
        )],
        vec![(player.clone(), player_fence), (equipment.clone(), stale)],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], apple);
    let snapshot = harness.query(equipment).await;
    assert!(snapshot.slots.iter().all(|slot| slot.item.is_none()));
}

/// A transfer against gear an active order holds leaves the item in exactly one
/// place, and a transfer holding the pre-transfer fence is refused.
#[tokio::test]
async fn resident_transfer_never_duplicates_equipment_held_by_an_active_order() {
    let mut harness = Harness::new(PlayerInventory::empty());
    let sword = harness.stack("minecraft:iron_sword", 1);
    harness.state.lock().unwrap().inventory.slots[9] = sword.clone();
    let handle = harness.materialize(OWNER, 1).await;
    let equipment = resident_endpoint(&handle, true);
    let player = harness.endpoint();

    // Move the sword into the resident's hands.
    let resident_fence = harness.query(equipment.clone()).await.fence;
    let request = transfer_request_id(
        "resident-transfer-6",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            equipment.clone(),
            0,
            1,
        )],
        vec![
            (player.clone(), harness.fence()),
            (equipment.clone(), resident_fence),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.state(),
        ScriptOperationState::Committed,
        "{outcome:?}"
    );

    // The resident is now on an active order (a hold it keeps while the gear
    // changes). The first order is fenced on revision 0: no order yet.
    let outcome = harness
        .execute_order(&order_request(
            "hold-1",
            &handle,
            0,
            ScriptResidentOrder::Hold {
                anchor: ScriptBlockPosition::new(0, 64, 0),
                heading_degrees: 0,
                formation: ScriptFormation::new(ScriptFormationKind::Line, 1),
                engagement_radius: 8,
            },
        ))
        .await;
    assert_eq!(outcome.failure(), None, "{outcome:?}");

    // Take the sword back out while the order is active.
    let held_fence = harness.query(equipment.clone()).await.fence;
    let request = transfer_request_id(
        "resident-transfer-7",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            equipment.clone(),
            0,
            player.clone(),
            9,
            1,
        )],
        vec![
            (player.clone(), harness.fence()),
            (equipment.clone(), held_fence.clone()),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.state(),
        ScriptOperationState::Committed,
        "{outcome:?}"
    );
    assert_eq!(
        harness.state.lock().unwrap().inventory.slots[9],
        sword,
        "the player holds the sword exactly once"
    );
    let snapshot = harness.query(equipment.clone()).await;
    assert!(
        snapshot.slots.iter().all(|slot| slot.item.is_none()),
        "the resident no longer holds a phantom copy"
    );

    // A transfer holding the pre-transfer fence is refused, so a concurrent
    // caller cannot mint a second copy of the same item.
    let request = transfer_request_id(
        "resident-transfer-8",
        harness.player_id,
        vec![ScriptOwnedItemTransfer::new(
            equipment.clone(),
            0,
            player.clone(),
            10,
            1,
        )],
        vec![
            (player.clone(), harness.fence()),
            (equipment.clone(), held_fence),
        ],
    );
    let outcome = harness.execute(&request).await;
    assert_eq!(
        outcome.failure(),
        Some(ScriptOperationFailure::StaleRevision)
    );
    assert_eq!(harness.state.lock().unwrap().inventory.slots[9], sword);
    assert_eq!(
        harness.state.lock().unwrap().inventory.slots[10],
        ItemStack::EMPTY
    );
}
