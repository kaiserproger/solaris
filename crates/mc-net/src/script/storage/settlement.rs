//! Durable settlement sites, surveys, and staged construction.
//!
//! This module owns the core authority of the settlement operations: site
//! discovery and query over the deterministic worldgen selector, resident site
//! reservations against a site point of interest, bounded terrain surveys, and
//! the prepare/advance/pause/cancel lifecycle of a staged structure. Nothing
//! here trusts a plugin: a site id is resolved through the selector (a forged id
//! has no cell), every structure is owner scoped, and every committed portion is
//! checked against the durable C1 reservation before it is applied.
//!
//! Durable state lives in the plugin storage journal next to the operation
//! receipts that make every mutation idempotent, so a replayed journal rebuilds
//! the exact same ledger. Authoritative terrain is reached only through
//! [`SettlementWorld`]; when no adapter is installed the operations that need
//! terrain answer `runtime_unavailable` instead of guessing.

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;

use mc_data::ItemStack;
use mc_entity::Vec3;
use mc_script::{
    MAX_BLUEPRINT_FOOTPRINT_AXIS, MAX_BLUEPRINT_ID_BYTES, MAX_SCRIPT_WORLD_TIME,
    MAX_SETTLEMENT_POIS, MAX_SETTLEMENT_RESIDENTS, MAX_SETTLEMENT_SITE_PAGE,
    MAX_STRUCTURE_ACTIVE_PER_PLUGIN, MAX_STRUCTURE_ID_BYTES, MAX_STRUCTURE_RESOURCE_TYPES,
    MAX_STRUCTURE_STAGES, MAX_SURVEY_TOKEN_BYTES, MAX_WAREHOUSE_HANDLE_BYTES,
    MAX_WORLD_COMMIT_PORTION, ScriptChunkAvailability, ScriptInventoryEndpoint,
    ScriptInventoryMaterial, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptResidentSiteReservation, ScriptSettlementBuilding,
    ScriptSettlementOperation, ScriptSettlementPoi, ScriptSettlementResult, ScriptSettlementSite,
    ScriptSettlementSitePage, ScriptSitePoiKind, ScriptSitePoiState, ScriptSiteVariant,
    ScriptStructureMaterial, ScriptStructureReceipt, ScriptStructureSnapshot,
    ScriptStructureStagePlan, ScriptStructureState, ScriptSurveyBounds, ScriptSurveySnapshot,
    ScriptWarehouseBinding, resident_generation_id, warehouse_handle,
};
use mc_worldgen::{
    BlockEntitySeedKind, BlueprintBlock, BlueprintCatalog, BlueprintInstance, PoiKind, QuarterTurn,
    SITE_CELL_BLOCKS, SettlementSelector, SiteCandidate, SiteLayout, SiteVariant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    DurableOperationReceipt, PluginStorage, PluginStorageMutationError, PluginStorageStartError,
    PreparedStorageBatch, ScriptStoragePrepareOutcome,
};
use crate::play::owned_inventory::{owned_inventory_fingerprint, resource_plan_hash};

/// Storage transactions one survey token stays valid for.
const SURVEY_TOKEN_TTL_TRANSACTIONS: u64 = 64;
/// Domain separator for every deterministic settlement identity.
const SETTLEMENT_DOMAIN: &[u8] = b"solaris.settlement.v1";
/// Stage name of a blueprint that authors no construction stages.
const BODY_STAGE: &str = "body";
/// Pause reason recorded when the reserved footprint changed under a structure.
const PAUSE_SITE_CHANGED: &str = "site_changed";

/// One point-of-interest reservation inside a settlement site.
///
/// The resident ledger is the authority for `consumed`; the durable bit mirrors
/// the observation so a site snapshot stays truthful after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurablePoiReservation {
    token: String,
    plugin_id: String,
    consumed: bool,
    released: bool,
}

/// Durable reservation state of one settlement site. Reservations are global
/// world state: a home occupied by one plugin's villager is not free for
/// another, so entries are keyed by point of interest and carry their owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableSiteState {
    site_id: String,
    pois: BTreeMap<String, DurablePoiReservation>,
    revision: u64,
}

impl DurableSiteState {
    fn new(site_id: String) -> Self {
        Self {
            site_id,
            pois: BTreeMap::new(),
            revision: 0,
        }
    }

    fn reservation(&self, poi_id: &str) -> Option<&DurablePoiReservation> {
        self.pois.get(poi_id)
    }
}

/// Durable state of one staged structure.
///
/// `stages` and `reserved_footprint` are persisted with the plan so a restart
/// reconstructs the exact snapshot without re-deriving the catalog, and
/// `blueprint_hash` pins the authored blueprint the plan was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableStructure {
    structure_id: String,
    plugin_id: String,
    site_id: String,
    blueprint_id: String,
    blueprint_hash: String,
    origin: [i32; 3],
    rotation: u16,
    reserved_footprint: [i32; 3],
    state: ScriptStructureState,
    stages: Vec<ScriptStructureStagePlan>,
    stage_index: usize,
    watermark: u64,
    built_blocks: u64,
    reservation_ref: Option<String>,
    resource_plan_hash: String,
    consumed: BTreeMap<String, u64>,
    prepare_revision: u64,
    pause_reason: Option<String>,
    revision: u64,
}

impl DurableStructure {
    fn reservation_ref(&self) -> Option<&str> {
        self.reservation_ref.as_deref()
    }

    fn is_active(&self) -> bool {
        matches!(
            self.state,
            ScriptStructureState::Prepared
                | ScriptStructureState::Running
                | ScriptStructureState::Paused
        )
    }

    /// Whether this structure is still placed in the world.
    ///
    /// Every state but `Cancelled` is placed: a completed structure keeps the
    /// ground it was built on and its authored containers are the warehouses
    /// the settlement profile exists to expose, so warehouse handles must
    /// survive construction and outlive it. Only a cancelled (removed)
    /// structure stops being warehouse-addressable; a structure that never
    /// existed is refused before this predicate is reached.
    fn is_placed(&self) -> bool {
        !matches!(self.state, ScriptStructureState::Cancelled)
    }

    fn stage(&self) -> Option<&ScriptStructureStagePlan> {
        self.stages.get(self.stage_index)
    }

    /// Reserved territory of the placed blueprint, in world coordinates.
    fn bounds(&self) -> ScriptSurveyBounds {
        bounds_of(self.origin, self.reserved_footprint)
    }
}

/// Owner, committed reservation and revision of one prepared structure, resolved
/// for the C4 resident builder path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StructureBuilderTarget {
    pub(super) plugin_id: String,
    pub(super) reservation_ref: String,
    pub(super) revision: u64,
}

/// One durable, bounded survey authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableSurveyToken {
    token: String,
    plugin_id: String,
    dimension: String,
    bounds: ScriptSurveyBounds,
    world_revision: u64,
    expires_transaction: u64,
    revision: u64,
}

/// One durable, owner-bound warehouse handle.
///
/// A binding grants exactly one plugin read access to one authored container of
/// one of its structures. `revision` is the storage transaction that minted the
/// binding, which is also the fence a warehouse endpoint reports until the
/// container itself becomes writable. `handle` is opaque and is the only
/// identity a plugin ever names back; the structure and container identities
/// stay owner-scoped here so a handle from another plugin resolves to a typed
/// refusal rather than to a container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableWarehouseBinding {
    handle: String,
    plugin_id: String,
    structure_id: String,
    container_id: u32,
    revision: u64,
}

impl DurableWarehouseBinding {
    fn snapshot(&self) -> ScriptWarehouseBinding {
        ScriptWarehouseBinding::new(
            self.handle.clone(),
            self.structure_id.clone(),
            self.container_id,
            self.revision,
        )
    }
}

/// One durable ledger change applied inside a storage frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DurableSettlementChange {
    Site {
        site: Box<DurableSiteState>,
    },
    Structure {
        structure: Box<DurableStructure>,
    },
    Survey {
        survey: Box<DurableSurveyToken>,
    },
    Warehouse {
        binding: Box<DurableWarehouseBinding>,
    },
}

impl DurableSettlementChange {
    pub(super) fn revision(&self) -> u64 {
        match self {
            Self::Site { site } => site.revision,
            Self::Structure { structure } => structure.revision,
            Self::Survey { survey } => survey.revision,
            Self::Warehouse { binding } => binding.revision,
        }
    }

    fn set_revision(&mut self, revision: u64) {
        match self {
            Self::Site { site } => site.revision = revision,
            Self::Structure { structure } => structure.revision = revision,
            Self::Survey { survey } => survey.revision = revision,
            Self::Warehouse { binding } => binding.revision = revision,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        match self {
            Self::Site { site } => validate_site_state(site),
            Self::Structure { structure } => validate_structure(structure),
            Self::Survey { survey } => validate_survey_token(survey),
            Self::Warehouse { binding } => validate_warehouse_binding(binding),
        }
    }
}

/// Core-owned settlement ledger, replayed from the journal on open.
#[derive(Debug, Default)]
pub(super) struct SettlementLedger {
    sites: BTreeMap<String, DurableSiteState>,
    structures: BTreeMap<String, DurableStructure>,
    surveys: BTreeMap<String, DurableSurveyToken>,
    warehouses: BTreeMap<String, DurableWarehouseBinding>,
}

impl SettlementLedger {
    pub(super) fn site(&self, site_id: &str) -> Option<&DurableSiteState> {
        self.sites.get(site_id)
    }

    pub(super) fn structure(&self, structure_id: &str) -> Option<&DurableStructure> {
        self.structures.get(structure_id)
    }

    pub(super) fn survey(&self, token: &str) -> Option<&DurableSurveyToken> {
        self.surveys.get(token)
    }

    /// The binding one opaque warehouse handle names, owner included.
    pub(super) fn warehouse(&self, handle: &str) -> Option<&DurableWarehouseBinding> {
        self.warehouses.get(handle)
    }

    /// The binding of one authored container, if any plugin already holds it.
    ///
    /// Container bindings are unique per `(structure, container)`: a second
    /// binding of the same container would hand two handles to one container.
    pub(super) fn warehouse_for_container(
        &self,
        structure_id: &str,
        container_id: u32,
    ) -> Option<&DurableWarehouseBinding> {
        self.warehouses.values().find(|binding| {
            binding.structure_id == structure_id && binding.container_id == container_id
        })
    }

    /// Structures of one plugin that still hold their reserved territory.
    pub(super) fn active_structures(&self, plugin_id: &str) -> Vec<&DurableStructure> {
        let mut active: Vec<&DurableStructure> = self
            .structures
            .values()
            .filter(|structure| structure.plugin_id == plugin_id && structure.is_active())
            .collect();
        active.sort_unstable_by(|left, right| left.structure_id.cmp(&right.structure_id));
        active
    }

    /// The site state and point of interest carrying one spawn-site token.
    fn reservation_by_token(&self, token: &str) -> Option<(&DurableSiteState, &str)> {
        for site in self.sites.values() {
            for (poi_id, reservation) in &site.pois {
                if reservation.token == token {
                    return Some((site, poi_id.as_str()));
                }
            }
        }
        None
    }

    /// Structures of one plugin, active or not: every one of them still owns the
    /// ground it was prepared against, because built blocks are never removed.
    fn structures_of(&self, plugin_id: &str) -> impl Iterator<Item = &DurableStructure> {
        self.structures
            .values()
            .filter(move |structure| structure.plugin_id == plugin_id)
    }

    pub(super) fn apply(
        &mut self,
        change: &DurableSettlementChange,
    ) -> Result<(), PluginStorageStartError> {
        match change {
            DurableSettlementChange::Site { site } => {
                validate_site_state(site)?;
                if let Some(previous) = self.sites.get(&site.site_id) {
                    if previous == site.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= site.revision {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement site revision",
                        ));
                    }
                    // Reservations are never dropped: another plugin's occupied
                    // home must survive any later change to the same site.
                    if previous
                        .pois
                        .keys()
                        .any(|poi_id| !site.pois.contains_key(poi_id))
                    {
                        return Err(PluginStorageStartError::Malformed("settlement poi dropped"));
                    }
                }
                self.sites
                    .insert(site.site_id.clone(), site.as_ref().clone());
            }
            DurableSettlementChange::Structure { structure } => {
                validate_structure(structure)?;
                if let Some(previous) = self.structures.get(&structure.structure_id) {
                    if previous == structure.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= structure.revision
                        || previous.plugin_id != structure.plugin_id
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement structure revision",
                        ));
                    }
                }
                self.structures
                    .insert(structure.structure_id.clone(), structure.as_ref().clone());
            }
            DurableSettlementChange::Survey { survey } => {
                validate_survey_token(survey)?;
                if let Some(previous) = self.surveys.get(&survey.token) {
                    if previous == survey.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= survey.revision
                        || previous.plugin_id != survey.plugin_id
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement survey revision",
                        ));
                    }
                }
                self.surveys
                    .insert(survey.token.clone(), survey.as_ref().clone());
            }
            DurableSettlementChange::Warehouse { binding } => {
                validate_warehouse_binding(binding)?;
                if let Some(previous) = self.warehouses.get(&binding.handle) {
                    if previous == binding.as_ref() {
                        return Ok(());
                    }
                    if previous.revision >= binding.revision
                        || previous.plugin_id != binding.plugin_id
                        || previous.structure_id != binding.structure_id
                        || previous.container_id != binding.container_id
                    {
                        return Err(PluginStorageStartError::Malformed(
                            "settlement warehouse revision",
                        ));
                    }
                }
                // One container carries at most one binding: a journal that
                // binds the same authored container twice is malformed.
                if self.warehouses.values().any(|existing| {
                    existing.handle != binding.handle
                        && existing.structure_id == binding.structure_id
                        && existing.container_id == binding.container_id
                }) {
                    return Err(PluginStorageStartError::Malformed(
                        "settlement warehouse container rebound",
                    ));
                }
                self.warehouses
                    .insert(binding.handle.clone(), binding.as_ref().clone());
            }
        }
        Ok(())
    }

    pub(super) fn reset(&mut self) {
        self.sites.clear();
        self.structures.clear();
        self.surveys.clear();
        self.warehouses.clear();
    }

    pub(super) fn change_log(&self) -> Vec<DurableSettlementChange> {
        self.sites
            .values()
            .map(|site| DurableSettlementChange::Site {
                site: Box::new(site.clone()),
            })
            .chain(
                self.structures
                    .values()
                    .map(|structure| DurableSettlementChange::Structure {
                        structure: Box::new(structure.clone()),
                    }),
            )
            .chain(
                self.surveys
                    .values()
                    .map(|survey| DurableSettlementChange::Survey {
                        survey: Box::new(survey.clone()),
                    }),
            )
            .chain(
                self.warehouses
                    .values()
                    .map(|binding| DurableSettlementChange::Warehouse {
                        binding: Box::new(binding.clone()),
                    }),
            )
            .collect()
    }
}

pub(super) fn decode_settlement_change(
    payload: &[u8],
) -> Result<DurableSettlementChange, PluginStorageStartError> {
    let change: DurableSettlementChange = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("settlement change"))?;
    change.validate()?;
    Ok(change)
}

/// The settlement projection one operation receipt needs to be indexed: the
/// owner and the bound reservation reference.
pub(super) struct SettlementProjection {
    pub(super) plugin_id: String,
    pub(super) reservation_ref: Option<String>,
}

impl PluginStorage {
    pub(super) fn settlements(&self) -> &SettlementLedger {
        &self.settlements
    }

    /// Everything a settlement receipt needs to find the reservation its
    /// structure spends from.
    pub(super) fn settlement_projection(&self, structure_id: &str) -> Option<SettlementProjection> {
        let structure = self.settlements.structure(structure_id)?;
        Some(SettlementProjection {
            plugin_id: structure.plugin_id.clone(),
            reservation_ref: structure.reservation_ref().map(str::to_owned),
        })
    }

    /// Owner, committed reservation and revision of one prepared structure, for
    /// the C4 resident builder path.
    ///
    /// A structure that has not advanced yet holds no reservation reference, so
    /// the one matching C1 reservation is resolved by its immutable resource
    /// plan hash: unbound, or already bound to this exact structure. A structure
    /// without a compatible reservation cannot consume materials and is not a
    /// build target.
    pub(super) fn resident_builder_target(
        &self,
        structure_id: &str,
    ) -> Option<StructureBuilderTarget> {
        let structure = self.settlements.structure(structure_id)?;
        let reservation_ref = match structure.reservation_ref() {
            Some(reference) => reference.to_owned(),
            None => self
                .reservations
                .iter()
                .find_map(|((owner, reference), (_, snapshot))| {
                    (owner == &structure.plugin_id
                        && !snapshot.released
                        && snapshot.resource_plan_hash == structure.resource_plan_hash
                        && snapshot
                            .bound_to
                            .as_deref()
                            .is_none_or(|bound| bound == structure_id))
                    .then(|| reference.clone())
                })?,
        };
        Some(StructureBuilderTarget {
            plugin_id: structure.plugin_id.clone(),
            reservation_ref,
            revision: structure.revision,
        })
    }

    /// Prepare one settlement receipt together with the ledger changes it
    /// decided, so a replay restores both or neither.
    fn prepare_settlement_operation_batch(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        mut payload: ScriptOperationPayload,
        mut changes: Vec<DurableSettlementChange>,
    ) -> Result<ScriptStoragePrepareOutcome<PreparedStorageBatch>, PluginStorageMutationError> {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        stamp_settlement_payload(&mut payload, transaction_id);
        let outcome = ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        Ok(ScriptStoragePrepareOutcome::Prepared(
            PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: Some(DurableOperationReceipt {
                    plugin_id: plugin_id.to_owned(),
                    operation_id: request
                        .operation_id()
                        .expect("settlement mutation decision identity")
                        .to_owned(),
                    request_id: request.request_id().to_owned(),
                    fingerprint: owned_inventory_fingerprint(request.operation()),
                    revision: transaction_id,
                    outcome,
                    delivered: false,
                }),
                resident: Vec::new(),
                settlement: changes,
                order: Vec::new(),
            },
        ))
    }

    /// Durably append settlement ledger changes that carry no plugin operation
    /// receipt: a survey mints its token while answering a query.
    fn prepare_settlement_change_batch(
        &mut self,
        plugin_id: &str,
        mut payload: ScriptOperationPayload,
        mut changes: Vec<DurableSettlementChange>,
    ) -> Result<
        ScriptStoragePrepareOutcome<(PreparedStorageBatch, ScriptOperationOutcome)>,
        PluginStorageMutationError,
    > {
        let transaction_id = self
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        stamp_settlement_payload(&mut payload, transaction_id);
        let outcome = ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        Ok(ScriptStoragePrepareOutcome::Prepared((
            PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: None,
                resident: Vec::new(),
                settlement: changes,
                order: Vec::new(),
            },
            outcome,
        )))
    }
}

/// Stamp the committed transaction onto the snapshot a settlement payload
/// carries, so the fence the caller reads back is its own commit.
fn stamp_settlement_payload(payload: &mut ScriptOperationPayload, transaction_id: u64) {
    let ScriptOperationPayload::Settlement { result } = payload else {
        return;
    };
    match &mut **result {
        ScriptSettlementResult::ResidentSite { reservation } => {
            reservation.revision = transaction_id;
        }
        ScriptSettlementResult::Structure { structure } => {
            structure.revision = transaction_id;
        }
        ScriptSettlementResult::Receipt { receipt } => {
            receipt.revision = transaction_id;
        }
        ScriptSettlementResult::Warehouse { binding } => {
            // A freshly minted binding carries revision 0 and takes its own
            // commit; an already-bound repeat keeps the revision its durable
            // ledger entry has, so the receipt never claims a newer fence than
            // the binding a later read reports.
            if binding.revision == 0 {
                binding.revision = transaction_id;
            }
        }
        _ => {}
    }
}

fn settlement_payload(result: ScriptSettlementResult) -> ScriptOperationPayload {
    ScriptOperationPayload::Settlement {
        result: Box::new(result),
    }
}

fn rejected(failure: ScriptOperationFailure) -> ScriptOperationOutcome {
    ScriptOperationOutcome::rejected(failure)
}

fn settled_outcome(revision: u64, result: ScriptSettlementResult) -> ScriptOperationOutcome {
    match ScriptOperationOutcome::committed(revision, settlement_payload(result)) {
        Ok(outcome) => outcome,
        Err(_) => rejected(ScriptOperationFailure::RuntimeUnavailable),
    }
}

fn validate_axis_bounded(size: [i32; 3], max: i32) -> Result<(), PluginStorageStartError> {
    if size.iter().any(|axis| *axis <= 0 || *axis > max) {
        return Err(PluginStorageStartError::Malformed("settlement footprint"));
    }
    Ok(())
}

fn validate_revision(revision: u64) -> Result<(), PluginStorageStartError> {
    if revision == 0 || revision > MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("settlement revision"));
    }
    Ok(())
}

fn validate_identifier(value: &str, max: usize) -> Result<(), PluginStorageStartError> {
    if value.is_empty()
        || value.len() > max
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
    {
        return Err(PluginStorageStartError::Malformed("settlement id"));
    }
    Ok(())
}

fn validate_hex_hash(value: &str) -> Result<(), PluginStorageStartError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PluginStorageStartError::Malformed("settlement hash"));
    }
    Ok(())
}

fn validate_material_counts(counts: &BTreeMap<String, u64>) -> Result<(), PluginStorageStartError> {
    if counts.len() > MAX_STRUCTURE_RESOURCE_TYPES {
        return Err(PluginStorageStartError::Malformed("settlement materials"));
    }
    for (resource, quantity) in counts {
        validate_identifier(resource, 128)?;
        if !resource.contains(':') || *quantity == 0 || *quantity > MAX_SCRIPT_WORLD_TIME {
            return Err(PluginStorageStartError::Malformed("settlement materials"));
        }
    }
    Ok(())
}

fn validate_site_state(site: &DurableSiteState) -> Result<(), PluginStorageStartError> {
    validate_identifier(&site.site_id, 64)?;
    if site.pois.len() > MAX_SETTLEMENT_POIS || site.revision > MAX_SCRIPT_WORLD_TIME {
        return Err(PluginStorageStartError::Malformed("settlement site"));
    }
    for (poi_id, reservation) in &site.pois {
        validate_identifier(poi_id, 128)?;
        validate_identifier(&reservation.plugin_id, 128)?;
        validate_identifier(&reservation.token, 128)?;
    }
    Ok(())
}

fn validate_structure(structure: &DurableStructure) -> Result<(), PluginStorageStartError> {
    validate_identifier(&structure.structure_id, MAX_STRUCTURE_ID_BYTES)?;
    validate_identifier(&structure.plugin_id, 128)?;
    validate_identifier(&structure.site_id, 64)?;
    validate_identifier(&structure.blueprint_id, MAX_BLUEPRINT_ID_BYTES)?;
    validate_hex_hash(&structure.blueprint_hash)?;
    validate_hex_hash(&structure.resource_plan_hash)?;
    if let Some(reference) = structure.reservation_ref() {
        validate_identifier(reference, 64)?;
    }
    if let Some(reason) = &structure.pause_reason {
        validate_identifier(reason, 64)?;
    }
    validate_axis_bounded(structure.reserved_footprint, MAX_BLUEPRINT_FOOTPRINT_AXIS)?;
    if QuarterTurn::from_degrees(structure.rotation).is_none() {
        return Err(PluginStorageStartError::Malformed("settlement rotation"));
    }
    if structure.stages.is_empty() || structure.stages.len() > MAX_STRUCTURE_STAGES {
        return Err(PluginStorageStartError::Malformed("settlement stages"));
    }
    if structure.stage_index > structure.stages.len()
        || structure.watermark > MAX_SCRIPT_WORLD_TIME
        || structure.prepare_revision > MAX_SCRIPT_WORLD_TIME
    {
        return Err(PluginStorageStartError::Malformed("settlement progress"));
    }
    let mut total: u64 = 0;
    for stage in &structure.stages {
        stage
            .validate()
            .map_err(|_| PluginStorageStartError::Malformed("settlement stage"))?;
        let mut previous: Option<&str> = None;
        for material in &stage.materials {
            if previous.is_some_and(|value| value >= material.resource.as_str()) {
                return Err(PluginStorageStartError::Malformed("settlement stage order"));
            }
            previous = Some(material.resource.as_str());
        }
        total = total
            .checked_add(stage.work_units)
            .ok_or(PluginStorageStartError::Malformed("settlement work"))?;
    }
    if structure.built_blocks > total
        || (structure.stage_index == structure.stages.len() && structure.built_blocks != total)
        || (structure.state == ScriptStructureState::Committed
            && structure.stage_index != structure.stages.len())
    {
        return Err(PluginStorageStartError::Malformed("settlement progress"));
    }
    validate_material_counts(&structure.consumed)?;
    validate_revision(structure.revision)
}

fn validate_survey_token(survey: &DurableSurveyToken) -> Result<(), PluginStorageStartError> {
    validate_identifier(&survey.token, MAX_SURVEY_TOKEN_BYTES)?;
    validate_identifier(&survey.plugin_id, 128)?;
    validate_identifier(&survey.dimension, 128)?;
    survey
        .bounds
        .validate()
        .map_err(|_| PluginStorageStartError::Malformed("settlement survey bounds"))?;
    if survey.world_revision > MAX_SCRIPT_WORLD_TIME
        || survey.expires_transaction > MAX_SCRIPT_WORLD_TIME
    {
        return Err(PluginStorageStartError::Malformed(
            "settlement survey revision",
        ));
    }
    validate_revision(survey.revision)
}

fn validate_warehouse_binding(
    binding: &DurableWarehouseBinding,
) -> Result<(), PluginStorageStartError> {
    validate_identifier(&binding.handle, MAX_WAREHOUSE_HANDLE_BYTES)?;
    validate_identifier(&binding.plugin_id, 128)?;
    validate_identifier(&binding.structure_id, MAX_STRUCTURE_ID_BYTES)?;
    validate_revision(binding.revision)
}

/// One loaded-container reading of the authoritative world.
///
/// The variant is what keeps a missing container distinct from an unloaded
/// chunk: `Missing` is a loaded chunk without a container block/entity at the
/// position, `Unloaded` is a chunk core cannot observe at all, and `Loaded`
/// carries the canonical slot items of the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainerReading {
    Unloaded,
    Missing,
    Loaded(Vec<ItemStack>),
}

/// The world facing half of the settlement runtime.
///
/// Core installs one adapter over live world storage; tests install an
/// in-memory fake. Nothing else in this module reaches authoritative terrain.
pub(crate) trait SettlementWorld: Send + Sync {
    /// Bounded terrain reading of one plugin's requested region.
    fn survey(
        &self,
        plugin_id: &str,
        dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<SurveyReading, ScriptOperationFailure>;
    /// Current revision of the loaded world.
    fn world_revision(&self) -> u64;
    /// Whether anything inside `bounds` changed after `revision`.
    fn footprint_changed_since(&self, bounds: ScriptSurveyBounds, revision: u64) -> bool;
    /// Whether another plugin's structure or claim overlaps `bounds`.
    fn claims_overlap(&self, plugin_id: &str, bounds: ScriptSurveyBounds) -> bool;
    /// Highest opaque block Y over the footprint columns, or `None` when any
    /// footprint chunk is not loaded.
    ///
    /// One bounded column read per `(x, z)`. This is an occupancy ceiling, not
    /// a terrain surface: an opaque player- or plugin-placed block raises it,
    /// and water or other non-opaque obstacles do not. A structure may only be
    /// placed where nothing rises above its base row, so this backs the fit
    /// check demanded by the contract's "no terrain overwriting" rule.
    fn max_opaque_y(
        &self,
        bounds: ScriptSurveyBounds,
    ) -> Result<Option<i32>, ScriptOperationFailure>;
    /// Canonical reading of one authored container at a world position.
    ///
    /// A warehouse handle resolves to the container its authored seed placed;
    /// this is the only world read that answers whether that position is a
    /// loaded container and, if so, what it holds. Missing and unloaded stay
    /// distinct so a caller never reports an unloaded chunk as an empty one.
    fn container_reading(
        &self,
        position: [i32; 3],
    ) -> Result<ContainerReading, ScriptOperationFailure>;
    /// Apply one committed structure portion to the world.
    ///
    /// The portion commits on the world's own async surface, so the future is
    /// boxed and `Send`; keeping it behind a trait object lets the settlement
    /// runtime stay generic over the live world and the in-memory fakes.
    fn apply_structure_portion<'a>(
        &'a self,
        plugin_id: &'a str,
        structure_id: &'a str,
        blocks: &'a [StructureBlockPlacement],
    ) -> Pin<Box<dyn Future<Output = Result<(), ScriptOperationFailure>> + Send + 'a>>;
}

/// One bounded terrain survey reading.
#[derive(Debug, Clone)]
pub(crate) struct SurveyReading {
    pub(crate) usable_plots: u32,
    pub(crate) water_columns: u32,
    pub(crate) claimed: bool,
    pub(crate) existing_structures: u32,
    pub(crate) biome_tags: Vec<String>,
    pub(crate) resource_tags: Vec<String>,
    pub(crate) chunk_availability: ScriptChunkAvailability,
    pub(crate) revision: u64,
}

/// One block of a committed structure portion, in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StructureBlockPlacement {
    pub(crate) pos: [i32; 3],
    pub(crate) state: mc_world::BlockStateId,
}

/// Deterministic settlement identity material: selector, catalog, world id, and
/// the terrain generator the world itself generates from.
pub(crate) struct SettlementRuntime {
    selector: SettlementSelector,
    catalog: Arc<BlueprintCatalog>,
    world_identity: String,
    start_cell: [i32; 2],
    ground: Arc<dyn mc_world::ChunkGenerator>,
}

impl SettlementRuntime {
    /// Built by the worldgen wiring and by the settlement tests.
    ///
    /// `ground` is the startup generator the world generates from, so a site's
    /// base rows come from the terrain the players will actually stand on.
    #[allow(dead_code)]
    pub(crate) fn new(
        selector: SettlementSelector,
        catalog: Arc<BlueprintCatalog>,
        world_identity: impl Into<String>,
        start_cell: [i32; 2],
        ground: Arc<dyn mc_world::ChunkGenerator>,
    ) -> Self {
        Self {
            selector,
            catalog,
            world_identity: world_identity.into(),
            start_cell,
            ground,
        }
    }

    pub(crate) fn ground(&self) -> &Arc<dyn mc_world::ChunkGenerator> {
        &self.ground
    }

    pub(crate) fn selector(&self) -> &SettlementSelector {
        &self.selector
    }

    pub(crate) fn catalog(&self) -> &BlueprintCatalog {
        &self.catalog
    }

    pub(crate) fn world_identity(&self) -> &str {
        &self.world_identity
    }

    pub(crate) fn start_cell(&self) -> [i32; 2] {
        self.start_cell
    }

    /// Resolve every requested approved guard post to its anchor position and
    /// capacity, from the committed site layout the selector derives.
    ///
    /// A post handle is a C2 point-of-interest id, whose leading segment is the
    /// deterministic site id; posts that name no guard point of interest, or
    /// whose site cannot be reconstructed, are omitted (the caller reports
    /// `blocked_route`). The request is bounded: one layout is resolved per
    /// distinct site and no cell other than the named sites is ever scanned.
    pub(super) fn guard_posts(&self, posts: &[String]) -> BTreeMap<String, (Vec3, u16)> {
        let mut wanted: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for post in posts {
            let Some(site_id) = post.split('.').next() else {
                continue;
            };
            wanted.entry(site_id).or_default().push(post.as_str());
        }
        let mut resolved = BTreeMap::new();
        for (site_id, posts) in wanted {
            let Some(cell) = self.selector.cell_from_site_id(site_id) else {
                continue;
            };
            let Some(candidate) = self.selector.candidate(cell) else {
                continue;
            };
            let Ok(layout) = layout_of(self, &candidate) else {
                continue;
            };
            for poi in &layout.pois {
                if poi.kind == PoiKind::Guard && posts.contains(&poi.poi_id.as_str()) {
                    resolved.insert(
                        poi.poi_id.clone(),
                        (
                            Vec3::new(
                                f64::from(poi.at[0]) + 0.5,
                                f64::from(poi.at[1]),
                                f64::from(poi.at[2]) + 0.5,
                            ),
                            poi.capacity,
                        ),
                    );
                }
            }
        }
        resolved
    }
}

impl super::InventoryRuntime {
    /// Execute one settlement request. Queries answer from the deterministic
    /// selector and the durable ledger; every mutation commits its receipt
    /// together with the ledger change it decided.
    pub(crate) async fn execute_settlement_operation(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let ScriptOperation::Settlement { operation } = request.operation() else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let Some(runtime) = self.settlement_runtime() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        match operation {
            ScriptSettlementOperation::ListSites { cursor, limit } => {
                Ok(self.list_sites(storage, runtime, cursor.as_deref(), *limit))
            }
            ScriptSettlementOperation::QuerySite {
                site_id,
                cursor,
                limit,
            } => Ok(self.query_site(storage, runtime, site_id, cursor.as_deref(), *limit)),
            ScriptSettlementOperation::Status { structure_id } => {
                Ok(self.structure_status(storage, plugin_id, structure_id))
            }
            ScriptSettlementOperation::Survey {
                dimension, bounds, ..
            } => self.survey(storage, plugin_id, dimension, *bounds),
            _ => {
                let Some(operation_id) = operation.operation_id() else {
                    return Ok(rejected(ScriptOperationFailure::InvalidRequest));
                };
                if let Some(outcome) =
                    replay_settlement_operation(storage, plugin_id, request, operation_id)
                {
                    return Ok(outcome);
                }
                match operation {
                    ScriptSettlementOperation::ReserveResidentSite {
                        site_id,
                        poi_id,
                        expected_site_revision,
                        ..
                    } => self.reserve_site_poi(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        site_id,
                        poi_id,
                        *expected_site_revision,
                    ),
                    ScriptSettlementOperation::ReleaseResidentSite {
                        spawn_site_token, ..
                    } => self.release_resident_site(storage, plugin_id, request, spawn_site_token),
                    ScriptSettlementOperation::PrepareStructure {
                        blueprint_id,
                        anchor,
                        rotation,
                        survey_token,
                        expected_site_revision,
                        ..
                    } => self.prepare_structure(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        blueprint_id,
                        *anchor,
                        *rotation,
                        survey_token,
                        *expected_site_revision,
                    ),
                    ScriptSettlementOperation::AdvanceStructure {
                        structure_id,
                        stage,
                        reservation_ref,
                        expected_revision,
                        work_units,
                        ..
                    } => {
                        self.advance_structure(
                            storage,
                            runtime,
                            plugin_id,
                            request,
                            structure_id,
                            stage,
                            reservation_ref,
                            *expected_revision,
                            *work_units,
                        )
                        .await
                    }
                    ScriptSettlementOperation::PauseStructure {
                        structure_id,
                        expected_revision,
                        ..
                    } => self.pause_structure(
                        storage,
                        plugin_id,
                        request,
                        structure_id,
                        *expected_revision,
                    ),
                    ScriptSettlementOperation::CancelStructure {
                        structure_id,
                        expected_revision,
                        ..
                    } => self.cancel_structure(
                        storage,
                        plugin_id,
                        request,
                        structure_id,
                        *expected_revision,
                    ),
                    ScriptSettlementOperation::BindWarehouse {
                        structure_id,
                        container_id,
                        ..
                    } => self.bind_warehouse(
                        storage,
                        runtime,
                        plugin_id,
                        request,
                        structure_id,
                        *container_id,
                    ),
                    _ => Ok(rejected(ScriptOperationFailure::InvalidRequest)),
                }
            }
        }
    }

    fn list_sites(
        &self,
        storage: &PluginStorage,
        runtime: &SettlementRuntime,
        cursor: Option<&str>,
        limit: u8,
    ) -> ScriptOperationOutcome {
        let start = match cursor {
            Some(cursor) => match parse_cursor(cursor) {
                Some(cell) => cell,
                None => return rejected(ScriptOperationFailure::CursorExpired),
            },
            None => runtime.start_cell(),
        };
        let limit = usize::from(limit).min(MAX_SETTLEMENT_SITE_PAGE);
        let mut sites = Vec::new();
        for candidate in runtime.selector().discover(start, limit) {
            match layout_of(runtime, &candidate)
                .and_then(|layout| site_snapshot(runtime, storage, &candidate, &layout))
            {
                Ok(site) => sites.push(site),
                Err(failure) => return rejected(failure),
            }
        }
        let mut page =
            ScriptSettlementSitePage::new(sites, Some(encode_cursor(scan_cell(start, limit))));
        page.canonicalize();
        settled_outcome(
            storage.revision,
            ScriptSettlementResult::Sites {
                page: Box::new(page),
            },
        )
    }

    fn query_site(
        &self,
        storage: &PluginStorage,
        runtime: &SettlementRuntime,
        site_id: &str,
        cursor: Option<&str>,
        limit: u8,
    ) -> ScriptOperationOutcome {
        let Some(cell) = runtime.selector().cell_from_site_id(site_id) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        let Some(candidate) = runtime.selector().candidate(cell) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        let layout = match layout_of(runtime, &candidate) {
            Ok(layout) => layout,
            Err(failure) => return rejected(failure),
        };
        let mut site = match site_snapshot(runtime, storage, &candidate, &layout) {
            Ok(site) => site,
            Err(failure) => return rejected(failure),
        };
        // The page is a contiguous slice of the canonical point-of-interest
        // order; the caller pages on by naming the last id it received.
        let start = match cursor {
            Some(cursor) => {
                match site
                    .pois
                    .binary_search_by(|poi| poi.poi_id.as_str().cmp(cursor))
                {
                    Ok(index) => index + 1,
                    Err(_) => return rejected(ScriptOperationFailure::CursorExpired),
                }
            }
            None => 0,
        };
        site.pois = site
            .pois
            .into_iter()
            .skip(start)
            .take(usize::from(limit))
            .collect();
        settled_outcome(
            storage.revision,
            ScriptSettlementResult::Site {
                site: Box::new(site),
            },
        )
    }

    fn structure_status(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        structure_id: &str,
    ) -> ScriptOperationOutcome {
        let Some(structure) = storage.settlements().structure(structure_id) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        if structure.plugin_id != plugin_id {
            return rejected(ScriptOperationFailure::Forbidden);
        }
        settled_outcome(
            structure.revision,
            ScriptSettlementResult::Structure {
                structure: Box::new(structure_snapshot(storage, structure)),
            },
        )
    }

    /// Bind and return one warehouse handle for an authored container of an
    /// active structure the caller owns.
    ///
    /// The plugin names its own durable structure and an authored container
    /// ordinal; core verifies ownership, liveness, that the ordinal is an
    /// authored `empty_container` seed of that structure's blueprint, and that
    /// the chunk is loaded with a container at the placed position. The binding
    /// is durable and idempotent: a container already bound to the same plugin
    /// answers its original handle and revision without a second ledger entry,
    /// and a container bound to another plugin is refused.
    fn bind_warehouse(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        container_id: u32,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(structure) = storage.settlements().structure(structure_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if structure.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !structure.is_placed() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if let Some(existing) = storage
            .settlements()
            .warehouse_for_container(structure_id, container_id)
        {
            if existing.plugin_id != plugin_id {
                return Ok(rejected(ScriptOperationFailure::Forbidden));
            }
            // A repeated bind is the same binding: answer the original handle
            // and revision, but still record the receipt for this operation id
            // so the plugin's durable intent recovers through
            // `operation_status`. No ledger change is appended, so the binding
            // and its revision stay exactly as minted.
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Warehouse {
                    binding: Box::new(existing.snapshot()),
                }),
                Vec::new(),
            );
        }
        let position = match warehouse_container_position(runtime, &structure, container_id) {
            Ok(position) => position,
            Err(failure) => return Ok(rejected(failure)),
        };
        match world.container_reading(position) {
            Ok(ContainerReading::Loaded(_)) => {}
            Ok(ContainerReading::Unloaded) => {
                return Ok(rejected(ScriptOperationFailure::Unloaded));
            }
            Ok(ContainerReading::Missing) => {
                return Ok(rejected(ScriptOperationFailure::NotFound));
            }
            Err(failure) => return Ok(rejected(failure)),
        }
        let handle = match warehouse_handle(plugin_id, structure_id, container_id) {
            Ok(handle) => handle,
            Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
        };
        let binding = DurableWarehouseBinding {
            handle,
            plugin_id: plugin_id.to_owned(),
            structure_id: structure_id.to_owned(),
            container_id,
            revision: 0,
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Warehouse {
                binding: Box::new(binding.snapshot()),
            }),
            vec![DurableSettlementChange::Warehouse {
                binding: Box::new(binding),
            }],
        )
    }

    /// Canonical snapshot of one bound warehouse container, or the typed
    /// failure that keeps a plugin from reading a foreign, unloaded or inactive
    /// container.
    ///
    /// The opaque handle resolves through the durable binding; the binding owner
    /// must be the caller; the bound structure must still be active; and the
    /// authored container must still be a loaded container at its placed
    /// position. Every one of those closes with its own family rather than an
    /// empty snapshot, so a plugin can never mistake a refusal for an empty
    /// container.
    pub(super) fn warehouse_inventory_snapshot(
        &self,
        storage: &PluginStorage,
        plugin_id: &str,
        handle: &str,
        expected_revision: Option<u64>,
    ) -> ScriptOperationOutcome {
        let Some(runtime) = self.settlement_runtime() else {
            return rejected(ScriptOperationFailure::RuntimeUnavailable);
        };
        let Some(world) = self.settlement_world() else {
            return rejected(ScriptOperationFailure::RuntimeUnavailable);
        };
        let Some(binding) = storage.settlements().warehouse(handle) else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        if binding.plugin_id != plugin_id {
            return rejected(ScriptOperationFailure::Forbidden);
        }
        let Some(structure) = storage
            .settlements()
            .structure(&binding.structure_id)
            .cloned()
        else {
            return rejected(ScriptOperationFailure::NotFound);
        };
        if !structure.is_placed() {
            return rejected(ScriptOperationFailure::Blocked);
        }
        if expected_revision.is_some_and(|expected| expected != binding.revision) {
            return rejected(ScriptOperationFailure::StaleRevision);
        }
        let position = match warehouse_container_position(runtime, &structure, binding.container_id)
        {
            Ok(position) => position,
            Err(failure) => return rejected(failure),
        };
        let items = match world.container_reading(position) {
            Ok(ContainerReading::Loaded(items)) => items,
            Ok(ContainerReading::Unloaded) => return rejected(ScriptOperationFailure::Unloaded),
            Ok(ContainerReading::Missing) => return rejected(ScriptOperationFailure::NotFound),
            Err(failure) => return rejected(failure),
        };
        self.sessions().query_warehouse_inventory(
            &ScriptInventoryEndpoint::Warehouse {
                handle: handle.to_owned(),
            },
            binding.revision,
            &items,
            self.items(),
        )
    }

    fn survey(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        dimension: &str,
        bounds: ScriptSurveyBounds,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        if bounds.validate().is_err() {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let reading = match world.survey(plugin_id, dimension, bounds) {
            Ok(reading) => reading,
            Err(failure) => return Ok(rejected(failure)),
        };
        if reading.chunk_availability != ScriptChunkAvailability::Loaded {
            return Ok(rejected(ScriptOperationFailure::Unloaded));
        }
        let transaction_id = storage
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let token = survey_token(plugin_id, transaction_id, dimension, bounds);
        let snapshot = ScriptSurveySnapshot::new(
            dimension.to_owned(),
            bounds,
            transaction_id,
            reading.chunk_availability,
            token.clone(),
            reading.usable_plots,
            reading.water_columns,
            reading.claimed,
            reading.existing_structures,
            reading.biome_tags,
            reading.resource_tags,
        );
        let (batch, outcome) = match storage.prepare_settlement_change_batch(
            plugin_id,
            settlement_payload(ScriptSettlementResult::Survey {
                survey: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Survey {
                survey: Box::new(DurableSurveyToken {
                    token,
                    plugin_id: plugin_id.to_owned(),
                    dimension: dimension.to_owned(),
                    bounds,
                    world_revision: reading.revision,
                    expires_transaction: transaction_id
                        .saturating_add(SURVEY_TOKEN_TTL_TRANSACTIONS),
                    revision: 0,
                }),
            }],
        )? {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(rejected(ScriptOperationFailure::Capacity));
            }
        };
        storage.commit_batch(batch)?;
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn reserve_site_poi(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        site_id: &str,
        poi_id: &str,
        expected_site_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(cell) = runtime.selector().cell_from_site_id(site_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let Some(candidate) = runtime.selector().candidate(cell) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let layout = match layout_of(runtime, &candidate) {
            Ok(layout) => layout,
            Err(failure) => return Ok(rejected(failure)),
        };
        let Some((slot, poi)) = layout
            .pois
            .iter()
            .enumerate()
            .find(|(_, poi)| poi.poi_id == poi_id)
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if poi.kind != PoiKind::Home {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let existing = storage.settlements().site(site_id).cloned();
        let site_revision = existing.as_ref().map_or(0, |site| site.revision);
        if expected_site_revision != site_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if let Some(reservation) = existing.as_ref().and_then(|site| site.reservation(poi_id))
            && !reservation.released
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let generation_id =
            match resident_generation_id(runtime.world_identity(), site_id, slot as u32) {
                Ok(generation_id) => generation_id,
                Err(_) => return Ok(rejected(ScriptOperationFailure::InvalidRequest)),
            };
        let position = Vec3::new(
            f64::from(poi.at[0]),
            f64::from(poi.at[1]),
            f64::from(poi.at[2]),
        );
        let token = match super::InventoryRuntime::reserve_resident_site(
            storage,
            plugin_id,
            &generation_id,
            position,
        ) {
            Ok(token) => token,
            Err(failure) => return Ok(rejected(failure)),
        };
        let mut site = existing.unwrap_or_else(|| DurableSiteState::new(site_id.to_owned()));
        site.pois.insert(
            poi_id.to_owned(),
            DurablePoiReservation {
                token: token.clone(),
                plugin_id: plugin_id.to_owned(),
                consumed: false,
                released: false,
            },
        );
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::ResidentSite {
                reservation: Box::new(ScriptResidentSiteReservation::new(
                    site_id.to_owned(),
                    poi_id.to_owned(),
                    token,
                    0,
                )),
            }),
            vec![DurableSettlementChange::Site {
                site: Box::new(site),
            }],
        )
    }

    fn release_resident_site(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        spawn_site_token: &str,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some((site, poi_id)) = storage
            .settlements()
            .reservation_by_token(spawn_site_token)
            .map(|(site, poi_id)| (site.clone(), poi_id.to_owned()))
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let mut site = site;
        let Some(reservation) = site.pois.get_mut(&poi_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if reservation.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if reservation.released {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        }
        // A consumed spawn site can never be handed back: the resident ledger
        // already bound this home to a spawned inhabitant, so releasing it would
        // make an occupied home reservable again. Refuse with the same family
        // the reserve path uses, before any state change is committed.
        let resident_site = storage.residents().site(&reservation.token).cloned();
        let consumed = reservation.consumed
            || resident_site.is_some_and(|site| {
                site.consumed
                    || storage
                        .residents()
                        .record_by_generation(&site.generation_id)
                        .is_some()
            });
        if consumed {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        // Mirror the resident ledger's observation durably: a slot whose
        // inhabitant was already spawned stays recorded as such even after the
        // settlement-level reservation is handed back.
        reservation.consumed |= storage
            .residents()
            .site(&reservation.token)
            .is_some_and(|site| site.consumed);
        reservation.released = true;
        let site_id = site.site_id.clone();
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::ResidentSite {
                reservation: Box::new(ScriptResidentSiteReservation::new(
                    site_id,
                    poi_id,
                    spawn_site_token.to_owned(),
                    0,
                )),
            }),
            vec![DurableSettlementChange::Site {
                site: Box::new(site),
            }],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_structure(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        blueprint_id: &str,
        anchor: [i32; 3],
        rotation: u16,
        survey_token: &str,
        expected_site_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(blueprint) = runtime.catalog().get(blueprint_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let Some(turn) = QuarterTurn::from_degrees(rotation) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let reserved_footprint = rotated_extent(blueprint.size(), turn);
        if reserved_footprint
            .iter()
            .any(|axis| *axis <= 0 || *axis > MAX_BLUEPRINT_FOOTPRINT_AXIS)
        {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let bounds = bounds_of(anchor, reserved_footprint);
        if bounds.min[1] < mc_world::MIN_Y || bounds.max[1] >= mc_world::MAX_Y {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        // Contract rule: a structure is only placed where nothing rises above
        // its base row. Terrain or vegetation above that row would be
        // overwritten (forbidden) or wall the interior in, so the placement is
        // refused instead of built on top of the terrain. Terrain level with
        // the base row is the ground the structure stands on.
        match world.max_opaque_y(bounds) {
            Ok(None) => return Ok(rejected(ScriptOperationFailure::Unloaded)),
            Ok(Some(surface)) if surface > anchor[1] => {
                return Ok(rejected(ScriptOperationFailure::Blocked));
            }
            Ok(Some(_)) => {}
            Err(failure) => return Ok(rejected(failure)),
        }
        if blueprint.street_connections().is_empty() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if storage.settlements().active_structures(plugin_id).len()
            >= MAX_STRUCTURE_ACTIVE_PER_PLUGIN
        {
            return Ok(rejected(ScriptOperationFailure::Capacity));
        }
        let cell = [
            anchor[0].div_euclid(SITE_CELL_BLOCKS),
            anchor[2].div_euclid(SITE_CELL_BLOCKS),
        ];
        let Some(candidate) = runtime.selector().candidate(cell) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        let site_revision = storage
            .settlements()
            .site(&candidate.site_id)
            .map_or(0, |site| site.revision);
        if expected_site_revision != site_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        if world.claims_overlap(plugin_id, bounds)
            || storage
                .settlements()
                .structures_of(plugin_id)
                .any(|structure| {
                    structure.state != ScriptStructureState::Cancelled
                        && bounds_overlap(&structure.bounds(), &bounds)
                })
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let Some(token) = storage.settlements().survey(survey_token) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if token.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if token.expires_transaction < storage.revision
            || world.footprint_changed_since(token.bounds, token.world_revision)
        {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let instance = BlueprintInstance::new(Arc::clone(&blueprint), turn, anchor);
        let stages = match stage_plans(&instance) {
            Ok(stages) => stages,
            Err(failure) => return Ok(rejected(failure)),
        };
        let plan = structure_resource_plan(&stages);
        if plan.validate().is_err() {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let transaction_id = storage
            .revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)?;
        let structure = DurableStructure {
            structure_id: mint_structure_id(plugin_id, transaction_id, blueprint_id, anchor),
            plugin_id: plugin_id.to_owned(),
            site_id: candidate.site_id.clone(),
            blueprint_id: blueprint_id.to_owned(),
            blueprint_hash: blueprint.content_hash().to_owned(),
            origin: anchor,
            rotation,
            reserved_footprint,
            state: ScriptStructureState::Prepared,
            stages,
            stage_index: 0,
            watermark: 0,
            built_blocks: 0,
            reservation_ref: None,
            resource_plan_hash: resource_plan_hash(&plan),
            consumed: BTreeMap::new(),
            prepare_revision: world.world_revision(),
            pause_reason: None,
            revision: 0,
        };
        let snapshot = structure_snapshot(storage, &structure);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(structure),
            }],
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn advance_structure(
        &self,
        storage: &mut PluginStorage,
        runtime: &SettlementRuntime,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        stage: &str,
        reservation_ref: &str,
        expected_revision: u64,
        work_units: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(world) = self.settlement_world() else {
            return Ok(rejected(ScriptOperationFailure::RuntimeUnavailable));
        };
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !matches!(
            record.state,
            ScriptStructureState::Prepared | ScriptStructureState::Running
        ) {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(planned_stage) = record.stage() else {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        };
        if planned_stage.stage != stage {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        if let Some(bound) = record.reservation_ref()
            && bound != reservation_ref
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let Some((_, reservation)) = storage.settlement_reservation(plugin_id, reservation_ref)
        else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if reservation.released || reservation.resource_plan_hash != record.resource_plan_hash {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if reservation
            .bound_to
            .as_deref()
            .is_some_and(|bound| bound != structure_id)
        {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        let mut record = record.clone();
        if world.footprint_changed_since(record.bounds(), record.prepare_revision) {
            record.state = ScriptStructureState::Paused;
            record.pause_reason = Some(PAUSE_SITE_CHANGED.to_owned());
            let snapshot = structure_snapshot(storage, &record);
            return commit_settlement(
                storage,
                plugin_id,
                request,
                settlement_payload(ScriptSettlementResult::Structure {
                    structure: Box::new(snapshot),
                }),
                vec![DurableSettlementChange::Structure {
                    structure: Box::new(record),
                }],
            );
        }
        let Some(blueprint) = runtime.catalog().get(&record.blueprint_id).cloned() else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if blueprint.content_hash() != record.blueprint_hash {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let Some(index) = record.stages.iter().position(|plan| plan.stage == stage) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        if index != record.stage_index {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let Some(turn) = QuarterTurn::from_degrees(record.rotation) else {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        };
        let instance = BlueprintInstance::new(blueprint, turn, record.origin);
        let cells = match stage_cells(&instance, index) {
            Ok(cells) => cells,
            Err(failure) => return Ok(rejected(failure)),
        };
        if cells.len() as u64 != planned_stage.work_units {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let stage_offset: u64 = record.stages[..index]
            .iter()
            .map(|plan| plan.work_units)
            .sum();
        let built_in_stage = record.built_blocks.saturating_sub(stage_offset);
        let remaining = (cells.len() as u64).saturating_sub(built_in_stage);
        // The request authorizes at most this much work; the portion is what the
        // stage can still commit, capped by the world commit bound.
        let portion_len = work_units
            .min(remaining)
            .min(MAX_WORLD_COMMIT_PORTION as u64);
        if portion_len == 0 {
            return Ok(rejected(ScriptOperationFailure::InvalidRequest));
        }
        let portion = &cells[built_in_stage as usize..(built_in_stage + portion_len) as usize];
        let mut consumed: BTreeMap<String, u64> = BTreeMap::new();
        for cell in portion {
            *consumed.entry(cell.block.clone()).or_insert(0) += 1;
        }
        for (resource, quantity) in &consumed {
            let available = reservation
                .quantities
                .iter()
                .find(|entry| &entry.resource_id == resource)
                .map_or(0, |entry| entry.remaining);
            if *quantity > available {
                return Ok(rejected(ScriptOperationFailure::InsufficientItems));
            }
        }
        let placements = portion
            .iter()
            .map(|cell| StructureBlockPlacement {
                pos: cell.pos,
                state: cell.state,
            })
            .collect::<Vec<_>>();
        if let Err(failure) = world
            .apply_structure_portion(plugin_id, structure_id, &placements)
            .await
        {
            return Ok(rejected(failure));
        }
        // The portion above is a durable world commit of this structure's own
        // staged work, so it advances the world's durable revision just like a
        // foreign edit would. Re-observe the footprint after it: the next
        // advance must fence against the state this structure itself produced,
        // not pause on the durable decision it just wrote.
        record.prepare_revision = world.world_revision();
        let receipt = ScriptStructureReceipt::new(
            structure_id.to_owned(),
            stage.to_owned(),
            record.watermark + 1,
            u32::try_from(portion_len).map_err(|_| PluginStorageMutationError::RevisionOverflow)?,
            portion_len,
            materials_of(&consumed),
            0,
        );
        record.watermark += 1;
        record.built_blocks += portion_len;
        record.reservation_ref = Some(reservation_ref.to_owned());
        record.pause_reason = None;
        for (resource, quantity) in consumed {
            *record.consumed.entry(resource).or_insert(0) += quantity;
        }
        if built_in_stage + portion_len == cells.len() as u64 {
            record.stage_index += 1;
        }
        record.state = if record.stage_index == record.stages.len() {
            ScriptStructureState::Committed
        } else {
            ScriptStructureState::Running
        };
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Receipt {
                receipt: Box::new(receipt),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }

    fn pause_structure(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !matches!(
            record.state,
            ScriptStructureState::Prepared | ScriptStructureState::Running
        ) {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let changed = self.settlement_world().is_some_and(|world| {
            world.footprint_changed_since(record.bounds(), record.prepare_revision)
        });
        let mut record = record.clone();
        record.state = ScriptStructureState::Paused;
        record.pause_reason = changed.then(|| PAUSE_SITE_CHANGED.to_owned());
        let snapshot = structure_snapshot(storage, &record);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }

    fn cancel_structure(
        &self,
        storage: &mut PluginStorage,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        structure_id: &str,
        expected_revision: u64,
    ) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
        let Some(record) = storage.settlements().structure(structure_id) else {
            return Ok(rejected(ScriptOperationFailure::NotFound));
        };
        if record.plugin_id != plugin_id {
            return Ok(rejected(ScriptOperationFailure::Forbidden));
        }
        if !record.is_active() {
            return Ok(rejected(ScriptOperationFailure::Blocked));
        }
        if record.revision != expected_revision {
            return Ok(rejected(ScriptOperationFailure::StaleRevision));
        }
        let mut record = record.clone();
        record.state = ScriptStructureState::Cancelled;
        let snapshot = structure_snapshot(storage, &record);
        commit_settlement(
            storage,
            plugin_id,
            request,
            settlement_payload(ScriptSettlementResult::Structure {
                structure: Box::new(snapshot),
            }),
            vec![DurableSettlementChange::Structure {
                structure: Box::new(record),
            }],
        )
    }
}

fn commit_settlement(
    storage: &mut PluginStorage,
    plugin_id: &str,
    request: &ScriptOperationRequest,
    payload: ScriptOperationPayload,
    changes: Vec<DurableSettlementChange>,
) -> Result<ScriptOperationOutcome, PluginStorageMutationError> {
    let batch =
        match storage.prepare_settlement_operation_batch(plugin_id, request, payload, changes)? {
            ScriptStoragePrepareOutcome::Prepared(batch) => batch,
            ScriptStoragePrepareOutcome::Rejected => {
                return Ok(rejected(ScriptOperationFailure::Capacity));
            }
        };
    storage.commit_batch(batch)?;
    let operation_id = request
        .operation_id()
        .expect("settlement mutation decision identity");
    Ok(storage
        .operation_receipt(plugin_id, operation_id)
        .expect("committed settlement receipt remains installed")
        .outcome
        .clone())
}

fn replay_settlement_operation(
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

/// One deterministic site layout; a catalog that cannot lay the variant out is a
/// core configuration failure, never a silent downgrade.
fn layout_of(
    runtime: &SettlementRuntime,
    candidate: &SiteCandidate,
) -> Result<SiteLayout, ScriptOperationFailure> {
    // The layout resolves its own base rows through the same generator the
    // world generates from; a generator that cannot answer a column fails the
    // layout closed rather than inventing a level.
    let ground = runtime.ground();
    runtime
        .selector()
        .layout(candidate, runtime.catalog(), &|x, z| {
            ground.surface_height(x, z)
        })
        .map_err(|_| ScriptOperationFailure::RuntimeUnavailable)
}

/// One site snapshot. The generator's variant footprint is reported verbatim: a
/// site is reserved territory, not a building, so it is never squeezed into the
/// per-building blueprint bound.
fn site_snapshot(
    runtime: &SettlementRuntime,
    storage: &PluginStorage,
    candidate: &SiteCandidate,
    layout: &SiteLayout,
) -> Result<ScriptSettlementSite, ScriptOperationFailure> {
    let ledger = storage.settlements().site(&candidate.site_id);
    let homes = layout
        .pois
        .iter()
        .filter(|poi| poi.kind == PoiKind::Home)
        .count();
    if homes > MAX_SETTLEMENT_RESIDENTS {
        return Err(ScriptOperationFailure::Blocked);
    }
    let mut pois = Vec::with_capacity(layout.pois.len());
    let mut inhabitant_generation_ids = Vec::with_capacity(homes);
    for (slot, poi) in layout.pois.iter().enumerate() {
        if poi.kind == PoiKind::Home {
            let generation =
                resident_generation_id(runtime.world_identity(), &candidate.site_id, slot as u32)
                    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
            inhabitant_generation_ids.push(generation);
        }
        pois.push(ScriptSettlementPoi::new(
            poi.poi_id.clone(),
            poi_kind(poi.kind),
            poi.at,
            poi.capacity,
            poi_state(storage, ledger, &poi.poi_id),
        ));
    }
    let mut site = ScriptSettlementSite::new(
        candidate.site_id.clone(),
        site_variant(candidate.variant),
        ledger.map_or(0, |site| site.revision),
        layout.candidate.origin,
        layout.candidate.size,
        layout
            .placements
            .iter()
            .map(|placement| {
                ScriptSettlementBuilding::new(
                    placement.blueprint_id.clone(),
                    placement.origin,
                    placement.rotation,
                )
            })
            .collect(),
        pois,
        inhabitant_generation_ids,
    );
    site.canonicalize();
    Ok(site)
}

fn poi_state(
    storage: &PluginStorage,
    ledger: Option<&DurableSiteState>,
    poi_id: &str,
) -> ScriptSitePoiState {
    let Some(reservation) = ledger.and_then(|site| site.reservation(poi_id)) else {
        return ScriptSitePoiState::Free;
    };
    if reservation.released {
        return ScriptSitePoiState::Free;
    }
    let spawned = storage
        .residents()
        .site(&reservation.token)
        .is_some_and(|site| site.consumed);
    if reservation.consumed || spawned {
        ScriptSitePoiState::Occupied
    } else {
        ScriptSitePoiState::Reserved
    }
}

fn site_variant(variant: SiteVariant) -> ScriptSiteVariant {
    match variant {
        SiteVariant::Hamlet => ScriptSiteVariant::Hamlet,
        SiteVariant::Village => ScriptSiteVariant::Village,
        SiteVariant::Town => ScriptSiteVariant::Town,
    }
}

fn poi_kind(kind: PoiKind) -> ScriptSitePoiKind {
    match kind {
        PoiKind::Home => ScriptSitePoiKind::Home,
        PoiKind::Work => ScriptSitePoiKind::Work,
        PoiKind::Meeting => ScriptSitePoiKind::Meeting,
        PoiKind::Guard => ScriptSitePoiKind::Guard,
    }
}

/// The one-work-portion-per-stage material plan of a structure. Its hash is the
/// fence a C1 reservation must carry before the structure may spend from it.
fn structure_resource_plan(stages: &[ScriptStructureStagePlan]) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(
        stages
            .iter()
            .map(|stage| {
                ScriptInventoryWorkPortion::new(
                    stage.work_units,
                    stage
                        .materials
                        .iter()
                        .map(|material| {
                            ScriptInventoryMaterial::new(
                                material.resource.clone(),
                                material.quantity,
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// One planned cell of a stage, after palette and rotation are resolved.
#[derive(Debug, Clone)]
struct StageCell {
    pos: [i32; 3],
    state: mc_world::BlockStateId,
    block: String,
}

/// Build the stage plan of one placed blueprint.
///
/// A blueprint that authors no stages builds its body in one stage named
/// [`BODY_STAGE`]; every other blueprint plans one stage per authored stage.
fn stage_plans(
    instance: &BlueprintInstance,
) -> Result<Vec<ScriptStructureStagePlan>, ScriptOperationFailure> {
    let count = instance.blueprint().stages().len().max(1);
    if count > MAX_STRUCTURE_STAGES {
        return Err(ScriptOperationFailure::Blocked);
    }
    let mut stages = Vec::with_capacity(count);
    let mut resources: BTreeSet<String> = BTreeSet::new();
    for index in 0..count {
        let cells = stage_cells(instance, index)?;
        if cells.is_empty() {
            return Err(ScriptOperationFailure::Blocked);
        }
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for cell in &cells {
            *counts.entry(cell.block.clone()).or_insert(0) += 1;
        }
        if counts.len() > MAX_STRUCTURE_RESOURCE_TYPES {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        resources.extend(counts.keys().cloned());
        if resources.len() > MAX_STRUCTURE_RESOURCE_TYPES {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        let block_count =
            u32::try_from(cells.len()).map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        stages.push(ScriptStructureStagePlan::new(
            stage_name(instance, index),
            block_count,
            u64::from(block_count),
            materials_of(&counts),
        ));
    }
    Ok(stages)
}

fn stage_name(instance: &BlueprintInstance, index: usize) -> String {
    instance
        .blueprint()
        .stages()
        .get(index)
        .map_or_else(|| BODY_STAGE.to_owned(), |stage| stage.id.clone())
}

/// Resolve one stage into world-space cells, one per distinct position.
///
/// A stage that authors several cells at one position keeps only the last
/// authored state, which is what applying every cell in order leaves behind, so
/// a committed portion can never split a position group.
fn stage_cells(
    instance: &BlueprintInstance,
    index: usize,
) -> Result<Vec<StageCell>, ScriptOperationFailure> {
    let blueprint = instance.blueprint();
    let (blocks, placed): (&[BlueprintBlock], Vec<mc_worldgen::PlacedBlock>) =
        if blueprint.stages().is_empty() {
            if index != 0 {
                return Err(ScriptOperationFailure::InvalidRequest);
            }
            (blueprint.blocks(), instance.placed_blocks())
        } else {
            let Some(stage) = blueprint.stages().get(index) else {
                return Err(ScriptOperationFailure::InvalidRequest);
            };
            (stage.blocks.as_slice(), instance.placed_stage_blocks(stage))
        };
    let palette: BTreeMap<u16, &str> = blueprint
        .palette()
        .iter()
        .map(|entry| (entry.index, entry.block.as_str()))
        .collect();
    let mut authored = Vec::with_capacity(blocks.len());
    for block in blocks {
        let block_id = palette
            .get(&block.palette)
            .copied()
            .ok_or(ScriptOperationFailure::InvalidRequest)?;
        authored.push((placed_position(instance, block), block_id));
    }
    // `placed` is sorted by position with a stable sort over the authored order,
    // so the two sequences describe the same cells one to one.
    authored.sort_by_key(|(pos, _)| *pos);
    if authored.len() != placed.len() {
        return Err(ScriptOperationFailure::InvalidRequest);
    }
    let mut cells: Vec<StageCell> = authored
        .into_iter()
        .zip(placed)
        .map(|((pos, block), placed)| {
            debug_assert_eq!(pos, placed.pos);
            StageCell {
                pos,
                state: placed.state,
                block: block.to_owned(),
            }
        })
        .collect();
    cells.reverse();
    cells.dedup_by(|left, right| left.pos == right.pos);
    cells.reverse();
    Ok(cells)
}

/// World position of one blueprint cell, matching
/// [`mc_worldgen::BlueprintInstance`] placement exactly.
fn placed_position(instance: &BlueprintInstance, block: &BlueprintBlock) -> [i32; 3] {
    let local = instance
        .turn()
        .rotate_offset([block.x, block.y, block.z], instance.blueprint().size());
    let origin = instance.origin();
    [
        origin[0].saturating_add(local[0]),
        origin[1].saturating_add(local[1]),
        origin[2].saturating_add(local[2]),
    ]
}

/// World position of one authored container of a durable structure.
///
/// `container_id` is the ordinal of an `empty_container` seed in the
/// blueprint's authored block-entity order, which is rotation independent; the
/// returned position projects that seed through the structure's own rotation
/// and origin, exactly as [`BlueprintInstance::placed_block_entities`] reports
/// it. A catalog whose blueprint no longer matches the content hash the
/// structure was prepared against is a stale revision, never a silently
/// different container.
fn warehouse_container_position(
    runtime: &SettlementRuntime,
    structure: &DurableStructure,
    container_id: u32,
) -> Result<[i32; 3], ScriptOperationFailure> {
    let blueprint = runtime
        .catalog()
        .get(&structure.blueprint_id)
        .ok_or(ScriptOperationFailure::NotFound)?;
    if blueprint.content_hash() != structure.blueprint_hash {
        return Err(ScriptOperationFailure::StaleRevision);
    }
    let turn = QuarterTurn::from_degrees(structure.rotation)
        .ok_or(ScriptOperationFailure::InvalidRequest)?;
    let index =
        usize::try_from(container_id).map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let local = blueprint
        .block_entities()
        .iter()
        .filter(|seed| seed.kind == BlockEntitySeedKind::EmptyContainer)
        .nth(index)
        .ok_or(ScriptOperationFailure::NotFound)?
        .at;
    let instance = BlueprintInstance::new(Arc::clone(blueprint), turn, structure.origin);
    let rotated = instance
        .turn()
        .rotate_offset(local, instance.blueprint().size());
    let origin = instance.origin();
    Ok([
        origin[0].saturating_add(rotated[0]),
        origin[1].saturating_add(rotated[1]),
        origin[2].saturating_add(rotated[2]),
    ])
}

fn rotated_extent(size: [i32; 3], turn: QuarterTurn) -> [i32; 3] {
    match turn {
        QuarterTurn::None | QuarterTurn::Cw180 => size,
        QuarterTurn::Cw90 | QuarterTurn::Cw270 => [size[2], size[1], size[0]],
    }
}

fn materials_of(counts: &BTreeMap<String, u64>) -> Vec<ScriptStructureMaterial> {
    counts
        .iter()
        .filter(|(_, quantity)| **quantity > 0)
        .take(MAX_STRUCTURE_RESOURCE_TYPES)
        .map(|(resource, quantity)| ScriptStructureMaterial::new(resource.clone(), *quantity))
        .collect()
}

fn bounds_of(origin: [i32; 3], size: [i32; 3]) -> ScriptSurveyBounds {
    // A structure footprint is bounded by the per-building blueprint axis, which
    // is well inside one survey bounds axis, so the constructor cannot fail.
    ScriptSurveyBounds::new(
        origin,
        [
            origin[0].saturating_add(size[0] - 1),
            origin[1].saturating_add(size[1] - 1),
            origin[2].saturating_add(size[2] - 1),
        ],
    )
    .expect("a bounded structure footprint fits one survey")
}

fn bounds_overlap(left: &ScriptSurveyBounds, right: &ScriptSurveyBounds) -> bool {
    (0..3).all(|axis| left.min[axis] <= right.max[axis] && right.min[axis] <= left.max[axis])
}

fn structure_snapshot(
    storage: &PluginStorage,
    structure: &DurableStructure,
) -> ScriptStructureSnapshot {
    // `remaining` is the material this structure may still spend. A terminal
    // structure has none: a committed one spent its whole plan, and a cancelled
    // one returned the rest. An active structure with no reservation yet reports
    // what its plan still needs rather than claiming unset-aside material.
    let remaining = match structure
        .reservation_ref()
        .and_then(|reference| storage.settlement_reservation(&structure.plugin_id, reference))
    {
        Some((_, reservation)) if structure.is_active() => reservation
            .quantities
            .iter()
            .map(|quantity| (quantity.resource_id.clone(), quantity.remaining))
            .collect::<BTreeMap<_, _>>(),
        Some(_) => BTreeMap::new(),
        None if structure.is_active() => planned_remaining(structure),
        None => BTreeMap::new(),
    };
    ScriptStructureSnapshot::new(
        structure.structure_id.clone(),
        structure.blueprint_id.clone(),
        structure.site_id.clone(),
        structure.state,
        structure.revision,
        structure.origin,
        structure.rotation,
        structure.reserved_footprint,
        structure.stages.clone(),
        structure.resource_plan_hash.clone(),
        structure.reservation_ref.clone(),
        structure.watermark,
        materials_of(&structure.consumed),
        materials_of(&remaining),
        structure.pause_reason.clone(),
    )
}

fn planned_remaining(structure: &DurableStructure) -> BTreeMap<String, u64> {
    let mut remaining: BTreeMap<String, u64> = BTreeMap::new();
    for stage in &structure.stages {
        for material in &stage.materials {
            *remaining.entry(material.resource.clone()).or_insert(0) += material.quantity;
        }
    }
    for (resource, consumed) in &structure.consumed {
        let entry = remaining.entry(resource.clone()).or_insert(0);
        *entry = entry.saturating_sub(*consumed);
    }
    remaining.retain(|_, quantity| *quantity > 0);
    remaining
}

/// Owner-scoped, bounded survey token. The storage transaction is mixed in, so
/// two identical surveys never share a token.
fn survey_token(
    plugin_id: &str,
    transaction_id: u64,
    dimension: &str,
    bounds: ScriptSurveyBounds,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SETTLEMENT_DOMAIN);
    hasher.update(b"survey");
    hasher.update((plugin_id.len() as u32).to_le_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(transaction_id.to_le_bytes());
    hasher.update((dimension.len() as u32).to_le_bytes());
    hasher.update(dimension.as_bytes());
    for value in bounds.min.iter().chain(bounds.max.iter()) {
        hasher.update(value.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Durable structure identity: unique per committed transaction, stable for the
/// lifetime of the record.
fn mint_structure_id(
    plugin_id: &str,
    transaction_id: u64,
    blueprint_id: &str,
    anchor: [i32; 3],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(SETTLEMENT_DOMAIN);
    hasher.update(b"structure");
    hasher.update((plugin_id.len() as u32).to_le_bytes());
    hasher.update(plugin_id.as_bytes());
    hasher.update(transaction_id.to_le_bytes());
    hasher.update((blueprint_id.len() as u32).to_le_bytes());
    hasher.update(blueprint_id.as_bytes());
    for value in anchor {
        hasher.update(value.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn encode_cursor(cell: [i32; 2]) -> String {
    format!("{}:{}", cell[0], cell[1])
}

fn parse_cursor(cursor: &str) -> Option<[i32; 2]> {
    let (x, z) = cursor.split_once(':')?;
    if z.contains(':') {
        return None;
    }
    let cell = [x.parse::<i32>().ok()?, z.parse::<i32>().ok()?];
    // Discovery walks forward from the cursor; refuse a cursor whose scan would
    // overflow rather than let the generator panic.
    if cell[0] > i32::MAX - MAX_SETTLEMENT_SITE_PAGE as i32
        || cell[1] > i32::MAX - MAX_SETTLEMENT_SITE_PAGE as i32
    {
        return None;
    }
    Some(cell)
}

/// The next cell in the selector's row-major scan order, matching
/// [`mc_worldgen::SettlementSelector::discover`].
fn scan_cell(start: [i32; 2], index: usize) -> [i32; 2] {
    [
        start[0] + (index % MAX_SETTLEMENT_SITE_PAGE) as i32,
        start[1] + (index / MAX_SETTLEMENT_SITE_PAGE) as i32,
    ]
}
