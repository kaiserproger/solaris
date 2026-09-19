//! Durable resident order/work/equipment ledger for contract task C4.
//!
//! The ledger is the plugin-journal projection that makes one accepted
//! `assign_resident_work`, `issue_resident_order`, `cancel_resident_order` or
//! `demobilize_resident` survive a restart exactly once. It duplicates no
//! authority: C3's resident ledger still owns identity, the regional entity
//! owner owns health and pose, C1 owns item movement, and this ledger owns the
//! per-handle order revision, the accepted order/work payload, the canonical
//! resident gear slots and the durable group-admission record that linearises a
//! squad batch.
//!
//! Every mutation rides in the same `PreparedStorageBatch` frame as the
//! operation receipt (or as a standalone `OP_RESIDENT_ORDER_CHANGE` frame for
//! core-observed transitions), so a replayed journal rebuilds the same state,
//! and a crash can never leave an accepted order applied without its durable
//! record.

use std::collections::{BTreeMap, BTreeSet};

use mc_entity::{
    FormationKind, FormationPlacement, FormationSlots, GroupMemberFence, RegionKey, TargetCategory,
    TargetPolicy, Vec3,
};
use mc_script::{
    MAX_RESIDENT_CARRY_SLOTS, MAX_RESIDENT_EQUIPMENT_SLOTS, MAX_SCRIPT_WORLD_TIME,
    ScriptAxisAlignedZone, ScriptHostileCategory, ScriptInventoryEndpoint, ScriptItemChange,
    ScriptResidentOrder, ScriptResidentWorkOrder, ScriptWorkPauseReason, ScriptWorkState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{PluginStorage, PluginStorageMutationError, PluginStorageStartError};

/// Side length of one active resident-work routing region in blocks.
pub(super) const RESIDENT_WORK_REGION_BLOCKS: i32 = 8 * 16;
/// Bounded number of live server-issued target references one plugin may hold.
pub(super) const MAX_RESIDENT_TARGET_REFS: usize = 64;
/// Bounded number of durable group admissions kept for replay.
pub(super) const MAX_RESIDENT_ADMISSIONS: usize = 4096;
/// Durable lifetime of a server-issued target reference, in storage revisions.
pub(super) const TARGET_REF_TTL_REVISIONS: u64 = 4096;
/// Bounded number of live garrison slot claims one batch resolution reads.
pub(super) const MAX_GARRISON_CLAIMS: usize = 4096;
/// Bounded length of one persisted resident custom-name component.
pub(super) const MAX_RESIDENT_CUSTOM_NAME_BYTES: usize = 256;

/// One canonical item stack of a resident's equipment or carry inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentStack {
    pub(super) item_id: String,
    pub(super) count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) damage: Option<i32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) enchantments: Vec<DurableResidentEnchantment>,
    /// Custom display name component, preserved across a C1 transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) custom_name: Option<String>,
    /// Item model component, preserved across a C1 transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) item_model: Option<String>,
}

impl DurableResidentStack {
    pub(super) fn new(item_id: String, count: u32) -> Self {
        Self {
            item_id,
            count,
            damage: None,
            enchantments: Vec::new(),
            custom_name: None,
            item_model: None,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        if !is_resource_id(&self.item_id)
            || self.count == 0
            || self.damage.is_some_and(|damage| damage < 0)
            || self.enchantments.len() > 16
            || self
                .item_model
                .as_deref()
                .is_some_and(|model| !is_resource_id(model))
            || self
                .custom_name
                .as_deref()
                .is_some_and(|name| name.len() > MAX_RESIDENT_CUSTOM_NAME_BYTES)
        {
            return Err(PluginStorageStartError::Malformed("resident stack"));
        }
        for enchantment in &self.enchantments {
            if !is_resource_id(&enchantment.id) || enchantment.level == 0 {
                return Err(PluginStorageStartError::Malformed(
                    "resident stack enchantment",
                ));
            }
        }
        Ok(())
    }
}

/// One enchantment of a durable resident stack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentEnchantment {
    pub(super) id: String,
    pub(super) level: u8,
}

/// Durable civilian/military assignment of one resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DurableAssignment {
    Civilian,
    Military,
    Demobilizing,
}

/// One occupied approved guard post of a garrison order.
///
/// The post is a C2 site point-of-interest handle and the slot is the
/// engine-computed position index inside that post's capacity. It is durable so
/// a reload reconstructs the same post and a later garrison batch never
/// double-books an occupied slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableGarrisonSlot {
    pub(super) post: String,
    pub(super) slot: u16,
}

/// One durable accepted squad order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentOrder {
    pub(super) revision: u64,
    pub(super) order: ScriptResidentOrder,
    /// Current index into a patrol route; advanced once per order application.
    #[serde(default)]
    pub(super) route_index: u16,
    /// Occupied guard post of a garrison order, when one was assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) garrison: Option<DurableGarrisonSlot>,
}

/// One durable work assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentWork {
    pub(super) revision: u64,
    pub(super) work: ScriptResidentWorkOrder,
    pub(super) planned: u64,
    #[serde(default)]
    pub(super) done: u64,
    pub(super) state: ScriptWorkState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<mc_script::ScriptWorkPauseReason>,
}

/// One durable per-handle order, work, gear and assignment record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableResidentOrderRecord {
    pub(super) handle: String,
    pub(super) plugin_id: String,
    pub(super) entity_uuid: String,
    pub(super) assignment: DurableAssignment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) order: Option<Box<DurableResidentOrder>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) work: Option<Box<DurableResidentWork>>,
    /// Canonical equipment slots (main hand, off hand, head, chest, legs, feet).
    pub(super) equipment: Vec<Option<DurableResidentStack>>,
    /// Canonical carry slots.
    pub(super) carry: Vec<Option<DurableResidentStack>>,
    pub(super) revision: u64,
}

impl DurableResidentOrderRecord {
    /// New record with canonical empty gear for one resident.
    pub(super) fn empty(
        handle: String,
        plugin_id: String,
        entity_uuid: String,
        assignment: DurableAssignment,
    ) -> Self {
        Self {
            handle,
            plugin_id,
            entity_uuid,
            assignment,
            order: None,
            work: None,
            equipment: vec![None; usize::from(MAX_RESIDENT_EQUIPMENT_SLOTS)],
            carry: vec![None; usize::from(MAX_RESIDENT_CARRY_SLOTS)],
            revision: 0,
        }
    }

    pub(super) const fn order_revision(&self) -> u64 {
        match &self.order {
            Some(order) => order.revision,
            None => 0,
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        validate_resident_order_field(&self.handle, "handle")?;
        validate_resident_order_field(&self.plugin_id, "owner")?;
        if Uuid::parse_str(&self.entity_uuid).is_err()
            || self.equipment.len() != usize::from(MAX_RESIDENT_EQUIPMENT_SLOTS)
            || self.carry.len() != usize::from(MAX_RESIDENT_CARRY_SLOTS)
            || self.revision == 0
            || self.revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(PluginStorageStartError::Malformed("resident order record"));
        }
        for stack in self.equipment.iter().chain(&self.carry).flatten() {
            stack.validate()?;
        }
        if let Some(order) = &self.order
            && (order.revision == 0
                || order.revision > MAX_SCRIPT_WORLD_TIME
                || order.revision > self.revision
                || order.order.validate().is_err())
        {
            return Err(PluginStorageStartError::Malformed("resident order"));
        }
        if let Some(garrison) = self
            .order
            .as_ref()
            .and_then(|order| order.garrison.as_ref())
        {
            validate_resident_order_field(&garrison.post, "garrison post")?;
            if garrison.slot >= mc_script::MAX_RESIDENT_ORDER_HANDLES as u16 {
                return Err(PluginStorageStartError::Malformed("garrison slot"));
            }
        }
        if let Some(work) = &self.work
            && (work.revision == 0
                || work.revision > MAX_SCRIPT_WORLD_TIME
                || work.revision > self.revision
                || work.planned == 0
                || work.done > work.planned
                || work.work.validate().is_err())
        {
            return Err(PluginStorageStartError::Malformed("resident work"));
        }
        Ok(())
    }
}

/// One member fence of a durable group admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableMemberFence {
    pub(super) handle: String,
    pub(super) entity_uuid: String,
    pub(super) region: [i32; 2],
    pub(super) order_revision: u64,
}

/// One durable group-admission record.
///
/// `committed` is the linearisation point: before it (including a crash between
/// prepare and commit) no member order changes exist, after it every accepted
/// member applies exactly once on replay. `applied` records which owners have
/// already pushed the new order into the entity owner, so a replay pushes only
/// the remainder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableAdmission {
    pub(super) admission_id: u64,
    pub(super) plugin_id: String,
    pub(super) operation_id: String,
    pub(super) fingerprint: [u8; 32],
    pub(super) order_revision: u64,
    pub(super) committed: bool,
    pub(super) members: Vec<DurableMemberFence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) applied: Vec<String>,
}

impl DurableAdmission {
    /// Members whose new order has not been pushed yet.
    pub(super) fn pending(&self) -> Vec<String> {
        let applied = self
            .applied
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        self.members
            .iter()
            .map(|member| member.handle.clone())
            .filter(|handle| !applied.contains(handle.as_str()))
            .collect()
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        validate_resident_order_field(&self.plugin_id, "admission owner")?;
        if !is_script_id(&self.operation_id)
            || self.admission_id == 0
            || self.admission_id > MAX_SCRIPT_WORLD_TIME
            || self.order_revision == 0
            || self.order_revision > MAX_SCRIPT_WORLD_TIME
            || self.members.is_empty()
            || self.members.len() > mc_script::MAX_RESIDENT_ORDER_HANDLES
            || self.applied.len() > self.members.len()
        {
            return Err(PluginStorageStartError::Malformed("group admission"));
        }
        let mut handles = BTreeSet::new();
        for member in &self.members {
            validate_resident_order_field(&member.handle, "admission member")?;
            if Uuid::parse_str(&member.entity_uuid).is_err()
                || member.order_revision > MAX_SCRIPT_WORLD_TIME
                || !handles.insert(member.handle.as_str())
            {
                return Err(PluginStorageStartError::Malformed("group admission member"));
            }
        }
        for handle in &self.applied {
            if !handles.contains(handle.as_str()) {
                return Err(PluginStorageStartError::Malformed(
                    "group admission applied member",
                ));
            }
        }
        Ok(())
    }
}

/// One server-issued target reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DurableTargetRef {
    pub(super) plugin_id: String,
    pub(super) target_ref: String,
    pub(super) entity_uuid: String,
    pub(super) category: String,
    pub(super) policy_revision: u64,
    /// Frame revision that issued this reference.
    pub(super) revision: u64,
    pub(super) expires_revision: u64,
}

impl DurableTargetRef {
    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        validate_resident_order_field(&self.plugin_id, "target owner")?;
        if self.target_ref.is_empty()
            || self.target_ref.len() > mc_script::MAX_TARGET_REF_BYTES
            || Uuid::parse_str(&self.entity_uuid).is_err()
            || !matches!(
                self.category.as_str(),
                "hostile" | "player" | "owned_resident" | "neutral_animal"
            )
            || self.policy_revision > MAX_SCRIPT_WORLD_TIME
            || self.revision == 0
            || self.revision > MAX_SCRIPT_WORLD_TIME
            || self.expires_revision > MAX_SCRIPT_WORLD_TIME
        {
            return Err(PluginStorageStartError::Malformed("target reference"));
        }
        Ok(())
    }
}

/// One durable ledger change applied inside a storage frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DurableResidentOrderChange {
    Record {
        record: Box<DurableResidentOrderRecord>,
    },
    Admission {
        admission: Box<DurableAdmission>,
    },
    AdmissionApplied {
        /// Frame revision of this acknowledgement, always the batch transaction.
        revision: u64,
        admission_id: u64,
        members: Vec<String>,
    },
    TargetRef {
        /// Frame revision of this reference, always the batch transaction.
        revision: u64,
        reference: Box<DurableTargetRef>,
    },
}

impl DurableResidentOrderChange {
    pub(super) const fn revision(&self) -> u64 {
        match self {
            Self::Record { record } => record.revision,
            Self::Admission { admission } => admission.admission_id,
            Self::AdmissionApplied { revision, .. } | Self::TargetRef { revision, .. } => *revision,
        }
    }

    pub(super) fn set_revision(&mut self, revision: u64) {
        match self {
            Self::Record { record } => {
                record.revision = revision;
                // The nested order and work revisions are the fences callers
                // send back, so they are stamped with the same transaction.
                if let Some(order) = &mut record.order {
                    order.revision = revision;
                }
                if let Some(work) = &mut record.work {
                    work.revision = revision;
                }
            }
            Self::Admission { admission } => {
                admission.admission_id = revision;
                admission.order_revision = revision;
            }
            Self::AdmissionApplied {
                revision: value, ..
            } => *value = revision,
            Self::TargetRef {
                reference,
                revision: value,
            } => {
                *value = revision;
                reference.revision = revision;
            }
        }
    }

    pub(super) fn validate(&self) -> Result<(), PluginStorageStartError> {
        match self {
            Self::Record { record } => record.validate(),
            Self::Admission { admission } => admission.validate(),
            Self::AdmissionApplied {
                revision,
                admission_id,
                members,
            } => {
                if *revision == 0
                    || *revision > MAX_SCRIPT_WORLD_TIME
                    || *admission_id == 0
                    || members.is_empty()
                    || members.len() > mc_script::MAX_RESIDENT_ORDER_HANDLES
                {
                    return Err(PluginStorageStartError::Malformed(
                        "admission applied change",
                    ));
                }
                Ok(())
            }
            Self::TargetRef {
                revision,
                reference,
            } => {
                if *revision == 0 || *revision > MAX_SCRIPT_WORLD_TIME {
                    return Err(PluginStorageStartError::Malformed("target ref revision"));
                }
                reference.validate()
            }
        }
    }
}

/// Core-owned resident order ledger: one record per handle plus the accepted
/// admissions and the live server-issued target references.
#[derive(Debug, Default)]
pub(super) struct ResidentOrderLedger {
    records: BTreeMap<String, DurableResidentOrderRecord>,
    /// Paused world work by active 8×8-chunk region. This is an event index,
    /// never a settlement-wide scheduler scan.
    paused_by_region: BTreeMap<(String, i32, i32), BTreeSet<String>>,
    /// Paused haul assignments blocked on an exact warehouse destination.
    paused_haul_by_warehouse: BTreeMap<ScriptInventoryEndpoint, BTreeSet<String>>,
    admissions: BTreeMap<u64, DurableAdmission>,
    references: BTreeMap<(String, String), DurableTargetRef>,
}

impl ResidentOrderLedger {
    pub(super) fn record(&self, handle: &str) -> Option<&DurableResidentOrderRecord> {
        self.records.get(handle)
    }

    pub(super) fn reference(&self, plugin_id: &str, target_ref: &str) -> Option<&DurableTargetRef> {
        self.references
            .get(&(plugin_id.to_owned(), target_ref.to_owned()))
    }

    /// Every guard-post slot this plugin's live garrison orders occupy, as
    /// `(post, slot, handle)`. Bounded by [`MAX_GARRISON_CLAIMS`] so a batch
    /// resolution never scans an unbounded ledger.
    pub(super) fn garrison_claims(&self, plugin_id: &str) -> Vec<(String, u16, String)> {
        let mut claims = Vec::new();
        for record in self.records.values() {
            if record.plugin_id != plugin_id {
                continue;
            }
            let Some(garrison) = record
                .order
                .as_ref()
                .and_then(|order| order.garrison.as_ref())
            else {
                continue;
            };
            claims.push((garrison.post.clone(), garrison.slot, record.handle.clone()));
            if claims.len() >= MAX_GARRISON_CLAIMS {
                break;
            }
        }
        claims
    }

    /// Paused assignments whose bounded world target overlaps one changed
    /// chunk. The index makes a world wake proportional to active work in that
    /// region, not to all residents or containers.
    pub(super) fn paused_records_for_chunks(
        &self,
        dimension: &str,
        chunks: &[[i32; 2]],
    ) -> Vec<DurableResidentOrderRecord> {
        let mut handles = BTreeSet::new();
        for [chunk_x, chunk_z] in chunks {
            let region_x = (chunk_x * 16).div_euclid(RESIDENT_WORK_REGION_BLOCKS);
            let region_z = (chunk_z * 16).div_euclid(RESIDENT_WORK_REGION_BLOCKS);
            if let Some(indexed) =
                self.paused_by_region
                    .get(&(dimension.to_owned(), region_x, region_z))
            {
                handles.extend(indexed.iter().cloned());
            }
        }
        handles
            .into_iter()
            .filter_map(|handle| self.records.get(&handle))
            .filter(|record| {
                record.work.as_ref().is_some_and(|work| {
                    work.state == ScriptWorkState::Paused
                        && matches!(
                            work.reason,
                            Some(
                                ScriptWorkPauseReason::Unloaded
                                    | ScriptWorkPauseReason::MissingStation
                                    | ScriptWorkPauseReason::Protected
                                    | ScriptWorkPauseReason::BlockedRoute
                            )
                        )
                })
            })
            .cloned()
            .collect()
    }

    /// A successful protection-definition mutation wakes only paused work
    /// regions that geometrically overlap its old or new zone, never a tick
    /// scan of settlements or containers.
    pub(super) fn paused_records_for_zone(
        &self,
        zone: &ScriptAxisAlignedZone,
    ) -> Vec<DurableResidentOrderRecord> {
        let minimum = zone.minimum();
        let maximum = zone.maximum();
        let handles = self
            .paused_by_region
            .iter()
            .filter(|((dimension, region_x, region_z), _)| {
                dimension == zone.dimension()
                    && f64::from(*region_x * RESIDENT_WORK_REGION_BLOCKS) <= maximum.x()
                    && f64::from((*region_x + 1) * RESIDENT_WORK_REGION_BLOCKS - 1) >= minimum.x()
                    && f64::from(*region_z * RESIDENT_WORK_REGION_BLOCKS) <= maximum.z()
                    && f64::from((*region_z + 1) * RESIDENT_WORK_REGION_BLOCKS - 1) >= minimum.z()
            })
            .flat_map(|(_, handles)| handles.iter().cloned())
            .collect::<BTreeSet<_>>();
        handles
            .into_iter()
            .filter_map(|handle| self.records.get(&handle))
            .filter(|record| {
                record.work.as_ref().is_some_and(|work| {
                    work.state == ScriptWorkState::Paused
                        && work.reason == Some(ScriptWorkPauseReason::Protected)
                })
            })
            .cloned()
            .collect()
    }

    /// Paused assignments affected by one accepted inventory transfer. The
    /// transfer names resident endpoints directly and warehouse waits use the
    /// durable exact-destination index; neither path scans the ledger.
    pub(super) fn paused_records_for_inventory_change(
        &self,
        resident_handles: &[String],
        endpoints: &[ScriptInventoryEndpoint],
    ) -> Vec<DurableResidentOrderRecord> {
        let mut handles = resident_handles.iter().cloned().collect::<BTreeSet<_>>();
        for endpoint in endpoints {
            if let Some(indexed) = self.paused_haul_by_warehouse.get(endpoint) {
                handles.extend(indexed.iter().cloned());
            }
        }
        handles
            .into_iter()
            .filter_map(|handle| self.records.get(&handle))
            .filter(|record| {
                record.work.as_ref().is_some_and(|work| {
                    work.state == ScriptWorkState::Paused
                        && matches!(
                            work.reason,
                            Some(
                                ScriptWorkPauseReason::MissingTool
                                    | ScriptWorkPauseReason::MissingInput
                                    | ScriptWorkPauseReason::NoStorage
                            )
                        )
                })
            })
            .cloned()
            .collect()
    }

    /// Committed admissions whose members still owe an engine-goal application.
    pub(super) fn pending_admissions(&self) -> Vec<(u64, Vec<String>)> {
        self.admissions
            .values()
            .filter(|admission| admission.committed)
            .map(|admission| (admission.admission_id, admission.pending()))
            .filter(|(_, pending)| !pending.is_empty())
            .collect()
    }

    pub(super) fn reset(&mut self) {
        self.records.clear();
        self.paused_by_region.clear();
        self.paused_haul_by_warehouse.clear();
        self.admissions.clear();
        self.references.clear();
    }

    /// Every live record first, then admissions and references. Ordering is
    /// stable so a compaction rewrites the same snapshot deterministically.
    pub(super) fn change_log(&self) -> Vec<DurableResidentOrderChange> {
        let mut changes = self
            .records
            .values()
            .map(|record| DurableResidentOrderChange::Record {
                record: Box::new(record.clone()),
            })
            .collect::<Vec<_>>();
        changes.extend(self.admissions.values().map(|admission| {
            DurableResidentOrderChange::Admission {
                admission: Box::new(admission.clone()),
            }
        }));
        changes.extend(self.references.values().map(|reference| {
            DurableResidentOrderChange::TargetRef {
                revision: reference.revision,
                reference: Box::new(reference.clone()),
            }
        }));
        changes
    }

    pub(super) fn apply(
        &mut self,
        change: &DurableResidentOrderChange,
    ) -> Result<(), PluginStorageStartError> {
        match change {
            DurableResidentOrderChange::Record { record } => {
                if let Some(existing) = self.records.get(&record.handle)
                    && existing.revision >= record.revision
                {
                    // Replaying an older or equal record is a no-op, never a
                    // rollback of a newer accepted order.
                    return Ok(());
                }
                if let Some(existing) = self.records.get(&record.handle).cloned() {
                    self.unindex_paused_work(&existing);
                }
                let record = (**record).clone();
                self.index_paused_work(&record);
                self.records.insert(record.handle.clone(), record);
            }
            DurableResidentOrderChange::Admission { admission } => {
                if !admission.committed {
                    return Ok(());
                }
                if self
                    .admissions
                    .get(&admission.admission_id)
                    .is_some_and(|existing| existing.committed)
                {
                    return Ok(());
                }
                self.admissions
                    .insert(admission.admission_id, (**admission).clone());
                while self.admissions.len() > MAX_RESIDENT_ADMISSIONS {
                    let Some(oldest) = self.admissions.keys().copied().next() else {
                        break;
                    };
                    self.admissions.remove(&oldest);
                }
            }
            DurableResidentOrderChange::AdmissionApplied {
                admission_id,
                members,
                ..
            } => {
                let Some(admission) = self.admissions.get_mut(admission_id) else {
                    return Err(PluginStorageStartError::Malformed(
                        "unknown admission application",
                    ));
                };
                for member in members {
                    if !admission.applied.contains(member) {
                        admission.applied.push(member.clone());
                    }
                }
            }
            DurableResidentOrderChange::TargetRef { reference, .. } => {
                self.references.insert(
                    (reference.plugin_id.clone(), reference.target_ref.clone()),
                    (**reference).clone(),
                );
                let owner_refs = self
                    .references
                    .keys()
                    .filter(|(owner, _)| owner == &reference.plugin_id)
                    .count();
                if owner_refs > MAX_RESIDENT_TARGET_REFS {
                    // Bound live references per plugin by dropping the ones that
                    // expire first; a dropped reference fails closed as stale.
                    let mut owned = self
                        .references
                        .iter()
                        .filter(|((owner, _), _)| owner == &reference.plugin_id)
                        .map(|((_, reference), value)| (reference.clone(), value.expires_revision))
                        .collect::<Vec<_>>();
                    owned.sort_unstable_by(|left, right| left.1.cmp(&right.1));
                    for (expired, _) in owned
                        .into_iter()
                        .take(owner_refs - MAX_RESIDENT_TARGET_REFS)
                    {
                        self.references
                            .remove(&(reference.plugin_id.clone(), expired));
                    }
                }
            }
        }
        Ok(())
    }
    fn index_paused_work(&mut self, record: &DurableResidentOrderRecord) {
        for (dimension, x, z) in paused_work_regions(record) {
            self.paused_by_region
                .entry((dimension, x, z))
                .or_default()
                .insert(record.handle.clone());
        }
        if let Some(destination) = paused_haul_warehouse(record) {
            self.paused_haul_by_warehouse
                .entry(destination)
                .or_default()
                .insert(record.handle.clone());
        }
    }

    fn unindex_paused_work(&mut self, record: &DurableResidentOrderRecord) {
        for (dimension, x, z) in paused_work_regions(record) {
            let key = (dimension, x, z);
            let remove = self.paused_by_region.get_mut(&key).is_some_and(|handles| {
                handles.remove(&record.handle);
                handles.is_empty()
            });
            if remove {
                self.paused_by_region.remove(&key);
            }
        }
        if let Some(destination) = paused_haul_warehouse(record) {
            let remove = self
                .paused_haul_by_warehouse
                .get_mut(&destination)
                .is_some_and(|handles| {
                    handles.remove(&record.handle);
                    handles.is_empty()
                });
            if remove {
                self.paused_haul_by_warehouse.remove(&destination);
            }
        }
    }
}

fn paused_haul_warehouse(record: &DurableResidentOrderRecord) -> Option<ScriptInventoryEndpoint> {
    let work = record.work.as_ref()?;
    if work.state != ScriptWorkState::Paused
        || work.reason != Some(ScriptWorkPauseReason::NoStorage)
    {
        return None;
    }
    match &work.work {
        ScriptResidentWorkOrder::Haul {
            destination: destination @ ScriptInventoryEndpoint::Warehouse { .. },
            ..
        } => Some(destination.clone()),
        _ => None,
    }
}

fn paused_work_regions(record: &DurableResidentOrderRecord) -> Vec<(String, i32, i32)> {
    let Some(work) = record
        .work
        .as_ref()
        .filter(|work| work.state == ScriptWorkState::Paused)
    else {
        return Vec::new();
    };
    let area = match &work.work {
        ScriptResidentWorkOrder::Harvest { area, .. }
        | ScriptResidentWorkOrder::Replant { area, .. }
        | ScriptResidentWorkOrder::CutTree { area, .. }
        | ScriptResidentWorkOrder::Mine { area, .. }
        | ScriptResidentWorkOrder::Fish { area, .. }
        | ScriptResidentWorkOrder::TendLivestock { area, .. } => area,
        ScriptResidentWorkOrder::Craft { station, .. } => station,
        ScriptResidentWorkOrder::Haul { .. } | ScriptResidentWorkOrder::Construct { .. } => {
            return Vec::new();
        }
        _ => return Vec::new(),
    };
    let min_x = area.min.x.div_euclid(RESIDENT_WORK_REGION_BLOCKS);
    let max_x = area.max.x.div_euclid(RESIDENT_WORK_REGION_BLOCKS);
    let min_z = area.min.z.div_euclid(RESIDENT_WORK_REGION_BLOCKS);
    let max_z = area.max.z.div_euclid(RESIDENT_WORK_REGION_BLOCKS);
    let mut regions = Vec::new();
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            regions.push((area.dimension.clone(), x, z));
        }
    }
    regions
}

pub(super) fn decode_resident_order_change(
    payload: &[u8],
) -> Result<DurableResidentOrderChange, PluginStorageStartError> {
    let change: DurableResidentOrderChange = serde_json::from_slice(payload)
        .map_err(|_| PluginStorageStartError::Malformed("resident order change"))?;
    change.validate()?;
    Ok(change)
}

/// The canonical fingerprint of one resident work/order operation: a repeated
/// operation id with the same canonical operation replays its stored result, a
/// different one conflicts.
pub(super) fn resident_order_fingerprint(operation: &mc_script::ScriptOperation) -> [u8; 32] {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, operation).expect("canonical script operation serializes");
    hash.finalize().into()
}

fn validate_resident_order_field(
    value: &str,
    field: &'static str,
) -> Result<(), PluginStorageStartError> {
    if value.is_empty() || value.len() > mc_script::MAX_PLUGIN_ID_BYTES {
        return Err(PluginStorageStartError::Malformed(field));
    }
    Ok(())
}

/// Canonical resource id check, mirroring the script contract so a malformed
/// persisted item id fails the journal closed instead of loading.
fn is_resource_id(value: &str) -> bool {
    if value.is_empty() || value.len() > mc_script::MAX_SCRIPT_RESOURCE_ID_BYTES {
        return false;
    }
    let Some((namespace, path)) = value.split_once(':') else {
        return false;
    };
    !namespace.is_empty()
        && !path.is_empty()
        && !path.contains(':')
        && namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
        && path.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.' | b'/')
        })
}

/// Bounded script id check for a persisted operation identity.
fn is_script_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= mc_script::MAX_SCRIPT_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

impl PluginStorage {
    pub(super) fn resident_orders(&self) -> &ResidentOrderLedger {
        &self.orders
    }

    /// The transaction id the next prepared batch will commit with.
    pub(super) fn next_transaction_id(&self) -> Result<u64, PluginStorageMutationError> {
        self.revision
            .checked_add(1)
            .ok_or(PluginStorageMutationError::RevisionOverflow)
    }

    /// Prepare one resident order receipt inside the plugin operation envelope.
    /// The durable ledger changes ride in the same frame as the receipt, so a
    /// replay either restores both or neither.
    pub(super) fn prepare_resident_order_operation_batch(
        &mut self,
        plugin_id: &str,
        request: &mc_script::ScriptOperationRequest,
        payload: mc_script::ScriptOperationPayload,
        mut changes: Vec<DurableResidentOrderChange>,
    ) -> Result<
        super::ScriptStoragePrepareOutcome<super::PreparedStorageBatch>,
        PluginStorageMutationError,
    > {
        let transaction_id = self.next_transaction_id()?;
        // The committed revision is the durable fence the caller sends back, so
        // the payload carries it and not a pre-commit placeholder.
        let mut payload = payload;
        if let mc_script::ScriptOperationPayload::ResidentOrder { result } = &mut payload {
            match &mut **result {
                mc_script::ScriptResidentOrderResult::Work { assignment } => {
                    assignment.revision = transaction_id;
                }
                mc_script::ScriptResidentOrderResult::Order { order_revision, .. }
                | mc_script::ScriptResidentOrderResult::OrderCancelled { order_revision, .. } => {
                    *order_revision = transaction_id;
                }
                mc_script::ScriptResidentOrderResult::WorkCancelled { revision, .. } => {
                    *revision = transaction_id;
                }
                mc_script::ScriptResidentOrderResult::Demobilized { resident } => {
                    resident.revision = transaction_id;
                }
                // The closed result union is non-exhaustive to plugins; the
                // durable revision is written for every variant this core owns.
                _ => {}
            }
        }
        let outcome = mc_script::ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        let receipt = super::operations::DurableOperationReceipt {
            plugin_id: plugin_id.to_owned(),
            operation_id: request
                .operation_id()
                .expect("resident order decision identity")
                .to_owned(),
            request_id: request.request_id().to_owned(),
            fingerprint: resident_order_fingerprint(request.operation()),
            revision: transaction_id,
            outcome,
            delivered: false,
        };
        Ok(super::ScriptStoragePrepareOutcome::Prepared(
            super::PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory: None,
                operation: Some(receipt),
                resident: Vec::new(),
                settlement: Vec::new(),
                order: changes,
            },
        ))
    }

    /// Prepare one combined resident-gear transfer batch: the C1 operation
    /// receipt, the resident order records whose canonical gear changed, and the
    /// player inventory after-image when a player endpoint participates. All
    /// three ride in one frame, so a replay restores both inventories or
    /// neither, and a crash can never leave an item at both endpoints.
    pub(super) fn prepare_resident_gear_batch(
        &mut self,
        plugin_id: &str,
        request: &mc_script::ScriptOperationRequest,
        payload: mc_script::ScriptOperationPayload,
        mut changes: Vec<DurableResidentOrderChange>,
        inventory: Option<crate::play::persistence::inventory_recovery::PlayerInventoryRecovery>,
    ) -> Result<
        super::ScriptStoragePrepareOutcome<super::PreparedStorageBatch>,
        PluginStorageMutationError,
    > {
        let transaction_id = self.next_transaction_id()?;
        let outcome = mc_script::ScriptOperationOutcome::committed(transaction_id, payload)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        for change in &mut changes {
            change.set_revision(transaction_id);
        }
        let receipt = super::operations::DurableOperationReceipt {
            plugin_id: plugin_id.to_owned(),
            operation_id: request
                .operation_id()
                .expect("resident gear decision identity")
                .to_owned(),
            request_id: request.request_id().to_owned(),
            fingerprint: crate::play::owned_inventory::owned_inventory_fingerprint(
                request.operation(),
            ),
            revision: transaction_id,
            outcome,
            delivered: false,
        };
        Ok(super::ScriptStoragePrepareOutcome::Prepared(
            super::PreparedStorageBatch {
                transaction_id,
                plugin_id: plugin_id.to_owned(),
                mutations: Vec::new(),
                inventory,
                operation: Some(receipt),
                resident: Vec::new(),
                settlement: Vec::new(),
                order: changes,
            },
        ))
    }

    /// Durably record one committed admission whose members still owe an engine
    /// goal push, exactly the state a crash between the group commit and its
    /// application leaves behind.
    #[cfg(test)]
    pub(super) fn force_pending_admission_for_test(
        &mut self,
        plugin_id: &str,
        operation_id: &str,
        records: &[DurableResidentOrderRecord],
    ) -> Result<u64, PluginStorageMutationError> {
        let order_revision = records
            .iter()
            .map(|record| record.order_revision())
            .max()
            .unwrap_or(1)
            .max(1);
        let admission = DurableAdmission {
            admission_id: 0,
            plugin_id: plugin_id.to_owned(),
            operation_id: operation_id.to_owned(),
            fingerprint: [0; 32],
            order_revision,
            committed: true,
            members: records
                .iter()
                .map(|record| DurableMemberFence {
                    handle: record.handle.clone(),
                    entity_uuid: record.entity_uuid.clone(),
                    region: [0, 0],
                    order_revision: record.order_revision(),
                })
                .collect(),
            applied: Vec::new(),
        };
        self.append_resident_order_change(DurableResidentOrderChange::Admission {
            admission: Box::new(admission),
        })
    }

    /// Apply one core-owned order ledger change without a plugin operation, for
    /// committed group-admission application acks. Revisions stay strictly
    /// monotonic with the journal.
    pub(super) fn append_resident_order_change(
        &mut self,
        mut change: DurableResidentOrderChange,
    ) -> Result<u64, PluginStorageMutationError> {
        let transaction_id = self.next_transaction_id()?;
        change.set_revision(transaction_id);
        change
            .validate()
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        let payload = serde_json::to_vec(&change)
            .map_err(|error| PluginStorageMutationError::Io(std::io::Error::other(error)))?;
        let mut frame_payload = Vec::with_capacity(payload.len() + 1);
        frame_payload.push(super::OP_RESIDENT_ORDER_CHANGE);
        frame_payload.extend_from_slice(&payload);
        if frame_payload.len() > super::MAX_FRAME_BYTES {
            return Err(PluginStorageMutationError::QuotaExceeded);
        }
        let frame = super::frame(&frame_payload);
        self.compact_before_append_if_needed(frame.len())?;
        self.append_frame(&frame, false)?;
        self.orders
            .apply(&change)
            .map_err(|_| PluginStorageMutationError::QuotaExceeded)?;
        self.revision = transaction_id;
        Ok(transaction_id)
    }
}

/// Engine formation kind for one script formation kind.
pub(super) fn engine_formation_kind(kind: mc_script::ScriptFormationKind) -> FormationKind {
    match kind {
        mc_script::ScriptFormationKind::Line => FormationKind::Line,
        mc_script::ScriptFormationKind::Column => FormationKind::Column,
        mc_script::ScriptFormationKind::Wedge => FormationKind::Wedge,
        mc_script::ScriptFormationKind::Square => FormationKind::Square,
        _ => FormationKind::Line,
    }
}

/// Engine target category for one script hostile category.
pub(super) fn engine_target_category(category: ScriptHostileCategory) -> TargetCategory {
    match category {
        ScriptHostileCategory::Hostile => TargetCategory::Hostile,
        ScriptHostileCategory::Player => TargetCategory::Player,
        ScriptHostileCategory::OwnedResident => TargetCategory::OwnedResident,
        ScriptHostileCategory::NeutralAnimal => TargetCategory::NeutralAnimal,
        _ => TargetCategory::Hostile,
    }
}

/// Build the engine target policy of one attack order.
pub(super) fn target_policy(
    policy: &mc_script::ScriptEngagementPolicy,
    engagement_radius: f64,
) -> Option<TargetPolicy> {
    TargetPolicy::new(
        policy.revision,
        engagement_radius,
        policy
            .allies
            .iter()
            .map(|ally| ally.as_str().to_owned())
            .collect::<Vec<_>>(),
        policy
            .permitted
            .iter()
            .copied()
            .map(engine_target_category)
            .collect::<Vec<_>>(),
    )
}

/// Compute one squad's engine formation and place its members into distinct
/// standable slots. A member that cannot be placed leaves the whole formation
/// `blocked`, never stacked on a shared coordinate.
pub(super) fn place_formation(
    kind: mc_script::ScriptFormation,
    anchor: Vec3,
    heading_degrees: Option<u16>,
    count: usize,
    dimension: &str,
    world: &dyn crate::play::resident_work::ResidentWorld,
) -> Option<FormationPlacement> {
    let heading = heading_degrees.map_or(0.0, |degrees| f64::from(degrees).to_radians());
    let slots = FormationSlots::compute(
        engine_formation_kind(kind.kind),
        anchor,
        heading,
        f64::from(kind.spacing) / 2.0,
        count,
    )?;
    Some(slots.place_all(count, |position| {
        world
            .standable(
                dimension,
                [
                    position.x.floor() as i32,
                    position.y.floor() as i32,
                    position.z.floor() as i32,
                ],
            )
            .unwrap_or(false)
    }))
}

/// Aggregate committed item changes per resource, dropping zero sums.
pub(super) fn item_changes(changes: &BTreeMap<String, i64>) -> Vec<ScriptItemChange> {
    changes
        .iter()
        .filter(|(_, delta)| **delta != 0)
        .map(|(item_id, delta)| ScriptItemChange::new(item_id.clone(), *delta))
        .collect()
}

/// Build the engine fences of one batch from the durable records.
pub(super) fn member_fence(
    handle: &str,
    record: &DurableResidentOrderRecord,
    region: RegionKey,
) -> GroupMemberFence<String> {
    GroupMemberFence {
        entity: handle.to_owned(),
        region,
        order_revision: record.order_revision(),
    }
}
