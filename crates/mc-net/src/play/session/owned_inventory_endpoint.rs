use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    ScriptInventoryEndpoint, ScriptInventoryExpectedRevision, ScriptInventoryFence,
    ScriptInventoryReservationQuantity, ScriptInventoryReservationSnapshot,
    ScriptInventoryResourcePlan, ScriptOperationFailure, ScriptOperationOutcome,
    ScriptOperationPayload, ScriptOperationRequest, ScriptOwnedInventoryResult,
    ScriptOwnedItemTransfer,
};

use crate::play::inventory::PlayerInventory;
use crate::play::owned_inventory::{
    OwnedInventoryCommit, OwnedInventoryPrepare, ResidentEndpointState, ResidentGearUpdate,
    endpoint_window, gear_slots_to_items, inventory_resource_stock, items_to_gear_slots,
    owned_inventory_snapshot, plan_owned_item_transfers, resource_plan_hash, resource_plan_totals,
    transfer_endpoints,
};
use crate::play::persistence::inventory_recovery::PlayerInventoryRecovery;
use crate::play::script_inventory_transaction::{
    ScriptStorageCommitError, ScriptStoragePrepareOutcome,
};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

impl SessionRegistry {
    /// Canonical owned-inventory snapshot for one player endpoint, or the typed
    /// failure that keeps the plugin from guessing at an unavailable owner.
    pub(crate) fn query_owned_inventory(
        &self,
        endpoint: &ScriptInventoryEndpoint,
        expected_revision: Option<u64>,
        items: &ItemRegistry,
    ) -> ScriptOperationOutcome {
        let ScriptInventoryEndpoint::PlayerInventory { player_id } = endpoint else {
            // A resident endpoint is resolved against the resident ledger, and a
            // warehouse endpoint against its durable binding and the loaded
            // container, before either reaches this player-only reader; only an
            // endpoint with no canonical session inventory is refused here.
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::Unloaded);
        };
        let Some((inventory, revision)) = self.owned_inventory_player_state(*player_id) else {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound);
        };
        if expected_revision.is_some_and(|expected| expected != revision) {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision);
        }
        let snapshot = match owned_inventory_snapshot(endpoint.clone(), revision, &inventory, items)
        {
            Ok(snapshot) => snapshot,
            Err(failure) => return ScriptOperationOutcome::rejected(failure),
        };
        committed(
            revision,
            ScriptOwnedInventoryResult::Snapshot {
                inventory: snapshot,
            },
        )
    }

    /// Canonical snapshot of one bound warehouse container.
    ///
    /// The caller has already resolved the opaque handle to its durable
    /// binding, verified ownership and liveness, and read the loaded container;
    /// this builds the same canonical snapshot every other endpoint uses, so a
    /// warehouse read is fenced by the binding revision and hashed exactly like
    /// a player or resident one.
    pub(crate) fn query_warehouse_inventory(
        &self,
        endpoint: &ScriptInventoryEndpoint,
        revision: u64,
        inventory: &[ItemStack],
        items: &ItemRegistry,
    ) -> ScriptOperationOutcome {
        let snapshot = match owned_inventory_snapshot(endpoint.clone(), revision, inventory, items)
        {
            Ok(snapshot) => snapshot,
            Err(failure) => return ScriptOperationOutcome::rejected(failure),
        };
        committed(
            revision,
            ScriptOwnedInventoryResult::Snapshot {
                inventory: snapshot,
            },
        )
    }

    /// Move items between the actor's own canonical inventories behind one
    /// recoverable decision: the player inventory after-image and the plugin
    /// operation receipt are appended together or not at all.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_owned_inventory_transfer<S>(
        &self,
        plugin_id: &str,
        actor_id: u64,
        transfers: &[ScriptOwnedItemTransfer],
        expected_revisions: &[ScriptInventoryExpectedRevision],
        decision_id: u64,
        reserved: &BTreeMap<ScriptInventoryEndpoint, BTreeMap<String, u64>>,
        request: &ScriptOperationRequest,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        storage: &mut S,
    ) -> Result<OwnedInventoryCommit, S::Error>
    where
        S: OwnedInventoryPrepare,
    {
        let endpoints = transfer_endpoints(transfers);
        for endpoint in &endpoints {
            match endpoint {
                ScriptInventoryEndpoint::PlayerInventory { player_id }
                    if *player_id == actor_id => {}
                ScriptInventoryEndpoint::PlayerInventory { .. } => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(ScriptOperationFailure::Forbidden),
                    ));
                }
                ScriptInventoryEndpoint::Warehouse { .. } => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(ScriptOperationFailure::Unloaded),
                    ));
                }
                // A resident endpoint is resolved against the owner's resident
                // ledger below; a foreign, absent or released resident gets a
                // typed refusal there.
                _ => {}
            }
        }

        let (gate, player_state, recipient, uuid) = {
            let inner = self.lock_inner("prepare owned inventory transfer");
            let Some(session) = inner.sessions.get(&actor_id) else {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            };
            if session.tx.is_closed() {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            }
            let Some(player_state) = inner.player_persistence.get(&actor_id).cloned() else {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            };
            (
                Arc::clone(&session.script_inventory_transaction_gate),
                player_state,
                ordered_session_recipient(actor_id, session),
                session.uuid,
            )
        };

        let Some(_gate) = gate.begin_compound(actor_id) else {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
            ));
        };
        let wait_started = std::time::Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit owned inventory transfer",
            wait_started,
            guard,
        );
        if player_state.inventory_recovery_required {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
            ));
        }

        let current_inventory = player_state.inventory.clone();
        let current_revision = player_state.inventory_operation_revision;
        let mut inventories = BTreeMap::new();
        let mut resident_states: BTreeMap<ScriptInventoryEndpoint, ResidentEndpointState> =
            BTreeMap::new();
        for endpoint in &endpoints {
            let (slots, revision) = match endpoint {
                ScriptInventoryEndpoint::PlayerInventory { .. } => {
                    (current_inventory.slots.to_vec(), current_revision)
                }
                _ => {
                    let state = match storage.resident_endpoint_state(plugin_id, endpoint) {
                        Ok(state) => state,
                        Err(failure) => {
                            return Ok(OwnedInventoryCommit::Rejected(
                                ScriptOperationOutcome::rejected(failure),
                            ));
                        }
                    };
                    let slots = match endpoint {
                        ScriptInventoryEndpoint::ResidentEquipment { .. } => {
                            gear_slots_to_items(&state.equipment, items)
                        }
                        _ => gear_slots_to_items(&state.carry, items),
                    };
                    let slots = match slots {
                        Ok(slots) => slots,
                        Err(failure) => {
                            return Ok(OwnedInventoryCommit::Rejected(
                                ScriptOperationOutcome::rejected(failure),
                            ));
                        }
                    };
                    let revision = state.revision;
                    resident_states.insert(endpoint.clone(), state);
                    (slots, revision)
                }
            };
            let snapshot = match owned_inventory_snapshot(endpoint.clone(), revision, &slots, items)
            {
                Ok(snapshot) => snapshot,
                Err(failure) => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(failure),
                    ));
                }
            };
            if expected_revisions
                .iter()
                .any(|expected| expected.endpoint == *endpoint && expected.fence != snapshot.fence)
            {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision),
                ));
            }
            inventories.insert(endpoint.clone(), slots);
        }

        let planned = match plan_owned_item_transfers(transfers, &inventories, items, item_facts) {
            Ok(planned) => planned,
            Err(failure) => {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(failure),
                ));
            }
        };
        if !reservation_stock_survives(&planned, reserved, items) {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::InsufficientItems),
            ));
        }

        // A resident endpoint fence is its durable record revision, the
        // transaction this batch commits with; a player endpoint keeps the
        // journal decision identity it already round-trips.
        let resident_revision = storage.gear_revision();
        let mut result_fences = Vec::with_capacity(endpoints.len());
        for endpoint in &endpoints {
            let slots = planned.get(endpoint).expect("planned endpoint inventory");
            let revision = if endpoint.resident_handle().is_some() {
                resident_revision
            } else {
                decision_id
            };
            let snapshot = match owned_inventory_snapshot(endpoint.clone(), revision, slots, items)
            {
                Ok(snapshot) => snapshot,
                Err(failure) => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(failure),
                    ));
                }
            };
            result_fences.push(ScriptInventoryExpectedRevision::new(
                endpoint.clone(),
                snapshot.fence,
            ));
        }
        result_fences.sort_unstable_by(|left, right| left.endpoint.cmp(&right.endpoint));

        // The player after-image, when a player endpoint participates.
        let player_endpoint = endpoints
            .iter()
            .find(|endpoint| matches!(endpoint, ScriptInventoryEndpoint::PlayerInventory { .. }));
        let planned_player_inventory = match player_endpoint {
            Some(endpoint) => {
                let slots = planned.get(endpoint).expect("planned player inventory");
                let Ok(slots) = <[ItemStack; 46]>::try_from(slots.clone()) else {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(ScriptOperationFailure::InvalidRequest),
                    ));
                };
                Some(PlayerInventory { slots })
            }
            None => None,
        };

        // The committed resident gear, one update per resident handle so a
        // transfer touching both endpoints of one resident commits both.
        let mut resident_updates: BTreeMap<String, (String, Vec<ItemStack>, Vec<ItemStack>)> =
            BTreeMap::new();
        for (endpoint, state) in &resident_states {
            let Some(handle) = endpoint.resident_handle() else {
                continue;
            };
            let entry = match resident_updates.entry(handle.to_owned()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let equipment = match gear_slots_to_items(&state.equipment, items) {
                        Ok(slots) => slots,
                        Err(failure) => {
                            return Ok(OwnedInventoryCommit::Rejected(
                                ScriptOperationOutcome::rejected(failure),
                            ));
                        }
                    };
                    let carry = match gear_slots_to_items(&state.carry, items) {
                        Ok(slots) => slots,
                        Err(failure) => {
                            return Ok(OwnedInventoryCommit::Rejected(
                                ScriptOperationOutcome::rejected(failure),
                            ));
                        }
                    };
                    entry.insert((state.entity_uuid.clone(), equipment, carry))
                }
            };
            let planned_slots = planned.get(endpoint).expect("planned resident inventory");
            match endpoint {
                ScriptInventoryEndpoint::ResidentEquipment { .. } => {
                    entry.1 = planned_slots.clone()
                }
                _ => entry.2 = planned_slots.clone(),
            }
        }
        let mut updates = Vec::with_capacity(resident_updates.len());
        for (handle, (entity_uuid, equipment, carry)) in resident_updates {
            let equipment = match items_to_gear_slots(&equipment, items) {
                Ok(slots) => slots,
                Err(failure) => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(failure),
                    ));
                }
            };
            let carry = match items_to_gear_slots(&carry, items) {
                Ok(slots) => slots,
                Err(failure) => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(failure),
                    ));
                }
            };
            updates.push(ResidentGearUpdate {
                handle,
                entity_uuid,
                equipment,
                carry,
            });
        }

        let payload = ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Transfer {
                inventories: result_fences,
            }),
        };
        let recovery = match &planned_player_inventory {
            Some(inventory) => Some(
                PlayerInventoryRecovery::capture(uuid, &player_state, inventory, items)
                    .map_err(std::io::Error::other)?,
            ),
            None => None,
        };
        let prepared = if updates.is_empty() {
            storage.prepare_owned(plugin_id, request, payload, recovery)?
        } else {
            storage.prepare_resident_gear(plugin_id, request, payload, updates, recovery)?
        };
        let prepared = match prepared {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
                ));
            }
        };
        let decision = match storage.commit_owned(prepared) {
            Ok(decision) => decision,
            Err(ScriptStorageCommitError::NotCommitted(error)) => return Err(error),
            Err(ScriptStorageCommitError::DurabilityUnknown(error)) => {
                player_state.inventory_recovery_required = true;
                return Err(error);
            }
        };
        if let Some(planned_inventory) = planned_player_inventory {
            player_state.replace_inventory(planned_inventory.clone());
            player_state.inventory_operation_revision = decision;
            let carried_item = player_state.carried_item.clone();
            drop(player_state);
            dispatch_visibility_command(
                &recipient,
                OutboundCommand::AuthoritativeInventory {
                    inventory: Box::new(planned_inventory),
                    carried_item,
                },
            );
        }
        Ok(OwnedInventoryCommit::Committed)
    }

    /// Reserve canonical stacks of one player endpoint. The reservation is
    /// durable and blocks every later transfer that would consume below it.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_owned_inventory_reservation<S>(
        &self,
        plugin_id: &str,
        endpoint: &ScriptInventoryEndpoint,
        resource_plan: &ScriptInventoryResourcePlan,
        expected_revision: &ScriptInventoryFence,
        reservation_ref: &str,
        reserved_existing: &BTreeMap<String, u64>,
        request: &ScriptOperationRequest,
        items: &ItemRegistry,
        storage: &mut S,
    ) -> Result<OwnedInventoryCommit, S::Error>
    where
        S: OwnedInventoryPrepare,
    {
        let ScriptInventoryEndpoint::PlayerInventory { player_id } = endpoint else {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::Unloaded),
            ));
        };
        let player_id = *player_id;
        let (gate, player_state) = {
            let inner = self.lock_inner("prepare owned inventory reservation");
            let Some(session) = inner.sessions.get(&player_id) else {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            };
            if session.tx.is_closed() {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            }
            let Some(player_state) = inner.player_persistence.get(&player_id).cloned() else {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound),
                ));
            };
            (
                Arc::clone(&session.script_inventory_transaction_gate),
                player_state,
            )
        };
        let Some(_gate) = gate.begin_compound(player_id) else {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
            ));
        };
        let wait_started = std::time::Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit owned inventory reservation",
            wait_started,
            guard,
        );
        if player_state.inventory_recovery_required {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
            ));
        }
        let revision = player_state.inventory_operation_revision;
        let snapshot = match owned_inventory_snapshot(
            endpoint.clone(),
            revision,
            &player_state.inventory.slots,
            items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(failure),
                ));
            }
        };
        if &snapshot.fence != expected_revision {
            return Ok(OwnedInventoryCommit::Rejected(
                ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision),
            ));
        }
        let totals = match resource_plan_totals(resource_plan) {
            Ok(totals) => totals,
            Err(failure) => {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(failure),
                ));
            }
        };
        for (resource_id, quantity) in &totals {
            let stock = match inventory_resource_stock(
                endpoint_window(endpoint, &player_state.inventory.slots),
                items,
                resource_id,
            ) {
                Ok(stock) => stock,
                Err(failure) => {
                    return Ok(OwnedInventoryCommit::Rejected(
                        ScriptOperationOutcome::rejected(failure),
                    ));
                }
            };
            let already = reserved_existing.get(resource_id).copied().unwrap_or(0);
            if stock < already.saturating_add(*quantity) {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::InsufficientItems),
                ));
            }
        }
        let quantities = totals
            .into_iter()
            .map(|(resource_id, quantity)| {
                ScriptInventoryReservationQuantity::new(resource_id, quantity, 0, 0, quantity)
            })
            .collect();
        let reservation = ScriptInventoryReservationSnapshot::new(
            reservation_ref.to_owned(),
            endpoint.clone(),
            resource_plan_hash(resource_plan),
            quantities,
            None,
            false,
            0,
        );
        let payload = ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Reservation { reservation }),
        };
        let prepared = match storage.prepare_owned(plugin_id, request, payload, None)? {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(OwnedInventoryCommit::Rejected(
                    ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
                ));
            }
        };
        match storage.commit_owned(prepared) {
            Ok(_) => Ok(OwnedInventoryCommit::Committed),
            Err(ScriptStorageCommitError::NotCommitted(error)) => Err(error),
            Err(ScriptStorageCommitError::DurabilityUnknown(error)) => Err(error),
        }
    }

    fn owned_inventory_player_state(&self, player_id: u64) -> Option<(Vec<ItemStack>, u64)> {
        let state = {
            let inner = self.lock_inner("read owned inventory player state");
            let session = inner.sessions.get(&player_id)?;
            if session.tx.is_closed() {
                return None;
            }
            inner.player_persistence.get(&player_id).cloned()
        }?;
        let guard = crate::lock_policy::lock_authoritative_mutex(&state, "play.player_persistence");
        let state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "read owned inventory player state",
            std::time::Instant::now(),
            guard,
        );
        Some((
            state.inventory.slots.to_vec(),
            state.inventory_operation_revision,
        ))
    }
}

fn committed(revision: u64, result: ScriptOwnedInventoryResult) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::OwnedInventory {
            result: Box::new(result),
        },
    )
    .expect("core-owned inventory outcome is canonical")
}

/// The reservation keeps its claim on the planned stock: a transfer may never
/// drop an endpoint below the quantities other operations still hold.
pub(super) fn reservation_stock_survives(
    planned: &BTreeMap<ScriptInventoryEndpoint, Vec<ItemStack>>,
    reserved: &BTreeMap<ScriptInventoryEndpoint, BTreeMap<String, u64>>,
    items: &ItemRegistry,
) -> bool {
    reserved.iter().all(|(endpoint, quantities)| {
        let Some(slots) = planned.get(endpoint) else {
            return true;
        };
        let window = endpoint_window(endpoint, slots);
        quantities.iter().all(|(resource_id, quantity)| {
            inventory_resource_stock(window, items, resource_id)
                .is_ok_and(|stock| stock >= *quantity)
        })
    })
}
