//! Server-owned warehouse/resident transfers in either direction. The container
//! gear record and operation receipt share one recoverable world decision.

use std::collections::BTreeMap;

use mc_script::{
    ScriptInventoryEndpoint, ScriptInventoryExpectedRevision, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOwnedInventoryResult, ScriptOwnedItemTransfer,
};

use super::{InventoryRuntime, PreparedDepositCommit, PreparedDepositContainer};
use crate::play::owned_inventory::{
    gear_slots_to_items, items_to_gear_slots, owned_inventory_snapshot, plan_owned_item_transfers,
    reservation_stock_survives, transfer_endpoints,
};
use crate::script::storage::{
    PluginStorage, PluginStorageMutationError, ScriptStoragePrepareOutcome,
};

impl InventoryRuntime {
    /// No player inventory or session participates. A resident's existing work,
    /// assignment and UUID survive the gear update in the same journal frame.
    pub(super) async fn commit_warehouse_resident_transfer(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        actor_id: u64,
        transfers: &[ScriptOwnedItemTransfer],
        expected_revisions: &[ScriptInventoryExpectedRevision],
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let reject = |failure| Ok(ScriptOperationOutcome::rejected(failure));
        if actor_id != 0 || self.world.is_none() {
            return reject(ScriptOperationFailure::InvalidRequest);
        }
        let Some(operation_id) = request.operation_id() else {
            return reject(ScriptOperationFailure::InvalidRequest);
        };
        let _save_guard = self.save_coordinator.lock().await;
        let endpoints = transfer_endpoints(transfers);
        let mut warehouse_handle = None;
        let mut resident_handle = None;
        for transfer in transfers {
            let (handle, resident) = match (&transfer.source, &transfer.destination) {
                (ScriptInventoryEndpoint::Warehouse { handle }, destination) => {
                    let Some(resident) = destination.resident_handle() else {
                        return reject(ScriptOperationFailure::InvalidRequest);
                    };
                    (handle, resident)
                }
                (source, ScriptInventoryEndpoint::Warehouse { handle }) => {
                    let Some(resident) = source.resident_handle() else {
                        return reject(ScriptOperationFailure::InvalidRequest);
                    };
                    (handle, resident)
                }
                _ => return reject(ScriptOperationFailure::InvalidRequest),
            };
            if warehouse_handle
                .as_ref()
                .is_some_and(|prior| *prior != handle)
                || resident_handle
                    .as_ref()
                    .is_some_and(|prior| *prior != resident)
            {
                return reject(ScriptOperationFailure::InvalidRequest);
            }
            warehouse_handle = Some(handle);
            resident_handle = Some(resident);
        }
        let (Some(warehouse_handle), Some(resident_handle)) = (warehouse_handle, resident_handle)
        else {
            return reject(ScriptOperationFailure::InvalidRequest);
        };
        let warehouse_endpoint = ScriptInventoryEndpoint::Warehouse {
            handle: warehouse_handle.clone(),
        };
        let container = match self.resolve_warehouse_container(storage, plugin_id, warehouse_handle)
        {
            Ok(container) => container,
            Err(failure) => return reject(failure),
        };
        let Some(resident) = storage.residents().record(resident_handle) else {
            return reject(ScriptOperationFailure::NotFound);
        };
        if resident.plugin_id != plugin_id {
            return reject(ScriptOperationFailure::Forbidden);
        }
        if resident.disposition != super::super::residents::ResidentDisposition::Alive {
            return reject(ScriptOperationFailure::NotFound);
        }
        let entity_uuid = resident.entity_uuid.clone();
        // The ledger observes deaths lazily. Admission must consult the entity
        // owner again: an earlier guest query may precede a death or conversion.
        let Ok(uuid) = uuid::Uuid::parse_str(&entity_uuid) else {
            return reject(ScriptOperationFailure::NotFound);
        };
        let live = self.sessions().resident_entity_snapshots(&[uuid]).await;
        let snapshot = match live.as_slice() {
            [Some(snapshot)] => snapshot,
            [None] => return reject(ScriptOperationFailure::Unloaded),
            _ => return reject(ScriptOperationFailure::RuntimeUnavailable),
        };
        if !super::super::residents::resident_entity_is_current(snapshot) {
            storage.observe_resident_death(&resident.clone())?;
            return reject(ScriptOperationFailure::NotFound);
        }
        let (mut equipment, mut carry, resident_revision) =
            match storage.resident_orders().record(resident_handle) {
                Some(record) if record.entity_uuid == entity_uuid => (
                    record
                        .equipment
                        .iter()
                        .map(|item| item.as_ref().map(super::gear_from_stack))
                        .collect(),
                    record
                        .carry
                        .iter()
                        .map(|item| item.as_ref().map(super::gear_from_stack))
                        .collect(),
                    record.revision,
                ),
                _ => (
                    vec![None; usize::from(mc_script::MAX_RESIDENT_EQUIPMENT_SLOTS)],
                    vec![None; usize::from(mc_script::MAX_RESIDENT_CARRY_SLOTS)],
                    0,
                ),
            };
        let equipment_endpoint = ScriptInventoryEndpoint::ResidentEquipment {
            handle: resident_handle.to_owned(),
        };
        let carry_endpoint = ScriptInventoryEndpoint::ResidentCarry {
            handle: resident_handle.to_owned(),
        };
        let equipment_items = match gear_slots_to_items(&equipment, &self.items) {
            Ok(items) => items,
            Err(failure) => return reject(failure),
        };
        let carry_items = match gear_slots_to_items(&carry, &self.items) {
            Ok(items) => items,
            Err(failure) => return reject(failure),
        };
        let mut inventories = BTreeMap::from([
            (warehouse_endpoint.clone(), container.items.clone()),
            (equipment_endpoint.clone(), equipment_items),
            (carry_endpoint.clone(), carry_items),
        ]);
        for endpoint in &endpoints {
            let revision = if *endpoint == warehouse_endpoint {
                container.revision
            } else {
                resident_revision
            };
            let snapshot = match owned_inventory_snapshot(
                endpoint.clone(),
                revision,
                &inventories[endpoint],
                &self.items,
            ) {
                Ok(snapshot) => snapshot,
                Err(failure) => return reject(failure),
            };
            if expected_revisions
                .iter()
                .any(|expected| expected.endpoint == *endpoint && expected.fence != snapshot.fence)
            {
                return reject(ScriptOperationFailure::StaleRevision);
            }
        }
        // The planner only sees the exact endpoints the request named; the
        // untouched resident endpoint is retained separately below.
        inventories.retain(|endpoint, _| endpoints.contains(endpoint));
        let planned =
            match plan_owned_item_transfers(transfers, &inventories, &self.items, &self.item_facts)
            {
                Ok(planned) => planned,
                Err(failure) => return reject(failure),
            };
        let reserved: BTreeMap<_, _> = endpoints
            .iter()
            .map(|endpoint| (endpoint.clone(), storage.reserved_quantities(endpoint)))
            .collect();
        if !reservation_stock_survives(&planned, &reserved, &self.items) {
            return reject(ScriptOperationFailure::InsufficientItems);
        }
        let planned_container = planned[&warehouse_endpoint].clone();
        if let Some(slots) = planned.get(&equipment_endpoint) {
            equipment = match items_to_gear_slots(slots, &self.items) {
                Ok(slots) => slots,
                Err(failure) => return reject(failure),
            };
        }
        if let Some(slots) = planned.get(&carry_endpoint) {
            carry = match items_to_gear_slots(slots, &self.items) {
                Ok(slots) => slots,
                Err(failure) => return reject(failure),
            };
        }
        let revision = storage.revision.saturating_add(1);
        let mut fences = Vec::with_capacity(endpoints.len());
        for endpoint in &endpoints {
            let slots = &planned[endpoint];
            let snapshot = match owned_inventory_snapshot(
                endpoint.clone(),
                if *endpoint == warehouse_endpoint {
                    container.revision
                } else {
                    revision
                },
                slots,
                &self.items,
            ) {
                Ok(snapshot) => snapshot,
                Err(failure) => return reject(failure),
            };
            fences.push(ScriptInventoryExpectedRevision::new(
                endpoint.clone(),
                snapshot.fence,
            ));
        }
        let payload = ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Transfer {
                inventories: fences,
            }),
        };
        let mut record = storage
            .resident_orders()
            .record(resident_handle)
            .cloned()
            .filter(|record| record.entity_uuid == entity_uuid)
            .unwrap_or_else(|| {
                super::DurableResidentOrderRecord::empty(
                    resident_handle.to_owned(),
                    plugin_id.to_owned(),
                    entity_uuid,
                    super::DurableAssignment::Civilian,
                )
            });
        record.equipment = equipment
            .iter()
            .map(|item| item.as_ref().map(super::stack_from_gear))
            .collect();
        record.carry = carry
            .iter()
            .map(|item| item.as_ref().map(super::stack_from_gear))
            .collect();
        let prepared = match storage.prepare_resident_gear_batch(
            plugin_id,
            request,
            payload,
            vec![super::DurableResidentOrderChange::Record {
                record: Box::new(record),
            }],
            None,
        )? {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => return reject(ScriptOperationFailure::Busy),
        };
        let decision = self
            .commit_prepared_deposit(
                storage,
                prepared,
                PreparedDepositContainer {
                    position: container.position,
                    expected: container.items,
                    updated: planned_container,
                },
                None,
                None,
            )
            .await?;
        match decision {
            PreparedDepositCommit::Committed(_) => Ok(storage
                .operation_receipt(plugin_id, operation_id)
                .expect("committed warehouse resident receipt remains installed")
                .outcome
                .clone()),
            PreparedDepositCommit::Refused(failure) => reject(failure),
        }
    }
}
