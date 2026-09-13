use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    MAX_RESIDENT_CARRY_SLOTS, MAX_RESIDENT_EQUIPMENT_SLOTS, ScriptInventoryEndpoint,
    ScriptInventoryStorageTransaction, ScriptOperation, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOwnedInventoryOperation, ScriptStorageMutation,
};

use crate::play::SessionRegistry;
use crate::play::owned_inventory::{
    OwnedInventoryCommit, OwnedInventoryPrepare, ResidentEndpointState, ResidentGearStack,
    ResidentGearUpdate, owned_inventory_fingerprint,
};
use crate::play::persistence::inventory_recovery::PlayerInventoryRecovery;
use crate::play::world_journal::{WorldChunkJournal, WorldChunkJournalError};
use crate::play::{
    ScriptStorageCommitError, ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
use crate::server::ShutdownHandle;

use super::resident_orders::{
    DurableAssignment, DurableResidentOrderChange, DurableResidentOrderRecord, DurableResidentStack,
};
use super::{
    MAX_TRANSACTION_FRAME_BYTES, OP_STORAGE_BATCH, PluginStorage, PluginStorageMutationError,
    PluginStorageStartError, PreparedStorageBatch, decode_storage_batch, encode_storage_batch,
    frame,
};

impl PreparedStorageBatch {
    pub(crate) const MAX_ENCODED_BYTES: usize = MAX_TRANSACTION_FRAME_BYTES;

    pub(crate) fn encode_world_inventory(&self) -> Result<Vec<u8>, PluginStorageMutationError> {
        self.validate_inventory_participant()
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        encode_storage_batch(self)
    }

    pub(crate) fn decode_world_inventory(payload: &[u8]) -> Result<Self, PluginStorageStartError> {
        let Some((&OP_STORAGE_BATCH, body)) = payload.split_first() else {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision batch",
            ));
        };
        if payload.len() > MAX_TRANSACTION_FRAME_BYTES {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision size",
            ));
        }
        let batch = decode_storage_batch(body)?;
        batch.validate_inventory_participant()?;
        Ok(batch)
    }

    fn validate_inventory_participant(&self) -> Result<(), PluginStorageStartError> {
        if self.inventory.is_none()
            && !self.operation.as_ref().is_some_and(|receipt| {
                matches!(
                    receipt.outcome.payload(),
                    mc_script::ScriptOperationPayload::OwnedInventory { .. }
                )
            })
        {
            return Err(PluginStorageStartError::Malformed(
                "inventory decision has no inventory participant",
            ));
        }
        Ok(())
    }
}

pub(crate) struct InventoryRuntime {
    world: Option<(PathBuf, WorldChunkJournal)>,
    sessions: Arc<SessionRegistry>,
    items: Arc<ItemRegistry>,
    item_facts: Arc<ItemFactsTable>,
    save_coordinator: Arc<tokio::sync::Mutex<()>>,
    settlement: Option<Arc<super::settlement::SettlementRuntime>>,
    settlement_world: Option<Arc<dyn super::settlement::SettlementWorld>>,
    resident_world: Option<Arc<dyn crate::play::resident_work::ResidentWorld>>,
}

impl InventoryRuntime {
    /// Shared session registry used by durable non-inventory operations.
    pub(super) fn sessions(&self) -> &Arc<SessionRegistry> {
        &self.sessions
    }

    /// Canonical item registry used to name live resident equipment.
    pub(super) fn items(&self) -> &ItemRegistry {
        &self.items
    }

    /// Canonical item facts used to resolve resident weapon damage and tool
    /// durability.
    pub(super) fn item_facts(&self) -> &ItemFactsTable {
        &self.item_facts
    }

    pub(crate) fn new(
        world_root: Option<&Path>,
        shutdown: &ShutdownHandle,
        sessions: Arc<SessionRegistry>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        let world = world_root
            .zip(sessions.world_chunk_journal())
            .map(|(root, journal)| (root.to_owned(), journal));
        Self {
            world,
            sessions,
            items,
            item_facts,
            save_coordinator: shutdown.save_coordinator(),
            settlement: None,
            settlement_world: None,
            resident_world: None,
        }
    }

    /// Install the deterministic settlement selector and blueprint catalog.
    ///
    /// Settlement operations answer `runtime_unavailable` until this is called;
    /// both installers are called from `bind_internal` when a deployed package
    /// owns the settlement profile.
    pub(crate) fn with_settlement_runtime(
        mut self,
        runtime: Arc<super::settlement::SettlementRuntime>,
    ) -> Self {
        self.settlement = Some(runtime);
        self
    }

    /// Install the authoritative world adapter the settlement operations use for
    /// surveys, footprint observation, and structure application. Without it the
    /// terrain facing operations fail closed.
    pub(crate) fn with_settlement_world(
        mut self,
        world: Arc<dyn super::settlement::SettlementWorld>,
    ) -> Self {
        self.settlement_world = Some(world);
        self
    }

    pub(super) fn settlement_runtime(&self) -> Option<&super::settlement::SettlementRuntime> {
        self.settlement.as_deref()
    }

    pub(super) fn settlement_world(&self) -> Option<&dyn super::settlement::SettlementWorld> {
        self.settlement_world.as_deref()
    }

    /// Install the authoritative world adapter resident work, routes and combat
    /// read. Without it every physical work step fails closed as `unsupported`.
    pub(crate) fn with_resident_world(
        mut self,
        world: Arc<dyn crate::play::resident_work::ResidentWorld>,
    ) -> Self {
        self.resident_world = Some(world);
        self
    }

    pub(super) fn resident_world(&self) -> Option<&dyn crate::play::resident_work::ResidentWorld> {
        self.resident_world.as_deref()
    }

    /// World chunk images have already replayed; no live actor or save is admitted.
    pub(crate) fn recover(
        &self,
        storage: &mut PluginStorage,
    ) -> Result<(), PluginStorageStartError> {
        let Some((root, journal)) = &self.world else {
            return Ok(());
        };
        journal
            .recover_inventory_decisions(|id, mut batch| {
                let player = batch.inventory.take();
                if batch.transaction_id > storage.revision {
                    if storage.revision.checked_add(1) != Some(batch.transaction_id)
                        || !storage.batch_preconditions_match(&batch)
                    {
                        return Err(WorldChunkJournalError::InventoryDecision(
                            "stale inventory storage projection".to_owned(),
                        ));
                    }
                    storage.commit_batch(batch).map_err(|error| {
                        WorldChunkJournalError::InventoryDecision(error.to_string())
                    })?;
                }
                if let Some(player) = player {
                    player.recover(root, id).map_err(|error| {
                        WorldChunkJournalError::InventoryDecision(error.to_string())
                    })?;
                }
                Ok(())
            })
            .map_err(|error| std::io::Error::other(error).into())
    }

    pub(crate) async fn commit_storage(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        transaction: &ScriptInventoryStorageTransaction,
    ) -> Result<bool, PluginStorageMutationError> {
        let Some((root, journal)) = &self.world else {
            return Ok(false);
        };
        // Acquire save admission before reserving a WAL turn or participant locks.
        // This reuses the existing file-writer authority, not a second player lock.
        let _save_guard = self.save_coordinator.lock().await;
        let id = journal.reserve_decision_ids(1).map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?[0];
        journal.wait_for_append_turn(id).await.map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?;
        let current_tick = self.sessions.simulation_tick();
        let mut commit = WorldInventoryCommit {
            storage,
            journal,
            root,
            id,
            current_tick,
            decided: false,
        };
        let result = self.sessions.commit_script_inventory_storage_transaction(
            plugin_id,
            transaction,
            &self.items,
            &self.item_facts,
            &mut commit,
        );
        if !commit.decided {
            journal
                .record_reserved_snapshot_groups(current_tick, vec![(id, Vec::new())])
                .map_err(|error| {
                    self.sessions.report_world_chunk_journal_failure();
                    std::io::Error::other(error)
                })?;
        } else if matches!(result, Ok(true)) {
            journal
                .mark_inventory_projected(id)
                .expect("unacknowledged inventory decision remains retained");
        } else if matches!(
            result,
            Err(PluginStorageMutationError::DurabilityUnknown(_))
        ) {
            // A durable decision with incomplete participants cannot admit more
            // world work indefinitely while its checkpoint cutoff is retained.
            self.sessions.report_world_chunk_journal_failure();
        }
        result
    }
}

struct WorldInventoryCommit<'a> {
    storage: &'a mut PluginStorage,
    journal: &'a WorldChunkJournal,
    root: &'a Path,
    id: u64,
    current_tick: u64,
    decided: bool,
}

impl ScriptStorageTransactionPrepare for WorldInventoryCommit<'_> {
    type Prepared = PreparedStorageBatch;
    type Error = PluginStorageMutationError;

    fn prepare(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
        inventory: PlayerInventoryRecovery,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        self.storage
            .prepare_batch(plugin_id, mutations, Some(inventory))
    }

    fn commit(
        &mut self,
        batch: Self::Prepared,
    ) -> Result<u64, ScriptStorageCommitError<Self::Error>> {
        self.commit_prepared(batch)
    }
}

impl OwnedInventoryPrepare for WorldInventoryCommit<'_> {
    type Prepared = PreparedStorageBatch;
    type Error = PluginStorageMutationError;

    fn prepare_owned(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        inventory: Option<PlayerInventoryRecovery>,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        self.storage
            .prepare_owned_batch(plugin_id, request, payload, inventory)
    }

    fn commit_owned(
        &mut self,
        batch: Self::Prepared,
    ) -> Result<u64, ScriptStorageCommitError<Self::Error>> {
        self.commit_prepared(batch)
    }

    fn resident_endpoint_state(
        &self,
        plugin_id: &str,
        endpoint: &ScriptInventoryEndpoint,
    ) -> Result<ResidentEndpointState, ScriptOperationFailure> {
        let handle = endpoint
            .resident_handle()
            .ok_or(ScriptOperationFailure::InvalidRequest)?;
        let resident = self
            .storage
            .residents()
            .record(handle)
            .ok_or(ScriptOperationFailure::NotFound)?;
        if resident.plugin_id != plugin_id {
            return Err(ScriptOperationFailure::Forbidden);
        }
        let (equipment, carry, revision) = match self.storage.resident_orders().record(handle) {
            Some(record) if record.entity_uuid == resident.entity_uuid => (
                record.equipment.clone(),
                record.carry.clone(),
                record.revision,
            ),
            _ => (
                vec![None; usize::from(MAX_RESIDENT_EQUIPMENT_SLOTS)],
                vec![None; usize::from(MAX_RESIDENT_CARRY_SLOTS)],
                0,
            ),
        };
        Ok(ResidentEndpointState {
            entity_uuid: resident.entity_uuid.clone(),
            revision,
            equipment: equipment
                .iter()
                .map(|stack| stack.as_ref().map(gear_from_stack))
                .collect(),
            carry: carry
                .iter()
                .map(|stack| stack.as_ref().map(gear_from_stack))
                .collect(),
        })
    }

    fn prepare_resident_gear(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        updates: Vec<ResidentGearUpdate>,
        inventory: Option<PlayerInventoryRecovery>,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        let mut changes = Vec::with_capacity(updates.len());
        for update in updates {
            // The resident's identity comes from C3's ledger; an existing order
            // record keeps its order, work and assignment, only its canonical
            // gear is replaced by this committed transfer.
            let entity_uuid = self
                .storage
                .resident_orders()
                .record(&update.handle)
                .map_or_else(
                    || update.entity_uuid.clone(),
                    |record| record.entity_uuid.clone(),
                );
            let mut record = match self.storage.resident_orders().record(&update.handle) {
                Some(record) => record.clone(),
                None => DurableResidentOrderRecord::empty(
                    update.handle.clone(),
                    plugin_id.to_owned(),
                    entity_uuid,
                    DurableAssignment::Civilian,
                ),
            };
            record.equipment = update
                .equipment
                .iter()
                .map(|stack| stack.as_ref().map(stack_from_gear))
                .collect();
            record.carry = update
                .carry
                .iter()
                .map(|stack| stack.as_ref().map(stack_from_gear))
                .collect();
            changes.push(DurableResidentOrderChange::Record {
                record: Box::new(record),
            });
        }
        self.storage
            .prepare_resident_gear_batch(plugin_id, request, payload, changes, inventory)
    }

    fn gear_revision(&self) -> u64 {
        self.storage.revision.saturating_add(1)
    }
}

/// One durable resident stack projected onto the C1 gear boundary.
fn gear_from_stack(stack: &DurableResidentStack) -> ResidentGearStack {
    ResidentGearStack {
        resource_id: stack.item_id.clone(),
        count: stack.count,
        damage: stack.damage,
        enchantments: stack
            .enchantments
            .iter()
            .map(|enchantment| (enchantment.id.clone(), i32::from(enchantment.level)))
            .collect(),
        custom_name: stack.custom_name.clone(),
        item_model: stack.item_model.clone(),
    }
}

/// One C1 gear stack projected back onto the durable resident stack. The caller
/// has already validated every identity and bound against the canonical
/// registries, so the conversion is lossless.
fn stack_from_gear(gear: &ResidentGearStack) -> DurableResidentStack {
    DurableResidentStack {
        item_id: gear.resource_id.clone(),
        count: gear.count,
        damage: gear.damage,
        enchantments: gear
            .enchantments
            .iter()
            .map(
                |(id, level)| super::resident_orders::DurableResidentEnchantment {
                    id: id.clone(),
                    level: u8::try_from(*level).unwrap_or(u8::MAX),
                },
            )
            .collect(),
        custom_name: gear.custom_name.clone(),
        item_model: gear.item_model.clone(),
    }
}

impl WorldInventoryCommit<'_> {
    fn commit_prepared(
        &mut self,
        mut batch: PreparedStorageBatch,
    ) -> Result<u64, ScriptStorageCommitError<PluginStorageMutationError>> {
        let player = batch.inventory.take();
        let payload = encode_storage_batch(&batch);
        batch.inventory = player;
        let projection = frame(&payload.map_err(ScriptStorageCommitError::NotCommitted)?);
        self.storage
            .compact_before_append_if_needed(projection.len())
            .map_err(ScriptStorageCommitError::NotCommitted)?;
        if let Err(error) = self.journal.record_reserved_inventory_decision(
            self.current_tick,
            self.id,
            Vec::new(),
            &batch,
        ) {
            if error.outcome_unknown() {
                self.decided = true;
                return Err(ScriptStorageCommitError::DurabilityUnknown(
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error)),
                ));
            }
            let error = match error {
                WorldChunkJournalError::InventoryEncoding(error) => error,
                error => PluginStorageMutationError::Io(std::io::Error::other(error)),
            };
            return Err(ScriptStorageCommitError::NotCommitted(error));
        }
        self.decided = true;
        self.project(batch, &projection).map_err(|error| {
            ScriptStorageCommitError::DurabilityUnknown(match error {
                error @ PluginStorageMutationError::DurabilityUnknown(_) => error,
                error => {
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error))
                }
            })
        })?;
        Ok(self.id)
    }

    fn project(
        &mut self,
        mut batch: PreparedStorageBatch,
        frame: &[u8],
    ) -> Result<(), PluginStorageMutationError> {
        let player = batch.inventory.take();
        self.storage.append_frame(frame, false)?;
        self.storage
            .install_storage_batch(batch)
            .expect("prepared inventory projection matches the actor-owned ledger");
        if let Some(player) = player {
            player
                .recover(self.root, self.id)
                .map_err(std::io::Error::other)?;
        }
        Ok(())
    }
}

impl InventoryRuntime {
    /// Execute one `solaris.*_owned_inventory` / reservation request. Queries
    /// answer from live session state; every mutation reserves one world journal
    /// decision and commits the plugin operation receipt together with the
    /// canonical player inventory after-image.
    pub(crate) async fn execute_owned_inventory(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::Inventory { operation } = request.operation() else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ));
        };
        match operation {
            ScriptOwnedInventoryOperation::Query {
                endpoint,
                expected_revision,
            } => {
                if endpoint.resident_handle().is_some() {
                    return Ok(self.resident_inventory_snapshot(
                        storage,
                        plugin_id,
                        endpoint,
                        *expected_revision,
                    ));
                }
                if let ScriptInventoryEndpoint::Warehouse { handle } = endpoint {
                    return Ok(self.warehouse_inventory_snapshot(
                        storage,
                        plugin_id,
                        handle,
                        *expected_revision,
                    ));
                }
                Ok(self
                    .sessions
                    .query_owned_inventory(endpoint, *expected_revision, &self.items))
            }
            ScriptOwnedInventoryOperation::ReservationStatus { reservation_ref } => Ok(
                super::owned_inventory::reservation_status(storage, plugin_id, reservation_ref),
            ),
            ScriptOwnedInventoryOperation::Release {
                operation_id,
                reservation_ref,
                expected_revision,
            } => {
                if let Some(outcome) =
                    replay_owned_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                let payload = match super::owned_inventory::release_payload(
                    storage,
                    plugin_id,
                    reservation_ref,
                    *expected_revision,
                ) {
                    Ok(payload) => payload,
                    Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
                };
                self.commit_owned_decision(
                    storage,
                    plugin_id,
                    operation_id,
                    move |commit, _decision_id| {
                        let prepared = match commit
                            .prepare_owned(plugin_id, request, payload, None)?
                        {
                            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
                            ScriptStoragePrepareOutcome::Rejected => {
                                return Ok(OwnedInventoryCommit::Rejected(
                                    ScriptOperationOutcome::rejected(ScriptOperationFailure::Busy),
                                ));
                            }
                        };
                        match commit.commit_owned(prepared) {
                            Ok(_) => Ok(OwnedInventoryCommit::Committed),
                            Err(ScriptStorageCommitError::NotCommitted(error)) => Err(error),
                            Err(ScriptStorageCommitError::DurabilityUnknown(error)) => Err(error),
                        }
                    },
                )
                .await
            }
            ScriptOwnedInventoryOperation::Transfer {
                actor_id,
                transfers,
                expected_revisions,
                ..
            } => {
                let Some(operation_id) = request.operation_id() else {
                    return Ok(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::InvalidRequest,
                    ));
                };
                if let Some(outcome) =
                    replay_owned_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                let endpoints = crate::play::owned_inventory::transfer_endpoints(transfers);
                let reserved: BTreeMap<ScriptInventoryEndpoint, BTreeMap<String, u64>> = endpoints
                    .iter()
                    .map(|endpoint| (endpoint.clone(), storage.reserved_quantities(endpoint)))
                    .collect();
                let reserved = &reserved;
                self.commit_owned_decision(
                    storage,
                    plugin_id,
                    operation_id,
                    move |commit, decision_id| {
                        self.sessions.commit_owned_inventory_transfer(
                            plugin_id,
                            *actor_id,
                            transfers,
                            expected_revisions,
                            decision_id,
                            reserved,
                            request,
                            &self.items,
                            &self.item_facts,
                            commit,
                        )
                    },
                )
                .await
            }
            ScriptOwnedInventoryOperation::Reserve {
                operation_id,
                endpoint,
                resource_plan,
                expected_revision,
            } => {
                if let Some(outcome) =
                    replay_owned_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                let Some(reservation_ref) =
                    super::owned_inventory::fresh_reservation_ref(storage, plugin_id)
                else {
                    return Ok(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::Busy,
                    ));
                };
                let reserved = storage.reserved_quantities(endpoint);
                let reserved = &reserved;
                self.commit_owned_decision(
                    storage,
                    plugin_id,
                    operation_id,
                    move |commit, _decision_id| {
                        self.sessions.commit_owned_inventory_reservation(
                            plugin_id,
                            endpoint,
                            resource_plan,
                            expected_revision,
                            &reservation_ref,
                            reserved,
                            request,
                            &self.items,
                            commit,
                        )
                    },
                )
                .await
            }
            _ => Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            )),
        }
    }

    /// Canonical snapshot of one resident endpoint, or the typed failure that
    /// keeps a plugin from guessing at a foreign or absent resident. The
    /// revision is the resident's durable order record revision, the same fence
    /// a transfer round-trips.
    fn resident_inventory_snapshot(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        endpoint: &ScriptInventoryEndpoint,
        expected_revision: Option<u64>,
    ) -> ScriptOperationOutcome {
        let Some(handle) = endpoint.resident_handle() else {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::InvalidRequest);
        };
        let Some(resident) = storage.residents().record(handle) else {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::NotFound);
        };
        if resident.plugin_id != plugin_id {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::Forbidden);
        }
        let (equipment, carry, revision) = match storage.resident_orders().record(handle) {
            Some(record) if record.entity_uuid == resident.entity_uuid => (
                record.equipment.clone(),
                record.carry.clone(),
                record.revision,
            ),
            _ => (
                vec![None; usize::from(MAX_RESIDENT_EQUIPMENT_SLOTS)],
                vec![None; usize::from(MAX_RESIDENT_CARRY_SLOTS)],
                0,
            ),
        };
        if expected_revision.is_some_and(|expected| expected != revision) {
            return ScriptOperationOutcome::rejected(ScriptOperationFailure::StaleRevision);
        }
        let gear = match endpoint {
            ScriptInventoryEndpoint::ResidentEquipment { .. } => equipment
                .iter()
                .map(|stack| stack.as_ref().map(gear_from_stack))
                .collect::<Vec<_>>(),
            _ => carry
                .iter()
                .map(|stack| stack.as_ref().map(gear_from_stack))
                .collect::<Vec<_>>(),
        };
        let slots = match crate::play::owned_inventory::gear_slots_to_items(&gear, &self.items) {
            Ok(slots) => slots,
            Err(failure) => return ScriptOperationOutcome::rejected(failure),
        };
        let snapshot = match crate::play::owned_inventory::owned_inventory_snapshot(
            endpoint.clone(),
            revision,
            &slots,
            &self.items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => return ScriptOperationOutcome::rejected(failure),
        };
        match ScriptOperationOutcome::committed(
            revision,
            ScriptOperationPayload::OwnedInventory {
                result: Box::new(mc_script::ScriptOwnedInventoryResult::Snapshot {
                    inventory: snapshot,
                }),
            },
        ) {
            Ok(outcome) => outcome,
            Err(_) => ScriptOperationOutcome::rejected(ScriptOperationFailure::InvalidRequest),
        }
    }

    async fn commit_owned_decision<F>(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        operation_id: &str,
        action: F,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError>
    where
        F: FnOnce(
            &mut WorldInventoryCommit<'_>,
            u64,
        ) -> Result<OwnedInventoryCommit, PluginStorageMutationError>,
    {
        let Some((root, journal)) = &self.world else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let _save_guard = self.save_coordinator.lock().await;
        let id = journal.reserve_decision_ids(1).map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?[0];
        journal.wait_for_append_turn(id).await.map_err(|error| {
            self.sessions.report_world_chunk_journal_failure();
            std::io::Error::other(error)
        })?;
        let current_tick = self.sessions.simulation_tick();
        let result = {
            let mut commit = WorldInventoryCommit {
                storage: &mut *storage,
                journal,
                root,
                id,
                current_tick,
                decided: false,
            };
            let result = action(&mut commit, id);
            if !commit.decided {
                journal
                    .record_reserved_snapshot_groups(current_tick, vec![(id, Vec::new())])
                    .map_err(std::io::Error::other)?;
            } else if matches!(result, Ok(OwnedInventoryCommit::Committed)) {
                journal
                    .mark_inventory_projected(id)
                    .expect("unacknowledged inventory decision remains retained");
            } else if matches!(
                result,
                Err(PluginStorageMutationError::DurabilityUnknown(_))
            ) {
                self.sessions.report_world_chunk_journal_failure();
            }
            result
        };
        match result {
            Ok(OwnedInventoryCommit::Rejected(outcome)) => Ok(outcome),
            Ok(OwnedInventoryCommit::Committed) => Ok(storage
                .operation_receipt(plugin_id, operation_id)
                .expect("committed owned inventory receipt remains installed")
                .outcome
                .clone()),
            Err(error) => Err(error),
        }
    }
}

fn replay_owned_operation(
    storage: &PluginStorage,
    plugin_id: &str,
    request: &ScriptOperationRequest,
    operation_id: &str,
) -> Option<ScriptOperationOutcome> {
    let fingerprint = owned_inventory_fingerprint(request.operation());
    storage
        .operation_receipt(plugin_id, operation_id)
        .map(|receipt| {
            if receipt.fingerprint == fingerprint {
                receipt.outcome.clone()
            } else {
                ScriptOperationOutcome::rejected(ScriptOperationFailure::OperationConflict)
            }
        })
}

#[cfg(test)]
impl InventoryRuntime {
    pub(crate) fn player_only_for_test(
        root: &Path,
        sessions: Arc<SessionRegistry>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
    ) -> Self {
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let (journal, pending) =
            WorldChunkJournal::open_for_test(root, blocks, Arc::clone(&items)).unwrap();
        assert!(journal.decode_pending(&pending).unwrap().is_empty());
        sessions.install_world_chunk_journal(journal);
        Self::new(
            Some(root),
            &ShutdownHandle::default(),
            sessions,
            items,
            item_facts,
        )
    }
}
