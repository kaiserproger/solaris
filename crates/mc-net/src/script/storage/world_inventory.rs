use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    MAX_RESIDENT_CARRY_SLOTS, MAX_RESIDENT_EQUIPMENT_SLOTS, ScriptInventoryEndpoint,
    ScriptInventoryExpectedRevision, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventoryStorageTransaction, ScriptOperation,
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult, ScriptOwnedItemTransfer,
    ScriptStorageMutation,
};

use super::resident_orders::{
    DurableAssignment, DurableResidentOrderChange, DurableResidentOrderRecord, DurableResidentStack,
};
use super::{
    MAX_TRANSACTION_FRAME_BYTES, OP_STORAGE_BATCH, PluginStorage, PluginStorageMutationError,
    PluginStorageStartError, PreparedStorageBatch, decode_storage_batch, encode_storage_batch,
    frame,
};
use crate::play::SessionRegistry;
use crate::play::owned_inventory::{
    OwnedInventoryCommit, OwnedInventoryPrepare, ResidentEndpointState, ResidentGearStack,
    ResidentGearUpdate, WarehousePlayerParticipant, WarehouseTransferRequest, gear_slots_to_items,
    items_to_gear_slots, owned_inventory_fingerprint,
};
use crate::play::persistence::inventory_recovery::PlayerInventoryRecovery;
use crate::play::resident_work::ResidentWorldEdit;
use crate::play::world_journal::{WorldChunkJournal, WorldChunkJournalError};
use crate::play::{
    ScriptStorageCommitError, ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
use crate::server::ShutdownHandle;

impl PreparedStorageBatch {
    pub(crate) const MAX_ENCODED_BYTES: usize = MAX_TRANSACTION_FRAME_BYTES;

    /// Encode any prepared plugin storage projection for one world-journal
    /// decision. A world decision may carry a settlement portion rather than an
    /// inventory participant.
    pub(crate) fn encode_world_decision(&self) -> Result<Vec<u8>, PluginStorageMutationError> {
        encode_storage_batch(self)
    }

    pub(crate) fn encode_world_inventory(&self) -> Result<Vec<u8>, PluginStorageMutationError> {
        self.validate_inventory_participant()
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        self.encode_world_decision()
    }

    /// Decode any plugin storage projection recovered from a world decision.
    pub(crate) fn decode_world_decision(payload: &[u8]) -> Result<Self, PluginStorageStartError> {
        let Some((&OP_STORAGE_BATCH, body)) = payload.split_first() else {
            return Err(PluginStorageStartError::Malformed("world decision batch"));
        };
        if payload.len() > MAX_TRANSACTION_FRAME_BYTES {
            return Err(PluginStorageStartError::Malformed("world decision size"));
        }
        decode_storage_batch(body)
    }

    fn validate_inventory_participant(&self) -> Result<(), PluginStorageStartError> {
        // A plugin-ledger decision carries at least one non-ledger participant:
        // a player's canonical inventory, a plugin-owned record that rode the
        // receipt (a worker's cargo), or the receipt's own owned-inventory
        // projection.
        let receipt_participates = self.operation.as_ref().is_some_and(|receipt| {
            matches!(
                receipt.outcome.payload(),
                mc_script::ScriptOperationPayload::OwnedInventory { .. }
            )
        });
        if self.inventory.is_none() && self.order.is_empty() && !receipt_participates {
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
    warehouse_reservation_floors:
        Arc<arc_swap::ArcSwap<HashMap<mc_world::BlockPos, BTreeMap<u32, u64>>>>,
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
        let warehouse_reservation_floors =
            Arc::new(arc_swap::ArcSwap::from_pointee(HashMap::new()));
        sessions.install_warehouse_reservation_floors(Arc::clone(&warehouse_reservation_floors));
        Self {
            world,
            sessions,
            items,
            item_facts,
            save_coordinator: shutdown.save_coordinator(),
            settlement: None,
            settlement_world: None,
            resident_world: None,
            warehouse_reservation_floors,
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
        if let Some((root, journal)) = &self.world {
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
                .map_err(std::io::Error::other)?;
        }
        self.refresh_warehouse_reservation_floors(storage);
        Ok(())
    }

    /// Rebuild the physical reservation floor cache from durable reservations.
    /// Unloaded bindings retain their exact durable source position; missing,
    /// stale, or destroyed bindings contribute no physical chest floor.
    pub(crate) fn refresh_warehouse_reservation_floors(&self, storage: &PluginStorage) {
        let mut floors: HashMap<mc_world::BlockPos, BTreeMap<u32, u64>> = HashMap::new();
        for (plugin_id, reservation_ref) in storage.reservations.keys() {
            let Some((_, reservation)) = storage.settlement_reservation(plugin_id, reservation_ref)
            else {
                continue;
            };
            if reservation.released {
                continue;
            }
            let ScriptInventoryEndpoint::Warehouse { handle } = &reservation.endpoint else {
                continue;
            };
            let position = match self.resolve_warehouse_container(storage, plugin_id, handle) {
                Ok(container) => container.position,
                Err(ScriptOperationFailure::Unloaded) => {
                    let Ok(position) = self.unloaded_warehouse_position(storage, plugin_id, handle)
                    else {
                        continue;
                    };
                    position
                }
                Err(_) => continue,
            };
            let floor = floors
                .entry(mc_world::BlockPos {
                    x: position[0],
                    y: position[1],
                    z: position[2],
                })
                .or_default();
            for quantity in reservation.quantities {
                if quantity.remaining == 0 {
                    continue;
                }
                let Ok(resource_id) = mc_data::Identifier::parse(&quantity.resource_id) else {
                    continue;
                };
                let Some(item_id) = self.items.id_of(&resource_id) else {
                    continue;
                };
                let total = floor
                    .get(&item_id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(quantity.remaining);
                floor.insert(item_id, total);
            }
        }
        self.warehouse_reservation_floors.store(Arc::new(floors));
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
        let projection =
            inventory_ledger_frame(&mut batch).map_err(ScriptStorageCommitError::NotCommitted)?;
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
        batch: PreparedStorageBatch,
        frame: &[u8],
    ) -> Result<(), PluginStorageMutationError> {
        let (storage, root, decision_id) = (&mut *self.storage, self.root, self.id);
        append_inventory_projection(storage, root, decision_id, batch, frame)
    }
}

/// The plugin ledger frame of one decided inventory batch.
///
/// The batch's mutations and its operation receipt are the ledger's half; the
/// player after-image belongs to the world journal, which recovers it with the
/// same decision, so it never enters this frame.
fn inventory_ledger_frame(
    batch: &mut PreparedStorageBatch,
) -> Result<Vec<u8>, PluginStorageMutationError> {
    let player = batch.inventory.take();
    let payload = encode_storage_batch(batch);
    batch.inventory = player;
    Ok(frame(&payload?))
}

/// Project one decided inventory batch into the plugin ledger and the player's
/// own file.
///
/// The caller has already compacted the ledger for `frame` and the decision
/// that carries both participants is already durable; this is the durable
/// projection that must precede the acknowledgement, and a failure here leaves
/// the decision unprojected for the next startup to replay.
fn append_inventory_projection(
    storage: &mut PluginStorage,
    root: &Path,
    decision_id: u64,
    mut batch: PreparedStorageBatch,
    frame: &[u8],
) -> Result<(), PluginStorageMutationError> {
    let player = batch.inventory.take();
    storage.append_frame(frame, false)?;
    storage
        .install_storage_batch(batch)
        .expect("prepared inventory projection matches the actor-owned ledger");
    if let Some(player) = player {
        player
            .recover(root, decision_id)
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

/// The container half of one prepared deposit, as the caller observed it.
pub(super) struct PreparedDepositContainer {
    pub(super) position: [i32; 3],
    /// The container's expected and planned 27-slot canonical images.
    pub(super) expected: Vec<ItemStack>,
    pub(super) updated: Vec<ItemStack>,
}

/// What one prepared deposit's world half answered.
pub(super) enum PreparedDepositCommit {
    /// The container's after-image and the encoded receipt are durable under
    /// this decision, and the plugin ledger frame has been projected.
    Committed(u64),
    /// The world half refused, or core owns no journal to project into:
    /// nothing changed anywhere.
    Refused(ScriptOperationFailure),
}

/// What one prepared structure portion's world half answered.
pub(super) enum PreparedStructurePortionCommit {
    /// The block after-images and settlement receipt are durable together, and
    /// the plugin ledger projection has been installed.
    Committed,
    /// The world half refused before a decision was accepted.
    Refused(ScriptOperationFailure),
}

impl InventoryRuntime {
    /// Commit one prepared deposit: the container's canonical slots and the
    /// receipt's own second participant move under ONE world-journal decision.
    ///
    /// This is the whole tail every server-owned deposit shares - the player's
    /// canonical inventory, a worker's canonical record, or the receipt's own
    /// owned-inventory projection - so the append, the projection and the
    /// acknowledgement can never be ordered differently by two callers. Each
    /// caller keeps its own fencing and planning, and publishes afterwards.
    ///
    /// Both halves must be able to land before either may: a container whose
    /// receipt has nowhere to go is not recoverable with its participant, so a
    /// missing journal refuses before the world is touched.
    pub(super) async fn commit_prepared_deposit(
        &self,
        storage: &mut PluginStorage,
        prepared: PreparedStorageBatch,
        container: PreparedDepositContainer,
        player: Option<WarehousePlayerParticipant>,
    ) -> Result<PreparedDepositCommit, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(PreparedDepositCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let Some((root, journal)) = &self.world else {
            return Ok(PreparedDepositCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let encoded = prepared.encode_world_inventory()?;
        let position = mc_world::BlockPos {
            x: container.position[0],
            y: container.position[1],
            z: container.position[2],
        };
        let decision_id = match world
            .commit_warehouse_transfer(WarehouseTransferRequest {
                position,
                expected_state_id: self.sessions.chest_state_id(position),
                expected_container: container.expected,
                updated_container: container.updated,
                player,
                receipt: encoded,
            })
            .await
        {
            Ok(decision_id) => decision_id,
            Err(failure) => return Ok(PreparedDepositCommit::Refused(failure)),
        };
        let mut batch = prepared;
        let projection = inventory_ledger_frame(&mut batch)?;
        storage.compact_before_append_if_needed(projection.len())?;
        if let Err(error) =
            append_inventory_projection(storage, root, decision_id, batch, &projection)
        {
            return Err(match error {
                error @ PluginStorageMutationError::DurabilityUnknown(_) => error,
                error => {
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error))
                }
            });
        }
        journal
            .mark_inventory_projected(decision_id)
            .expect("unacknowledged inventory decision remains retained");
        Ok(PreparedDepositCommit::Committed(decision_id))
    }
}

impl InventoryRuntime {
    /// Commit a finite construction portion: its server-owned block edits and
    /// settlement receipt share one world-journal decision, then the receipt
    /// projects into the plugin ledger from that same decision.
    pub(super) async fn commit_prepared_structure_portion(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        structure_id: &str,
        blocks: &[super::settlement::StructureBlockPlacement],
        prepared: PreparedStorageBatch,
    ) -> Result<PreparedStructurePortionCommit, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(PreparedStructurePortionCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let Some((root, journal)) = &self.world else {
            return Ok(PreparedStructurePortionCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let mut batch = prepared;
        let projection = inventory_ledger_frame(&mut batch)?;
        storage.compact_before_append_if_needed(projection.len())?;
        let encoded = batch.encode_world_decision()?;
        let decision_id = match world
            .commit_structure_portion(plugin_id, structure_id, blocks, encoded)
            .await
        {
            Ok(decision_id) => decision_id,
            Err(failure) => return Ok(PreparedStructurePortionCommit::Refused(failure)),
        };
        if let Err(error) =
            append_inventory_projection(storage, root, decision_id, batch, &projection)
        {
            return Err(match error {
                error @ PluginStorageMutationError::DurabilityUnknown(_) => error,
                error => {
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error))
                }
            });
        }
        journal
            .mark_inventory_projected(decision_id)
            .expect("unacknowledged structure decision remains retained");
        self.refresh_warehouse_reservation_floors(storage);
        Ok(PreparedStructurePortionCommit::Committed)
    }
}

#[derive(Debug)]
pub(super) enum PreparedResidentEditCommit {
    /// Both the conditional block images and resident receipt are durable.
    Committed,
    /// No source image or journal decision was accepted.
    Refused(ScriptOperationFailure),
}

impl InventoryRuntime {
    /// Commit previewed resident world edits, cargo after-image and work
    /// watermark in one world-journal decision. The receipt contains canonical
    /// break loot; the world only consumes the exact preview preconditions.
    pub(super) async fn commit_prepared_resident_edits(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        dimension: &str,
        edits: &[ResidentWorldEdit],
        prepared: PreparedStorageBatch,
    ) -> Result<PreparedResidentEditCommit, PluginStorageMutationError> {
        let Some(world) = self.resident_world() else {
            return Ok(PreparedResidentEditCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let Some((root, journal)) = &self.world else {
            return Ok(PreparedResidentEditCommit::Refused(
                ScriptOperationFailure::RuntimeUnavailable,
            ));
        };
        let mut batch = prepared;
        let projection = inventory_ledger_frame(&mut batch)?;
        storage.compact_before_append_if_needed(projection.len())?;
        let encoded = batch.encode_world_decision()?;
        let decision_id = match world
            .commit_world_edits(plugin_id, dimension, edits, encoded)
            .await
        {
            Ok(decision_id) => decision_id,
            Err(failure) => return Ok(PreparedResidentEditCommit::Refused(failure)),
        };
        if let Err(error) =
            append_inventory_projection(storage, root, decision_id, batch, &projection)
        {
            return Err(match error {
                error @ PluginStorageMutationError::DurabilityUnknown(_) => error,
                error => {
                    PluginStorageMutationError::DurabilityUnknown(std::io::Error::other(error))
                }
            });
        }
        journal
            .mark_inventory_projected(decision_id)
            .expect("unacknowledged resident edit decision remains retained");
        Ok(PreparedResidentEditCommit::Committed)
    }
}

impl InventoryRuntime {
    /// Plan one worker's move between its own endpoint and the bound warehouse.
    ///
    /// The worker moves items between its own canonical endpoint and the bound
    /// container's loaded slots, in either direction; the durable binding
    /// resolves the handle, and a foreign, inactive, unresolvable or unloaded
    /// container stays the typed refusal that keeps the worker from moving
    /// anything core cannot see.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn plan_resident_warehouse_move(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        source: &ScriptInventoryEndpoint,
        destination: &ScriptInventoryEndpoint,
        item: Option<&str>,
        record: &DurableResidentOrderRecord,
        limit: u64,
    ) -> Result<ResidentWarehouseMove, ScriptOperationFailure> {
        // Exactly one side is the container; the other is the worker's own
        // endpoint, and which one is which decides what "updated" means below.
        let (resident_slots, handle) = match (source, destination) {
            (
                ScriptInventoryEndpoint::ResidentCarry { handle: resident }
                | ScriptInventoryEndpoint::ResidentEquipment { handle: resident },
                ScriptInventoryEndpoint::Warehouse { handle },
            ) if resident == &record.handle => (
                match source {
                    ScriptInventoryEndpoint::ResidentCarry { .. } => &record.carry,
                    _ => &record.equipment,
                },
                handle,
            ),
            (
                ScriptInventoryEndpoint::Warehouse { handle },
                ScriptInventoryEndpoint::ResidentCarry { handle: resident }
                | ScriptInventoryEndpoint::ResidentEquipment { handle: resident },
            ) if resident == &record.handle => (
                match destination {
                    ScriptInventoryEndpoint::ResidentCarry { .. } => &record.carry,
                    _ => &record.equipment,
                },
                handle,
            ),
            _ => return Err(ScriptOperationFailure::InvalidRequest),
        };
        let container = self.resolve_warehouse_container(storage, plugin_id, handle)?;
        let gear: Vec<Option<ResidentGearStack>> = resident_slots
            .iter()
            .map(|stack| stack.as_ref().map(gear_from_stack))
            .collect();
        let resident_items = gear_slots_to_items(&gear, &self.items)?;
        let (source_slots, destination_slots) = match source {
            ScriptInventoryEndpoint::Warehouse { .. } => (&container.items, &resident_items),
            _ => (&resident_items, &container.items),
        };
        let planned = crate::play::owned_inventory::plan_warehouse_deposit(
            source,
            destination,
            source_slots,
            destination_slots,
            limit,
            &self.items,
            &self.item_facts,
            item,
        )?;
        // The planner reports both sides; the worker's is whichever endpoint it
        // was, and the container's is the other one.
        let (updated_resident, updated_container) = match source {
            ScriptInventoryEndpoint::Warehouse { .. } => {
                (planned.updated_destination, planned.updated_source)
            }
            _ => (planned.updated_source, planned.updated_destination),
        };
        let planned_gear = items_to_gear_slots(&updated_resident, &self.items)?;
        let updated: Vec<Option<DurableResidentStack>> = planned_gear
            .iter()
            .map(|stack| stack.as_ref().map(stack_from_gear))
            .collect();
        let mut record = record.clone();
        match source {
            ScriptInventoryEndpoint::ResidentCarry { .. } => record.carry = updated,
            ScriptInventoryEndpoint::ResidentEquipment { .. } => record.equipment = updated,
            _ => match destination {
                ScriptInventoryEndpoint::ResidentCarry { .. } => record.carry = updated,
                _ => record.equipment = updated,
            },
        }
        Ok(ResidentWarehouseMove {
            record: Box::new(record),
            container: PreparedDepositContainer {
                position: container.position,
                expected: container.items,
                updated: updated_container,
            },
            moved: planned.moved,
            withdraws: matches!(source, ScriptInventoryEndpoint::Warehouse { .. }),
        })
    }

    /// Commit one planned worker move: the container's canonical slots and the
    /// worker's record change ride one journal decision, whether the worker
    /// deposited into the container or was issued from it.
    ///
    /// `record` is the record after-image the caller is committing, which is
    /// the plan's own record plus the work assignment it recorded.
    pub(super) async fn commit_resident_warehouse_move(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        record: Box<DurableResidentOrderRecord>,
        payload: ScriptOperationPayload,
        container: &PreparedDepositContainer,
    ) -> Result<ResidentDepositCommit, PluginStorageMutationError> {
        let _save_guard = self.save_coordinator.lock().await;
        let prepared = match storage.prepare_resident_order_operation_batch(
            plugin_id,
            request,
            payload,
            vec![DurableResidentOrderChange::Record { record }],
        ) {
            Ok(ScriptStoragePrepareOutcome::Prepared(prepared)) => prepared,
            Ok(ScriptStoragePrepareOutcome::Rejected) => {
                return Ok(ResidentDepositCommit::Refused);
            }
            Err(error) => return Err(error),
        };
        let commit = self
            .commit_prepared_deposit(
                storage,
                prepared,
                PreparedDepositContainer {
                    position: container.position,
                    expected: container.expected.clone(),
                    updated: container.updated.clone(),
                },
                // The worker's own second participant is the record inside the
                // receipt: no player is fenced or after-imaged by this deposit.
                None,
            )
            .await?;
        Ok(match commit {
            PreparedDepositCommit::Committed(_) => ResidentDepositCommit::Committed,
            // A moved container fence, an absent container and an engine that
            // could not take the command all leave the worker's cargo where it
            // was: the prepared batch is dropped unappended.
            PreparedDepositCommit::Refused(_) => ResidentDepositCommit::Refused,
        })
    }
}

/// One planned worker move against a bound container, in either direction.
///
/// The plan is computed against the container the worker's own work step read
/// and the record it started from; nothing is mutated. The caller commits the
/// record after-image and the container images under one durable decision, or
/// neither.
pub(super) struct ResidentWarehouseMove {
    /// The worker's record with the moved items already added or removed.
    pub(super) record: Box<DurableResidentOrderRecord>,
    /// The container half the commit fences.
    pub(super) container: PreparedDepositContainer,
    /// Resource id -> units that entered the container (deposit) or left it
    /// (withdrawal).
    pub(super) moved: BTreeMap<String, u64>,
    /// The moved units left the container: the assignment reports them as
    /// consumed rather than produced.
    pub(super) withdraws: bool,
}

impl ResidentWarehouseMove {
    /// The work units this move transfers.
    pub(super) fn units(&self) -> u64 {
        self.moved.values().copied().sum()
    }

    /// Unit signed like the receipt reports it: the container's own change.
    pub(super) fn container_delta(&self, units: u64) -> i64 {
        let units = i64::try_from(units).unwrap_or(i64::MAX);
        if self.withdraws { -units } else { units }
    }
}

/// What one worker move left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResidentDepositCommit {
    /// The record and the container are durable under one journal decision.
    Committed,
    /// Nothing changed: the container fence moved, it is not there, or the
    /// engine refused the command.
    Refused,
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
                if endpoints
                    .iter()
                    .any(|endpoint| matches!(endpoint, ScriptInventoryEndpoint::Warehouse { .. }))
                {
                    // A warehouse endpoint is a server-owned container
                    // composite: the container's real slots and the actor's
                    // canonical inventory move under ONE journal decision, so
                    // the player/resident planner below never sees it.
                    return self
                        .commit_warehouse_transfer(
                            storage,
                            plugin_id,
                            request,
                            *actor_id,
                            transfers,
                            expected_revisions,
                        )
                        .await;
                }
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
                if let ScriptInventoryEndpoint::Warehouse { .. } = endpoint {
                    return self
                        .commit_warehouse_reservation(
                            storage,
                            plugin_id,
                            request,
                            operation_id,
                            endpoint,
                            resource_plan,
                            expected_revision,
                            &reservation_ref,
                        )
                        .await;
                }
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

    /// Reserve material from one caller-owned live bound warehouse. Unlike a
    /// player endpoint, the stock is the resolved physical container snapshot;
    /// the binding revision is therefore the reservation's admission fence.
    #[allow(clippy::too_many_arguments)]
    async fn commit_warehouse_reservation(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        operation_id: &str,
        endpoint: &ScriptInventoryEndpoint,
        resource_plan: &mc_script::ScriptInventoryResourcePlan,
        expected_revision: &mc_script::ScriptInventoryFence,
        reservation_ref: &str,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptInventoryEndpoint::Warehouse { handle } = endpoint else {
            unreachable!("warehouse reservation dispatch")
        };
        let container = match self.resolve_warehouse_container(storage, plugin_id, handle) {
            Ok(container) => container,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let position = mc_world::BlockPos {
            x: container.position[0],
            y: container.position[1],
            z: container.position[2],
        };
        let _admission = self
            .sessions
            .lock_warehouse_reservation_admission(position)
            .await;
        let container = match self.resolve_warehouse_container(storage, plugin_id, handle) {
            Ok(container) => container,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let snapshot = match crate::play::owned_inventory::owned_inventory_snapshot(
            endpoint.clone(),
            container.revision,
            &container.items,
            &self.items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        if &snapshot.fence != expected_revision {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::StaleRevision,
            ));
        }
        let totals = match crate::play::owned_inventory::resource_plan_totals(resource_plan) {
            Ok(totals) => totals,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let reserved = storage.reserved_quantities(endpoint);
        for (resource_id, quantity) in &totals {
            let stock = match crate::play::owned_inventory::inventory_resource_stock(
                &container.items,
                &self.items,
                resource_id,
            ) {
                Ok(stock) => stock,
                Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
            };
            if stock
                < reserved
                    .get(resource_id)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(*quantity)
            {
                return Ok(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::InsufficientItems,
                ));
            }
        }
        let previous_floors = self.warehouse_reservation_floors.load_full();
        let mut next_floors = (*previous_floors).clone();
        let floor = next_floors
            .entry(mc_world::BlockPos {
                x: container.position[0],
                y: container.position[1],
                z: container.position[2],
            })
            .or_default();
        for (resource_id, quantity) in &totals {
            let resource = mc_data::Identifier::parse(resource_id)
                .expect("live warehouse stock validation accepted the resource identifier");
            let item_id = self
                .items
                .id_of(&resource)
                .expect("live warehouse stock validation found the resource item");
            *floor.entry(item_id).or_insert(0) += quantity;
        }
        // Publish the pending floor before the first await. The exclusive
        // admission gate then makes concurrent chest clicks resync until either
        // this decision becomes durable or the prior projection is restored.
        self.warehouse_reservation_floors
            .store(Arc::new(next_floors));
        let quantities = totals
            .into_iter()
            .map(|(resource_id, quantity)| {
                ScriptInventoryReservationQuantity::new(resource_id, quantity, 0, 0, quantity)
            })
            .collect();
        let reservation = ScriptInventoryReservationSnapshot::new(
            reservation_ref.to_owned(),
            endpoint.clone(),
            crate::play::owned_inventory::resource_plan_hash(resource_plan),
            quantities,
            None,
            false,
            0,
        );
        let payload = ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Reservation { reservation }),
        };
        let outcome = self
            .commit_owned_decision(
                storage,
                plugin_id,
                operation_id,
                move |commit, _decision_id| {
                    let prepared = match commit.prepare_owned(plugin_id, request, payload, None)? {
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
            .await;
        if outcome
            .as_ref()
            .map_or(true, |outcome| outcome.failure().is_some())
        {
            self.warehouse_reservation_floors.store(previous_floors);
        }
        outcome
    }

    /// Execute one `transfer_owned_items` that names a bound warehouse
    /// container.
    ///
    /// The container's real slots and the actor's canonical inventory move
    /// through one server-owned composite, and the plugin operation receipt
    /// rides that composite's own world-journal decision: the container's
    /// after-image and the receipt are durable together or not at all. The
    /// caller prepares, the composite commits, and only then does this project
    /// the ledger, the player's after-image and the acknowledgement.
    async fn commit_warehouse_transfer(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        actor_id: u64,
        transfers: &[ScriptOwnedItemTransfer],
        expected_revisions: &[ScriptInventoryExpectedRevision],
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(operation_id) = request.operation_id() else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ));
        };
        // The world half is checked by the deposit commit below; acquiring save
        // admission before the ledger batch is prepared is what keeps the
        // batch's transaction id the revision this projection installs.
        let _save_guard = self.save_coordinator.lock().await;
        // One authored container, and only the actor's own inventory beside it:
        // the composite has exactly one container participant.
        let mut handle = None;
        for endpoint in crate::play::owned_inventory::transfer_endpoints(transfers) {
            match endpoint {
                ScriptInventoryEndpoint::Warehouse { handle: named } => {
                    if handle.is_some() {
                        return Ok(ScriptOperationOutcome::rejected(
                            ScriptOperationFailure::InvalidRequest,
                        ));
                    }
                    handle = Some(named);
                }
                ScriptInventoryEndpoint::PlayerInventory { player_id } if player_id == actor_id => {
                }
                ScriptInventoryEndpoint::PlayerInventory { .. } => {
                    return Ok(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::Forbidden,
                    ));
                }
                _ => {
                    return Ok(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::InvalidRequest,
                    ));
                }
            }
        }
        let Some(handle) = handle else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ));
        };
        let container = match self.resolve_warehouse_container(storage, plugin_id, &handle) {
            Ok(container) => container,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let warehouse_endpoint = ScriptInventoryEndpoint::Warehouse {
            handle: handle.clone(),
        };
        let player_endpoint = ScriptInventoryEndpoint::PlayerInventory {
            player_id: actor_id,
        };
        // Both participants are fenced by the revisions the plugin holds: the
        // container by its durable binding revision, the actor by the journal
        // watermark its own inventory round-trips.
        let container_fence = match crate::play::owned_inventory::owned_inventory_snapshot(
            warehouse_endpoint.clone(),
            container.revision,
            &container.items,
            &self.items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let Some(container_expected) = expected_revisions
            .iter()
            .find(|expected| expected.endpoint == warehouse_endpoint)
        else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ));
        };
        if container_expected.fence != container_fence.fence {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::StaleRevision,
            ));
        }
        let Some(actor) = self.sessions.warehouse_transfer_actor(actor_id) else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::NotFound,
            ));
        };
        if actor.recovery_required {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::Busy,
            ));
        }
        let actor_fence = match crate::play::owned_inventory::owned_inventory_snapshot(
            player_endpoint.clone(),
            actor.revision,
            &actor.inventory,
            &self.items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let Some(actor_expected) = expected_revisions
            .iter()
            .find(|expected| expected.endpoint == player_endpoint)
        else {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InvalidRequest,
            ));
        };
        if actor_expected.fence != actor_fence.fence {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::StaleRevision,
            ));
        }
        let inventories = BTreeMap::from([
            (warehouse_endpoint.clone(), container.items.clone()),
            (player_endpoint.clone(), actor.inventory.clone()),
        ]);
        let planned = match crate::play::owned_inventory::plan_owned_item_transfers(
            transfers,
            &inventories,
            &self.items,
            &self.item_facts,
        ) {
            Ok(planned) => planned,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let reserved: BTreeMap<ScriptInventoryEndpoint, BTreeMap<String, u64>> = inventories
            .keys()
            .map(|endpoint| (endpoint.clone(), storage.reserved_quantities(endpoint)))
            .collect();
        if !crate::play::owned_inventory::reservation_stock_survives(
            &planned,
            &reserved,
            &self.items,
        ) {
            return Ok(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::InsufficientItems,
            ));
        }
        let planned_container = planned
            .get(&warehouse_endpoint)
            .expect("planned warehouse inventory")
            .clone();
        let planned_player = planned
            .get(&player_endpoint)
            .expect("planned player inventory")
            .clone();
        // The receipt names the container's resulting fence. The actor's own
        // resulting fence is deliberately not part of it: a player endpoint's
        // revision IS the world-journal decision id, which the composite
        // allocates while this receipt is already encoded, so the plugin reads
        // the actor's fence back from a query instead of a receipt that could
        // not state it.
        let warehouse_snapshot = match crate::play::owned_inventory::owned_inventory_snapshot(
            warehouse_endpoint.clone(),
            container.revision,
            &planned_container,
            &self.items,
        ) {
            Ok(snapshot) => snapshot,
            Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
        };
        let payload = ScriptOperationPayload::OwnedInventory {
            result: Box::new(ScriptOwnedInventoryResult::Transfer {
                inventories: vec![ScriptInventoryExpectedRevision::new(
                    warehouse_endpoint.clone(),
                    warehouse_snapshot.fence,
                )],
            }),
        };
        let recovery =
            match self
                .sessions
                .warehouse_transfer_recovery(actor_id, &planned_player, &self.items)
            {
                Ok(recovery) => recovery,
                Err(failure) => return Ok(ScriptOperationOutcome::rejected(failure)),
            };
        let prepared =
            match storage.prepare_owned_batch(plugin_id, request, payload, Some(recovery)) {
                Ok(ScriptStoragePrepareOutcome::Prepared(prepared)) => prepared,
                Ok(ScriptStoragePrepareOutcome::Rejected) => {
                    return Ok(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::Busy,
                    ));
                }
                Err(error) => return Err(error),
            };
        let commit = self
            .commit_prepared_deposit(
                storage,
                prepared,
                PreparedDepositContainer {
                    position: container.position,
                    expected: container.items.clone(),
                    updated: planned_container,
                },
                Some(WarehousePlayerParticipant {
                    actor_id,
                    expected_inventory: actor.inventory,
                    expected_carried_item: actor.carried_item.clone(),
                    updated_inventory: planned_player,
                    updated_carried_item: actor.carried_item,
                }),
            )
            .await?;
        let decision_id = match commit {
            PreparedDepositCommit::Committed(decision_id) => decision_id,
            PreparedDepositCommit::Refused(failure) => {
                return Ok(ScriptOperationOutcome::rejected(failure));
            }
        };
        self.sessions
            .publish_warehouse_transfer(actor_id, decision_id);
        Ok(storage
            .operation_receipt(plugin_id, operation_id)
            .expect("committed owned inventory receipt remains installed")
            .outcome
            .clone())
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
