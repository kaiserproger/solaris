//! Durable resident identity, lifecycle, and spawn-site reservations.
//!
//! This module owns C3's core authority: an opaque handle resolves to the same
//! entity UUID across restart, unload/reload and region migration, and it never
//! grants cross-plugin access. The durable projection lives in the plugin
//! storage journal next to the operation receipts that make every mutation
//! idempotent, so a replayed journal rebuilds the exact same bindings. Query
//! and mutation work is bounded by the handles the caller names.

use std::collections::BTreeMap;

use mc_data::items::ItemRegistry;
use mc_entity::{EntityLifecycle, EntitySnapshot, SpawnEntity, Vec3};
use mc_script::{
    MAX_RESIDENT_CARRIED_ITEMS, MAX_RESIDENT_CLAIM_DISTANCE, MAX_RESIDENT_LIVE_PER_PLUGIN,
    MAX_RESIDENT_PAGE, MAX_RESIDENT_RECORDS_PER_PLUGIN, ScriptOperation, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest, ScriptPosition,
    ScriptResidentItemSummary, ScriptResidentLifecycle, ScriptResidentLoadedState,
    ScriptResidentOperation, ScriptResidentPois, ScriptResidentResult, ScriptResidentSnapshot,
    resident_entity_uuid, resident_handle_for_entity, resident_handle_for_generation,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::play::resident_work::RESIDENT_WORLD_DIMENSION;

use crate::play::owned_inventory::owned_inventory_fingerprint;

use super::{
    DurableOperationReceipt, PluginStorage, PluginStorageMutationError, PluginStorageStartError,
    PreparedStorageBatch, ScriptStoragePrepareOutcome,
};

/// Durable lifecycle projection kept by core. `alive_unloaded` is derived when a
/// resident is read back, never persisted; the tombstone states are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResidentDisposition {
    Alive,
    Dead,
    Released,
}

impl ResidentDisposition {
    const fn persisted_lifecycle(self) -> ScriptResidentLifecycle {
        match self {
            Self::Alive => ScriptResidentLifecycle::AliveUnloaded,
            Self::Dead => ScriptResidentLifecycle::Dead,
            Self::Released => ScriptResidentLifecycle::Released,
        }
    }
}

/// One durable resident record. `revision` is the storage transaction that last
/// changed it, which is also the fence callers pass as `expected_revision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentRecord {
    pub(super) handle: String,
    pub(super) plugin_id: String,
    pub(super) entity_uuid: String,
    pub(super) generation_id: Option<String>,
    pub(super) disposition: ResidentDisposition,
    pub(super) pois: ScriptResidentPois,
    pub(super) revision: u64,
}

/// One durable spawn-site reservation. A token reserves exactly one home slot
/// for one generation identity and is consumed by a single spawn operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentSite {
    pub(super) token: String,
    pub(super) plugin_id: String,
    pub(super) generation_id: String,
    pub(super) x_bits: u64,
    pub(super) y_bits: u64,
    pub(super) z_bits: u64,
    pub(super) consumed: bool,
    pub(super) released: bool,
    pub(super) revision: u64,
}

impl DurableResidentSite {
    pub(super) fn position(&self) -> Option<Vec3> {
        let position = Vec3::new(
            f64::from_bits(self.x_bits),
            f64::from_bits(self.y_bits),
            f64::from_bits(self.z_bits),
        );
        (position.x.is_finite() && position.y.is_finite() && position.z.is_finite())
            .then_some(position)
    }

    fn consumed(mut self) -> Self {
        self.consumed = true;
        self
    }
}

/// One durable ledger change applied inside a storage frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DurableResidentChange {
    Record { record: Box<DurableResidentRecord> },
    Site { site: Box<DurableResidentSite> },
}

impl DurableResidentChange {
    pub(super) fn revision(&self) -> u64 {
        match self {
            Self::Record { record } => record.revision,
            Self::Site { site } => site.revision,
        }
    }

    fn set_revision(&mut self, revision: u64) {
        match self {
            Self::Record { record } => record.revision = revision,
            Self::Site { site } => site.revision = revision,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        match self {
            Self::Record { record } => validate_resident_record(record),
            Self::Site { site } => validate_resident_site(site),
        }
    }
}

/// Core-owned resident ledger: one handle per binding, one generation and one
/// entity UUID per binding, plus the outstanding spawn-site reservations.
#[derive(Debug, Default)]
pub(super) struct ResidentLedger {
    records: BTreeMap<String, DurableResidentRecord>,
    by_generation: BTreeMap<String, String>,
    by_entity: BTreeMap<String, String>,
    sites: BTreeMap<String, DurableResidentSite>,
    live_capacity: usize,
}

impl ResidentLedger {
    pub(super) fn new() -> Self {
        Self {
            live_capacity: MAX_RESIDENT_LIVE_PER_PLUGIN,
            ..Self::default()
        }
    }

    pub(super) fn record(&self, handle: &str) -> Option<&DurableResidentRecord> {
        self.records.get(handle)
    }

    pub(super) fn record_by_entity(&self, entity_uuid: &str) -> Option<&DurableResidentRecord> {
        let handle = self.by_entity.get(entity_uuid)?;
        self.records.get(handle)
    }

    pub(super) fn record_by_generation(
        &self,
        generation_id: &str,
    ) -> Option<&DurableResidentRecord> {
        let handle = self.by_generation.get(generation_id)?;
        self.records.get(handle)
    }

    pub(super) fn site(&self, token: &str) -> Option<&DurableResidentSite> {
        self.sites.get(token)
    }

    pub(super) fn live_capacity(&self) -> usize {
        self.live_capacity
    }

    pub(super) fn live_count(&self, plugin_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| {
                record.plugin_id == plugin_id && record.disposition == ResidentDisposition::Alive
            })
            .count()
    }

    fn record_count(&self, plugin_id: &str) -> usize {
        self.records
            .values()
            .filter(|record| record.plugin_id == plugin_id)
            .count()
    }

    pub(super) fn handles_for_plugin(&self, plugin_id: &str) -> Vec<String> {
        let mut handles = self
            .records
            .values()
            .filter(|record| record.plugin_id == plugin_id)
            .map(|record| record.handle.clone())
            .collect::<Vec<_>>();
        handles.sort_unstable();
        handles
    }

    fn insert_record(
        &mut self,
        record: DurableResidentRecord,
    ) -> Result<(), PluginStorageStartError> {
        if let Some(previous) = self.records.get(&record.handle) {
            if previous == &record {
                return Ok(());
            }
            if previous.revision >= record.revision {
                return Err(PluginStorageStartError::Malformed("resident revision"));
            }
        } else if self.record_count(&record.plugin_id) >= MAX_RESIDENT_RECORDS_PER_PLUGIN {
            return Err(PluginStorageStartError::LiveQuotaExceeded);
        }
        self.rebind(&record);
        self.records.insert(record.handle.clone(), record);
        Ok(())
    }

    /// Keep the generation and entity indices at one binding each. A released
    /// record frees its index entry so an explicitly released NPC can be
    /// adopted again; a dead tombstone keeps it and blocks a silent rebind.
    fn rebind(&mut self, record: &DurableResidentRecord) {
        if let Some(generation) = record.generation_id.clone() {
            bind_index(
                &mut self.by_generation,
                &self.records,
                &generation,
                &record.handle,
            );
        }
        let entity_uuid = record.entity_uuid.clone();
        bind_index(
            &mut self.by_entity,
            &self.records,
            &entity_uuid,
            &record.handle,
        );
    }

    fn insert_site(&mut self, site: DurableResidentSite) -> Result<(), PluginStorageStartError> {
        if let Some(previous) = self.sites.get(&site.token) {
            if previous == &site {
                return Ok(());
            }
            if previous.revision >= site.revision || previous.plugin_id != site.plugin_id {
                return Err(PluginStorageStartError::Malformed("resident site revision"));
            }
        }
        self.sites.insert(site.token.clone(), site);
        Ok(())
    }

    pub(super) fn apply(
        &mut self,
        change: &DurableResidentChange,
    ) -> Result<(), PluginStorageStartError> {
        match change {
            DurableResidentChange::Record { record } => {
                validate_resident_record(record)?;
                self.insert_record(record.as_ref().clone())
            }
            DurableResidentChange::Site { site } => {
                validate_resident_site(site)?;
                self.insert_site(site.as_ref().clone())
            }
        }
    }

    pub(super) fn reset(&mut self) {
        self.records.clear();
        self.by_generation.clear();
        self.by_entity.clear();
        self.sites.clear();
    }

    pub(super) fn change_log(&self) -> Vec<DurableResidentChange> {
        self.records
            .values()
            .map(|record| DurableResidentChange::Record {
                record: Box::new(record.clone()),
            })
            .chain(self.sites.values().map(|site| DurableResidentChange::Site {
                site: Box::new(site.clone()),
            }))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn set_live_capacity_for_test(&mut self, capacity: usize) {
        self.live_capacity = capacity;
    }
}

/// Point one index key at one handle. A different existing binding keeps the
/// key unless that record was explicitly released.
fn bind_index(
    index: &mut BTreeMap<String, String>,
    records: &BTreeMap<String, DurableResidentRecord>,
    key: &str,
    handle: &str,
) {
    match index.get(key) {
        Some(current) if current == handle => {}
        Some(current) => {
            let released = records
                .get(current)
                .is_some_and(|previous| previous.disposition == ResidentDisposition::Released);
            if released {
                index.insert(key.to_owned(), handle.to_owned());
            }
        }
        None => {
            index.insert(key.to_owned(), handle.to_owned());
        }
    }
}

pub(super) fn validate_resident_record(
    record: &DurableResidentRecord,
) -> Result<(), PluginStorageStartError> {
    mc_script::validate_resident_handle(&record.handle)
        .map_err(|_| PluginStorageStartError::Malformed("resident handle"))?;
    validate_ledger_field(&record.plugin_id)?;
    mc_script::validate_entity_uuid(&record.entity_uuid)
        .map_err(|_| PluginStorageStartError::Malformed("resident entity"))?;
    if let Some(generation) = &record.generation_id {
        mc_script::validate_generation_id(generation)
            .map_err(|_| PluginStorageStartError::Malformed("resident generation"))?;
    }
    record
        .pois
        .validate()
        .map_err(|_| PluginStorageStartError::Malformed("resident pois"))?;
    if record.revision == 0 || record.revision > mc_script::MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("resident revision"));
    }
    Ok(())
}

pub(super) fn validate_resident_site(
    site: &DurableResidentSite,
) -> Result<(), PluginStorageStartError> {
    mc_script::validate_spawn_site_token(&site.token)
        .map_err(|_| PluginStorageStartError::Malformed("resident site token"))?;
    validate_ledger_field(&site.plugin_id)?;
    mc_script::validate_generation_id(&site.generation_id)
        .map_err(|_| PluginStorageStartError::Malformed("resident site generation"))?;
    if site.position().is_none() {
        return Err(PluginStorageStartError::Malformed("resident site position"));
    }
    if site.revision == 0 || site.revision > mc_script::MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("resident site revision"));
    }
    Ok(())
}

fn validate_ledger_field(value: &str) -> Result<(), PluginStorageStartError> {
    if value.is_empty() || value.len() > mc_script::MAX_PLUGIN_ID_BYTES {
        return Err(PluginStorageStartError::Malformed("resident owner"));
    }
    Ok(())
}

pub(super) fn decode_resident_change(
    payload: &[u8],
) -> Result<DurableResidentChange, PluginStorageStartError> {
    let change: DurableResidentChange = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("resident change"))?;
    change.validate()?;
    Ok(change)
}

impl PluginStorage {
    pub(super) fn residents(&self) -> &ResidentLedger {
        &self.residents
    }

    /// Prepare one resident receipt inside the plugin operation envelope. The
    /// durable ledger changes ride in the same frame as the receipt, so a
    /// replay either restores both or neither.
    fn prepare_resident_operation_batch(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        mut changes: Vec<DurableResidentChange>,
    ) -> Result<ScriptStoragePrepareOutcome<PreparedStorageBatch>, PluginStorageMutationError> {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        // The committed revision is the durable fence the caller must send back,
        // so the payload carries it and not the pre-commit placeholder.
        let mut payload = payload;
        if let ScriptOperationPayload::Resident { result } = &mut payload
            && let ScriptResidentResult::Snapshot { resident } = &mut **result
        {
            resident.revision = transaction_id;
        }
        let outcome = ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        let receipt = DurableOperationReceipt {
            plugin_id: plugin_id.to_owned(),
            operation_id: request
                .operation_id()
                .expect("resident mutation decision identity")
                .to_owned(),
            request_id: request.request_id().to_owned(),
            fingerprint: owned_inventory_fingerprint(request.operation()),
            revision: transaction_id,
            outcome,
            delivered: false,
        };
        Ok(ScriptStoragePrepareOutcome::Prepared(
            PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: Some(receipt),
                resident: changes,
                settlement: Vec::new(),
                order: Vec::new(),
            },
        ))
    }

    /// Apply one core-owned ledger change without a plugin operation, for
    /// worldgen materialisation and observed deaths. Revisions stay strictly
    /// monotonic with the journal.
    fn append_resident_change(
        &mut self,
        mut change: DurableResidentChange,
    ) -> Result<DurableResidentChange, PluginStorageMutationError> {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        change.set_revision(transaction_id);
        change
            .validate()
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        let payload = serde_json::to_vec(&change)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        let mut frame_payload = Vec::with_capacity(payload.len() + 1);
        frame_payload.push(super::OP_RESIDENT_CHANGE);
        frame_payload.extend_from_slice(&payload);
        if frame_payload.len() > super::MAX_FRAME_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        let frame = super::frame(&frame_payload);
        self.compact_before_append_if_needed(frame.len())?;
        self.append_frame(&frame, false)?;
        self.residents
            .apply(&change)
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        self.revision = transaction_id;
        Ok(change)
    }

    /// Append one core-observed death tombstone without a plugin operation.
    pub(super) fn observe_resident_death(
        &mut self,
        record: &DurableResidentRecord,
    ) -> Result<DurableResidentRecord, PluginStorageMutationError> {
        let mut dead = record.clone();
        dead.disposition = ResidentDisposition::Dead;
        dead.revision = self.revision.saturating_add(1);
        self.append_resident_change(DurableResidentChange::Record {
            record: Box::new(dead),
        })?;
        Ok(self
            .residents
            .record(&record.handle)
            .cloned()
            .expect("observed resident death stays bound"))
    }

    #[cfg(test)]
    pub(super) fn resident_live_capacity_for_test(&mut self, capacity: usize) {
        self.residents.set_live_capacity_for_test(capacity);
    }
}

/// Rebuild one bounded snapshot from the durable record and, when the entity is
/// resident in the loaded owner, its live state.
fn resident_snapshot(
    record: &DurableResidentRecord,
    live: Option<&EntitySnapshot>,
    items: &ItemRegistry,
) -> ScriptResidentSnapshot {
    let loaded = live
        .filter(|snapshot| resident_entity_is_current(snapshot))
        .and_then(|snapshot| resident_loaded_state(snapshot, items));
    let lifecycle = match record.disposition {
        ResidentDisposition::Alive if loaded.is_some() => ScriptResidentLifecycle::AliveLoaded,
        disposition => disposition.persisted_lifecycle(),
    };
    // The closed schema exposes live state only for `alive_loaded`; a released or
    // dead tombstone keeps its identity without a live pose.
    let loaded = (lifecycle == ScriptResidentLifecycle::AliveLoaded)
        .then_some(loaded)
        .flatten();
    ScriptResidentSnapshot::new(
        record.handle.clone(),
        record.entity_uuid.clone(),
        lifecycle,
        record.revision,
        record.generation_id.clone(),
        record.pois.clone(),
        loaded,
    )
}

fn resident_entity_is_current(snapshot: &EntitySnapshot) -> bool {
    snapshot.lifecycle == EntityLifecycle::Alive && snapshot.type_name == "minecraft:villager"
}

/// A live entity that no longer is the bound villager is a conversion or death:
/// the record keeps its identity and becomes a tombstone.
fn resident_death_observed(record: &DurableResidentRecord, live: Option<&EntitySnapshot>) -> bool {
    record.disposition == ResidentDisposition::Alive
        && live.is_some_and(|snapshot| !resident_entity_is_current(snapshot))
}

fn resident_loaded_state(
    snapshot: &EntitySnapshot,
    items: &ItemRegistry,
) -> Option<ScriptResidentLoadedState> {
    let position = ScriptPosition::try_new(
        snapshot.position.x,
        snapshot.position.y,
        snapshot.position.z,
    )?;
    let carried = snapshot
        .item_stack
        .iter()
        .filter_map(|stack| {
            let name = items.name_of(stack.item_id)?;
            u32::try_from(stack.count)
                .ok()
                .filter(|count| *count > 0)
                .map(|count| ScriptResidentItemSummary::new(name.to_string(), count))
        })
        .take(MAX_RESIDENT_CARRIED_ITEMS)
        .collect();
    Some(ScriptResidentLoadedState::new(
        position,
        snapshot.health,
        carried,
    ))
}

fn replay_resident_operation(
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

fn rejected(failure: ScriptOperationFailure) -> ScriptOperationOutcome {
    ScriptOperationOutcome::rejected(failure)
}

fn resident_distance(left: Vec3, right: Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

impl super::InventoryRuntime {
    /// Execute one durable resident request. Queries answer from the live entity
    /// owner; every mutation commits its receipt together with the ledger
    /// change it decided.
    pub(crate) async fn execute_resident_operation(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::Resident { operation } = request.operation() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        match operation {
            ScriptResidentOperation::Query { handles, cursor } => {
                self.query_residents(storage, plugin_id, handles, cursor.as_deref())
                    .await
            }
            _ => {
                let Some(operation_id) = operation.operation_id() else {
                    return Ok(rejected(ScriptOperationFailure::InvalidRequest));
                };
                if let Some(outcome) =
                    replay_resident_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                self.mutate_resident(storage, plugin_id, request).await
            }
        }
    }

    /// Idempotent worldgen/site materialisation. The same generation id always
    /// resolves to the same entity UUID, so re-materialising a chunk adopts the
    /// existing resident instead of spawning a second one.
    ///
    /// This is the worldgen bootstrap seam the settlement catalog (C2) calls; it
    /// has no in-tree caller until that lands.
    #[allow(dead_code)]
    pub(crate) async fn materialize_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        generation_id: &str,
        position: Vec3,
    ) -> Result<ScriptResidentSnapshot, ScriptOperationFailure> {
        if mc_script::validate_generation_id(generation_id).is_err() {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        if let Some(record) = storage
            .residents()
            .record_by_generation(generation_id)
            .cloned()
        {
            if record.plugin_id != plugin_id {
                return Err(ScriptOperationFailure::Forbidden);
            }
            let live = self
                .resident_live(std::slice::from_ref(&record.entity_uuid))
                .await;
            return Ok(resident_snapshot(
                &record,
                live.first().and_then(Option::as_ref),
                self.items(),
            ));
        }
        let uuid = Uuid::from_bytes(
            resident_entity_uuid(generation_id)
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?,
        );
        let Some(snapshot) = self.materialize_entity(uuid, position).await else {
            return Err(ScriptOperationFailure::RuntimeUnavailable);
        };
        let handle = resident_handle_for_generation(plugin_id, generation_id)
            .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        storage
            .append_resident_change(DurableResidentChange::Record {
                record: Box::new(DurableResidentRecord {
                    handle: handle.clone(),
                    plugin_id: plugin_id.to_owned(),
                    entity_uuid: uuid.to_string(),
                    generation_id: Some(generation_id.to_owned()),
                    disposition: ResidentDisposition::Alive,
                    pois: ScriptResidentPois::default(),
                    revision: 0,
                }),
            })
            .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
        let record = storage
            .residents()
            .record(&handle)
            .cloned()
            .expect("bootstrapped resident stays bound");
        Ok(resident_snapshot(&record, Some(&snapshot), self.items()))
    }

    /// Reserve one durable spawn site for a core-side caller. The plugin only
    /// ever sees the opaque token.
    ///
    /// The settlement runtime reserves one site per home point of interest; the
    /// reservation is consumed by the plugin's spawn operation, never by core.
    pub(crate) fn reserve_resident_site(
        storage: &mut PluginStorage,
        plugin_id: &str,
        generation_id: &str,
        position: Vec3,
    ) -> Result<String, ScriptOperationFailure> {
        let token = mc_script::resident_spawn_site_token(plugin_id, generation_id)
            .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        if let Some(site) = storage.residents().site(&token)
            && (site.plugin_id != plugin_id || site.consumed || site.released)
        {
            return Err(ScriptOperationFailure::Capacity);
        }
        storage
            .append_resident_change(DurableResidentChange::Site {
                site: Box::new(DurableResidentSite {
                    token: token.clone(),
                    plugin_id: plugin_id.to_owned(),
                    generation_id: generation_id.to_owned(),
                    x_bits: position.x.to_bits(),
                    y_bits: position.y.to_bits(),
                    z_bits: position.z.to_bits(),
                    consumed: false,
                    released: false,
                    revision: 0,
                }),
            })
            .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)?;
        Ok(token)
    }

    async fn query_residents(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        handles: &[String],
        cursor: Option<&str>,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let (handles, next_cursor) = if handles.is_empty() {
            let all = storage.residents().handles_for_plugin(plugin_id);
            let start = match cursor {
                Some(cursor) => match all.binary_search(&cursor.to_owned()) {
                    Ok(index) => index + 1,
                    Err(_) => return Ok(rejected(ScriptOperationFailure::CursorExpired)),
                },
                None => 0,
            };
            let remaining = all.into_iter().skip(start).collect::<Vec<_>>();
            let truncated = remaining.len() > MAX_RESIDENT_PAGE;
            let page = remaining
                .into_iter()
                .take(MAX_RESIDENT_PAGE)
                .collect::<Vec<_>>();
            let next = truncated.then(|| page.last().cloned()).flatten();
            (page, next)
        } else {
            for handle in handles {
                let Some(record) = storage.residents().record(handle) else {
                    return Ok(rejected(ScriptOperationFailure::NotFound));
                };
                if record.plugin_id != plugin_id {
                    return Ok(rejected(ScriptOperationFailure::Forbidden));
                }
            }
            (handles.to_vec(), None)
        };

        let entity_uuids = handles
            .iter()
            .filter_map(|handle| storage.residents().record(handle))
            .map(|record| record.entity_uuid.clone())
            .collect::<Vec<_>>();
        let live = self.resident_live(&entity_uuids).await;

        let mut residents = Vec::with_capacity(handles.len());
        let mut deaths = Vec::new();
        let mut revision = 0_u64;
        for (index, handle) in handles.iter().enumerate() {
            let Some(record) = storage.residents().record(handle).cloned() else {
                continue;
            };
            let live = live.get(index).and_then(Option::as_ref);
            if resident_death_observed(&record, live) {
                deaths.push(record);
            }
        }
        for record in deaths {
            let dead = storage.observe_resident_death(&record)?;
            revision = revision.max(dead.revision);
        }
        // Snapshots are built after the tombstone is durable so the caller never
        // sees the pre-observation lifecycle.
        for (index, handle) in handles.iter().enumerate() {
            let Some(record) = storage.residents().record(handle).cloned() else {
                continue;
            };
            revision = revision.max(record.revision);
            residents.push(resident_snapshot(
                &record,
                live.get(index).and_then(Option::as_ref),
                self.items(),
            ));
        }
        let result = ScriptResidentResult::Page {
            residents,
            cursor: next_cursor,
        };
        Ok(ScriptOperationOutcome::committed(
            revision,
            ScriptOperationPayload::Resident {
                result: Box::new(result),
            },
        )
        .expect("core-owned resident page outcome is canonical"))
    }

    async fn mutate_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::Resident { operation } = request.operation() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        match operation {
            ScriptResidentOperation::Claim {
                actor_id,
                entity_uuid,
                expected_entity_revision,
                ..
            } => {
                self.claim_resident(
                    storage,
                    plugin_id,
                    request,
                    *actor_id,
                    entity_uuid,
                    *expected_entity_revision,
                )
                .await
            }
            ScriptResidentOperation::Spawn {
                spawn_site_token, ..
            } => {
                self.spawn_resident(storage, plugin_id, request, spawn_site_token)
                    .await
            }
            ScriptResidentOperation::Release {
                handle,
                expected_revision,
                ..
            } => {
                self.release_resident(storage, plugin_id, request, handle, *expected_revision)
                    .await
            }
            ScriptResidentOperation::SetPois {
                handle,
                home_poi,
                work_poi,
                meeting_poi,
                expected_revision,
                ..
            } => {
                self.set_resident_pois(
                    storage,
                    plugin_id,
                    request,
                    handle,
                    ScriptResidentPois::new(
                        home_poi.clone(),
                        work_poi.clone(),
                        meeting_poi.clone(),
                    ),
                    *expected_revision,
                )
                .await
            }
            ScriptResidentOperation::Query { .. } => {
                Ok(rejected(ScriptOperationFailure::InvalidRequest))
            }
            _ => Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        }
    }

    async fn claim_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        actor_id: u64,
        entity_uuid: &str,
        expected_entity_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        if let Some(record) = storage.residents().record_by_entity(entity_uuid).cloned() {
            if record.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            if record.revision != expected_entity_revision {
                return Ok(rejected(ScriptOperationFailure::StaleRevision));
            }
            let live = self
                .resident_live(std::slice::from_ref(&record.entity_uuid))
                .await;
            let live = live.first().and_then(Option::as_ref);
            let record = if resident_death_observed(&record, live) {
                storage.observe_resident_death(&record)?
            } else {
                record
            };
            return self.commit_resident(
                storage,
                plugin_id,
                request,
                Vec::new(),
                resident_snapshot(&record, live, self.items()),
            );
        }
        if expected_entity_revision != 0 {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let live = self.resident_live(&[entity_uuid.to_owned()]).await;
        let Some(snapshot) = live.first().and_then(Option::as_ref) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if !resident_entity_is_current(snapshot) {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let Some(actor) = self.sessions().resident_actor_position(actor_id) else {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        };
        if resident_distance(actor, snapshot.position) > MAX_RESIDENT_CLAIM_DISTANCE {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if storage.residents().live_count(plugin_id) >= storage.residents().live_capacity() {
            return Ok(rejected(ScriptOperationFailure::Capacity));
        }
        let handle = match resident_handle_for_entity(plugin_id, entity_uuid) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let record = DurableResidentRecord {
            handle,
            plugin_id: plugin_id.to_owned(),
            entity_uuid: entity_uuid.to_owned(),
            generation_id: None,
            disposition: ResidentDisposition::Alive,
            pois: ScriptResidentPois::default(),
            revision: 0,
        };
        let snapshot = resident_snapshot(&record, Some(snapshot), self.items());
        self.commit_resident(
            storage,
            plugin_id,
            request,
            vec![DurableResidentChange::Record {
                record: Box::new(record),
            }],
            snapshot,
        )
    }

    async fn spawn_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        spawn_site_token: &str,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(site) = storage.residents().site(spawn_site_token).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if site.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if site.released {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        if site.consumed {
            return Ok(rejected(ScriptOperationFailure::Capacity));
        }
        let Some(position) = site.position() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        if let Some(record) = storage
            .residents()
            .record_by_generation(&site.generation_id)
            .cloned()
        {
            if record.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            let live = self
                .resident_live(std::slice::from_ref(&record.entity_uuid))
                .await;
            let snapshot =
                resident_snapshot(&record, live.first().and_then(Option::as_ref), self.items());
            return self.commit_resident(
                storage,
                plugin_id,
                request,
                vec![DurableResidentChange::Site {
                    site: Box::new(site.consumed()),
                }],
                snapshot,
            );
        }
        if storage.residents().live_count(plugin_id) >= storage.residents().live_capacity() {
            return Ok(rejected(ScriptOperationFailure::Capacity));
        }
        let uuid = match resident_entity_uuid(&site.generation_id) {
            Ok(uuid) => Uuid::from_bytes(uuid),
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        // The resident is created at the reserved point of interest, so that
        // cell has to be a body space: floor below, feet and head clear. A cell
        // the world cannot confirm, or one left under the terrain (a slope the
        // building's anchor row does not clear), refuses the spawn instead of
        // materialising a buried resident. A reservation that already owns a
        // record replays above, so the world never blocks a legitimate replay.
        let cell = [
            position.x.floor() as i32,
            position.y.floor() as i32,
            position.z.floor() as i32,
        ];
        let Some(world) = self.resident_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        match world.standable(RESIDENT_WORLD_DIMENSION, cell) {
            Some(true) => {}
            Some(false) => return Ok(rejected(ScriptOperationFailure::Blocked)),
            None => return Ok(rejected(ScriptOperationFailure::Unloaded)),
        }
        let Some(snapshot) = self.materialize_entity(uuid, position).await else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let handle = match resident_handle_for_generation(plugin_id, &site.generation_id) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let record = DurableResidentRecord {
            handle,
            plugin_id: plugin_id.to_owned(),
            entity_uuid: uuid.to_string(),
            generation_id: Some(site.generation_id.clone()),
            disposition: ResidentDisposition::Alive,
            pois: ScriptResidentPois::default(),
            revision: 0,
        };
        let snapshot = resident_snapshot(&record, Some(&snapshot), self.items());
        self.commit_resident(
            storage,
            plugin_id,
            request,
            vec![
                DurableResidentChange::Record {
                    record: Box::new(record),
                },
                DurableResidentChange::Site {
                    site: Box::new(site.consumed()),
                },
            ],
            snapshot,
        )
    }

    async fn release_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.residents().record(handle).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let mut released = record.clone();
        released.disposition = ResidentDisposition::Released;
        let live = self
            .resident_live(std::slice::from_ref(&record.entity_uuid))
            .await;
        let snapshot = resident_snapshot(
            &released,
            live.first().and_then(Option::as_ref),
            self.items(),
        );
        self.commit_resident(
            storage,
            plugin_id,
            request,
            vec![DurableResidentChange::Record {
                record: Box::new(released),
            }],
            snapshot,
        )
    }

    async fn set_resident_pois(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        handle: &str,
        pois: ScriptResidentPois,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.residents().record(handle).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if record.disposition != ResidentDisposition::Alive {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        let mut next = record.clone();
        next.pois = pois;
        let live = self
            .resident_live(std::slice::from_ref(&record.entity_uuid))
            .await;
        let snapshot =
            resident_snapshot(&next, live.first().and_then(Option::as_ref), self.items());
        self.commit_resident(
            storage,
            plugin_id,
            request,
            vec![DurableResidentChange::Record {
                record: Box::new(next),
            }],
            snapshot,
        )
    }

    fn commit_resident(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        changes: Vec<DurableResidentChange>,
        snapshot: ScriptResidentSnapshot,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let payload = ScriptOperationPayload::Resident {
            result: Box::new(ScriptResidentResult::Snapshot { resident: snapshot }),
        };
        let batch =
            match storage.prepare_resident_operation_batch(plugin_id, request, payload, changes)? {
                ScriptStoragePrepareOutcome::Prepared(batch) => batch,
                ScriptStoragePrepareOutcome::Rejected => {
                    return Ok(rejected(ScriptOperationFailure::Capacity));
                }
            };
        storage.commit_batch(batch)?;
        let operation_id = request
            .operation_id()
            .expect("resident mutation decision identity");
        Ok(storage
            .operation_receipt(plugin_id, operation_id)
            .expect("committed resident receipt remains installed")
            .outcome
            .clone())
    }

    /// One bounded owner query per call: at most the UUIDs the caller named.
    async fn resident_live(&self, entity_uuids: &[String]) -> Vec<Option<EntitySnapshot>> {
        let parsed = entity_uuids
            .iter()
            .map(|uuid| Uuid::parse_str(uuid).ok())
            .collect::<Vec<_>>();
        let resolvable = parsed.iter().flatten().copied().collect::<Vec<_>>();
        if resolvable.is_empty() {
            return parsed.iter().map(|_| None).collect();
        }
        let resolved = self.sessions().resident_entity_snapshots(&resolvable).await;
        let mut resolved = resolved.into_iter();
        parsed
            .iter()
            .map(|uuid| {
                if uuid.is_some() {
                    resolved.next().flatten()
                } else {
                    None
                }
            })
            .collect()
    }

    /// Spawn the deterministic villager once, or adopt the entity that already
    /// carries this UUID.
    async fn materialize_entity(&self, uuid: Uuid, position: Vec3) -> Option<EntitySnapshot> {
        if let Some(snapshot) = self
            .sessions()
            .resident_entity_snapshots(&[uuid])
            .await
            .into_iter()
            .next()
            .flatten()
        {
            return Some(snapshot);
        }
        let mut entity = SpawnEntity::new(139, "minecraft:villager", position);
        entity.uuid = Some(uuid);
        self.sessions().spawn_resident_entity(entity).await.ok()
    }
}
