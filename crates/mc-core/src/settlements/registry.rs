//! Tick-owned records. Membership remains exclusively in villager columns.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::boundary::EntityId;

pub(crate) const MAX_RADIUS: u32 = 256;
/// Population growth rule: a settlement is at least `GROWTH_PER_RESIDENT *
/// residents` blocks wide, capped at `MAX_RADIUS`. The record's radius only
/// ever rises, so the created radius is a floor.
pub(crate) const GROWTH_PER_RESIDENT: u32 = 4;

/// Stable global identity, allocated monotonically for founded and generated towns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettlementId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettlementRecord {
    pub id: SettlementId,
    pub name: String,
    pub owner: Option<String>,
    /// Last authenticated display name; UUID `owner` remains the authority.
    #[serde(default)]
    pub owner_name: Option<String>,
    pub anchor: [i32; 3],
    pub radius: u32,
    pub revision: u64,
    #[serde(default)]
    pub household_sequence: u64,
    #[serde(default)]
    pub households: BTreeMap<String, super::households::Household>,
    #[serde(default)]
    pub building_sequence: u64,
    #[serde(default)]
    pub buildings: BTreeMap<super::buildings::BuildingId, super::buildings::Building>,
    #[serde(default)]
    pub district: Option<super::districts::DistrictPlan>,
    #[serde(default)]
    pub(crate) economy: super::economy::State,
    /// Stable worldgen provenance, retained after claim or rename.
    #[serde(default)]
    pub village_source: Option<String>,
    #[serde(default)]
    pub bootstrap_storage: Option<[i32; 3]>,
    #[serde(default)]
    pub project: Option<super::construction::Project>,
    /// One owner request, retried before autonomous planning.
    #[serde(default)]
    pub pending_order: Option<super::construction::OwnerOrder>,
    #[serde(default)]
    pub construction_cursor: u32,
}

/// One absolute post-image or tombstone, not an aggregate registry blob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordImage {
    pub id: SettlementId,
    pub value: Option<SettlementRecord>,
}

#[derive(Debug, Default)]
pub struct Registry {
    records: BTreeMap<SettlementId, SettlementRecord>,
    touched: BTreeSet<SettlementId>,
    high_water: u64,
    /// Derived and rebuilt after hydration; never serialized or journaled.
    counts: BTreeMap<SettlementId, usize>,
    /// Derived occupancy index: last counted settlement per entity.
    /// Hydrated by `rebuild_counts` alongside `counts`; never serialized.
    resident_settlement: BTreeMap<EntityId, SettlementId>,
    household_counts: BTreeMap<SettlementId, BTreeMap<String, usize>>,
    resident_household: BTreeMap<EntityId, (SettlementId, String)>,
    /// Global living population for locally owned households; rebuilt from
    /// every dimension's native columns, never saved in settlement rows.
    household_sizes: BTreeMap<String, usize>,
    /// Global-tail reconciliation for explicit household membership mutations.
    /// Runtime work only: never a second persisted roster.
    household_changes: BTreeSet<String>,
    /// Derived work queue: no idle-town or terminal-receipt tick scan.
    economic_active: BTreeSet<SettlementId>,
    /// Derived work queue: settlements whose resident count changed and whose
    /// radius must be re-checked against the growth rule.
    radius_pending: BTreeSet<SettlementId>,
}

impl SettlementRecord {
    /// Autonomous settlements must be claimed before even an operator may
    /// mutate them through player management commands or dialogs.
    pub(crate) fn can_manage(&self, actor: &str, operator: bool) -> bool {
        self.owner
            .as_deref()
            .is_some_and(|owner| owner == actor || operator)
    }

    pub(crate) fn contains(&self, at: [i32; 3]) -> bool {
        at.iter()
            .zip(self.anchor)
            .map(|(a, b)| (i64::from(*a) - i64::from(b)).pow(2))
            .sum::<i64>()
            <= i64::from(self.radius).pow(2)
    }
    /// Cheap overestimate of the heap this record owns, for the journal
    /// queue's byte budget: map element counts times element size, string
    /// capacities and item-stack heaps; fixed-size endpoint/fence records get
    /// a flat allowance. Never serializes.
    pub(crate) fn heap_estimate(&self) -> usize {
        use crate::journal_budget::{
            NESTED_STATE_ALLOWANCE as FIXED, stacks_heap, tree_root, tree_slot,
        };
        use mc_script::ScriptSettlementBuildingRef as Building;
        use std::mem::size_of;
        // Inline key+value bytes of a tree element, with node slack priced in.
        let key =
            |key: &String, value: usize| key.capacity() + tree_slot(size_of::<String>() + value);
        let opt = |text: &Option<String>| text.as_ref().map_or(0, String::capacity);
        let building = |building: &Building| {
            size_of::<Building>()
                + building.kind.capacity()
                + building.id.capacity()
                + building.purpose.capacity()
        };
        let households = self
            .households
            .iter()
            .map(|(id, household)| {
                key(id, size_of::<super::households::Household>())
                    + household.id.capacity()
                    + opt(&household.needs)
                    + opt(&household.policy)
            })
            .sum::<usize>();
        let district = self.district.as_ref().map_or(0, |plan| {
            plan.owner.capacity()
                + plan.branch.capacity()
                + plan.buildings.capacity() * size_of::<Building>()
                + plan.buildings.iter().map(building).sum::<usize>()
        });
        let jobs = self
            .economy
            .production
            .jobs
            .iter()
            .map(|(id, job)| {
                key(id, size_of::<super::production::Job>())
                    + job.id.capacity()
                    + job.owner.capacity()
                    + job.recipe.capacity()
                    + job.recipe_revision.capacity()
                    + job.dimension.capacity()
                    + opt(&job.paused)
                    + building(&job.building)
                    + job.receipts.capacity() * size_of::<super::production::Receipt>()
                    + job
                        .receipts
                        .iter()
                        .map(|receipt| {
                            receipt.job.capacity()
                                + receipt.recipe.capacity()
                                + receipt.recipe_revision.capacity()
                                + stacks_heap(&receipt.inputs)
                                + stacks_heap(&receipt.outputs)
                                + receipt.tool_wear.capacity()
                                    * size_of::<super::production::ToolWear>()
                                + receipt
                                    .tool_wear
                                    .iter()
                                    .map(|wear| wear.item.capacity())
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>();
        let commerce = &self.economy.commerce;
        let contracts = commerce
            .employments
            .keys()
            .map(|id| key(id, size_of::<super::commerce::Employment>()) + FIXED)
            .sum::<usize>()
            + commerce
                .quotes
                .iter()
                .map(|(id, quote)| {
                    key(id, size_of::<super::commerce::Quote>())
                        + crate::journal_budget::item_heap(&quote.goods)
                        + FIXED
                })
                .sum::<usize>()
            + commerce
                .mandates
                .iter()
                .map(|(id, mandate)| {
                    key(id, size_of::<super::commerce::EscrowMandate>())
                        + mandate.shipment.capacity()
                        + FIXED
                })
                .sum::<usize>();
        let shipments = self
            .economy
            .caravan
            .shipments
            .iter()
            .map(|(id, shipment)| {
                key(id, size_of::<super::caravan::Shipment>())
                    + shipment.id.capacity()
                    + shipment.owner.capacity()
                    + opt(&shipment.paid_mandate)
                    + opt(&shipment.blocked)
                    + crate::journal_budget::item_heap(&shipment.manifest)
                    + shipment.waypoints.capacity() * size_of::<[i32; 3]>()
                    + shipment.carry.capacity() * size_of::<super::caravan::CarryLot>()
                    // Two container refs, three inventory fences, three receipts.
                    + 4 * FIXED
            })
            .sum::<usize>();
        let buildings = self
            .buildings
            .values()
            .map(|building| {
                size_of::<super::buildings::Building>()
                    + building.work_points.capacity() * size_of::<[i32; 3]>()
            })
            .sum::<usize>();
        let root = |len: usize, value: usize| tree_root(len, size_of::<String>() + value);
        let roots = root(
            self.households.len(),
            size_of::<super::households::Household>(),
        ) + root(
            self.economy.production.jobs.len(),
            size_of::<super::production::Job>(),
        ) + root(
            commerce.employments.len(),
            size_of::<super::commerce::Employment>(),
        ) + root(commerce.quotes.len(), size_of::<super::commerce::Quote>())
            + root(
                commerce.mandates.len(),
                size_of::<super::commerce::EscrowMandate>(),
            )
            + root(
                self.economy.caravan.shipments.len(),
                size_of::<super::caravan::Shipment>(),
            )
            + root(
                self.buildings.len(),
                size_of::<super::buildings::Building>(),
            );
        self.name.capacity()
            + opt(&self.owner)
            + opt(&self.village_source)
            + opt(&self.owner_name)
            + self.project.as_ref().map_or(0, |project| {
                project.blueprint.capacity() + project.structure.capacity()
            })
            + self
                .pending_order
                .as_ref()
                .map_or(0, |order| order.blueprint.capacity())
            + roots
            + households
            + district
            + jobs
            + contracts
            + shipments
            + buildings
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

pub fn validate_record(record: &SettlementRecord) -> Result<(), &'static str> {
    if record.id.0 == 0 || record.revision == 0 {
        return Err("invalid settlement identity or revision");
    }
    if !valid_name(&record.name)
        || record
            .owner
            .as_ref()
            .is_some_and(|owner| owner.is_empty() || owner.len() > 128)
    {
        return Err("invalid settlement name or owner");
    }
    if !(1..=MAX_RADIUS).contains(&record.radius) {
        return Err("settlement radius must be in 1..=256");
    }
    if record
        .bootstrap_storage
        .is_some_and(|at| !record.contains(at))
    {
        return Err("bootstrap storage outside settlement radius");
    }
    super::construction::validate_record(record)?;
    super::households::validate_record(record)?;
    super::buildings::validate_record_buildings(record)?;
    if let Some(district) = &record.district {
        super::districts::validate(district)?;
    }
    super::economy::validate(&record.economy)?;
    Ok(())
}

impl Registry {
    pub fn records(&self) -> impl Iterator<Item = &SettlementRecord> {
        self.records.values()
    }
    pub(crate) fn refresh_owner_names<'a>(
        &mut self,
        name_of: impl Fn(uuid::Uuid) -> Option<&'a str>,
    ) {
        for record in self.records.values_mut() {
            let Some(owner) = record
                .owner
                .as_deref()
                .and_then(|owner| uuid::Uuid::parse_str(owner).ok())
            else {
                continue;
            };
            let Some(name) = name_of(owner) else {
                continue;
            };
            if record.owner_name.as_deref() != Some(name) {
                record.owner_name = Some(name.to_owned());
                // Display-only: do not invalidate economic/command revisions.
                self.touched.insert(record.id);
            }
        }
    }

    pub fn get(&self, id: SettlementId) -> Option<&SettlementRecord> {
        self.records.get(&id)
    }

    /// Mutation custody records a touched-only settlement post-image.
    pub(crate) fn get_mut(&mut self, id: SettlementId) -> Option<&mut SettlementRecord> {
        if !self.records.contains_key(&id) {
            return None;
        }
        self.touched.insert(id);
        self.records.get_mut(&id)
    }

    /// Temporary domain extraction is restored before finishing the mutation.
    /// Do not mark touched on reads, pauses, or idempotent command replays.
    pub(super) fn economy_for_tick(&mut self, id: SettlementId) -> &mut super::economy::State {
        &mut self
            .records
            .get_mut(&id)
            .expect("native economic settlement")
            .economy
    }

    pub(super) fn finish_economy(&mut self, id: SettlementId, revision: u64, changed: bool) {
        if !changed {
            return;
        }
        let record = self
            .records
            .get_mut(&id)
            .expect("native economic settlement");
        record.economy.refresh_routes();
        record.revision = revision;
        self.touched.insert(id);
        if super::economy::active(&record.economy) {
            self.economic_active.insert(id);
        } else {
            self.economic_active.remove(&id);
        }
    }

    pub(super) fn next_economy(&self, after: Option<SettlementId>) -> Option<SettlementId> {
        use std::ops::Bound::{Excluded, Unbounded};
        self.economic_active
            .range((after.map_or(Unbounded, Excluded), Unbounded))
            .next()
            .copied()
    }

    pub(super) fn queue_household_change(&mut self, household: String) {
        self.household_changes.insert(household);
    }

    pub(super) fn take_household_changes(&mut self) -> BTreeSet<String> {
        std::mem::take(&mut self.household_changes)
    }

    pub fn find(&self, name: &str) -> Option<&SettlementRecord> {
        self.records.values().find(|record| record.name == name)
    }

    pub(super) fn find_village(&self, source: &str) -> Option<&SettlementRecord> {
        self.records
            .values()
            .find(|record| record.village_source.as_deref() == Some(source))
    }

    pub fn high_water(&self) -> u64 {
        self.high_water
    }

    pub fn set_high_water(&mut self, value: u64) {
        self.high_water = self.high_water.max(value);
    }

    fn next_id(&self, sequence: u64) -> Result<SettlementId, &'static str> {
        sequence
            .max(self.high_water)
            .checked_add(1)
            .map(SettlementId)
            .ok_or("settlement identity exhausted")
    }

    pub fn create(
        &mut self,
        sequence: &mut u64,
        name: String,
        owner: String,
        anchor: [i32; 3],
        radius: u32,
    ) -> Result<SettlementId, &'static str> {
        if self.find(&name).is_some() {
            return Err("settlement name already exists");
        }
        let id = self.next_id(*sequence)?;
        let record = SettlementRecord {
            id,
            name,
            owner: Some(owner),
            owner_name: None,
            anchor,
            radius,
            revision: 1,
            household_sequence: 0,
            households: BTreeMap::new(),
            building_sequence: 0,
            buildings: BTreeMap::new(),
            district: None,
            economy: super::economy::State::default(),
            village_source: None,
            bootstrap_storage: None,
            project: None,
            pending_order: None,
            construction_cursor: 0,
        };
        validate_record(&record)?;
        self.records.insert(id, record);
        self.touched.insert(id);
        self.high_water = id.0;
        *sequence = id.0;
        Ok(id)
    }

    /// Reuse saved worldgen provenance across claim, rename and restart; new
    /// villages share the existing settlement sequence with founded towns.
    pub(crate) fn adopt_village(
        &mut self,
        sequence: &mut u64,
        source: &str,
        anchor: [i32; 3],
    ) -> Result<SettlementId, &'static str> {
        if let Some(record) = self.find_village(source) {
            return Ok(record.id);
        }
        let id = self.next_id(*sequence)?;
        let name = format!("village_{:08x}", id.0);
        if self.find(&name).is_some() {
            return Err("generated village name already exists");
        }
        let record = SettlementRecord {
            id,
            name,
            owner: None,
            owner_name: None,
            anchor,
            radius: super::commands::DEFAULT_RADIUS,
            revision: 1,
            household_sequence: 0,
            households: BTreeMap::new(),
            building_sequence: 0,
            buildings: BTreeMap::new(),
            district: None,
            economy: super::economy::State::default(),
            village_source: Some(source.to_owned()),
            bootstrap_storage: None,
            project: None,
            pending_order: None,
            construction_cursor: 0,
        };
        validate_record(&record)?;
        self.records.insert(id, record);
        self.touched.insert(id);
        self.high_water = id.0;
        *sequence = id.0;
        Ok(id)
    }

    pub(crate) fn claim(
        &mut self,
        id: SettlementId,
        owner: String,
        at: [i32; 3],
    ) -> Result<(), &'static str> {
        if owner.is_empty() || owner.len() > 128 {
            return Err("invalid settlement owner");
        }
        let record = self.records.get_mut(&id).ok_or("settlement not found")?;
        if record.owner.is_some() {
            return Err("settlement already has an owner");
        }
        if !record.contains(at) {
            return Err("stand inside the settlement to claim it");
        }
        let revision = record
            .revision
            .checked_add(1)
            .ok_or("settlement revision exhausted")?;
        record.owner = Some(owner);
        record.owner_name = None;
        record.revision = revision;
        self.touched.insert(id);
        Ok(())
    }

    pub fn rename(&mut self, id: SettlementId, name: String) -> Result<(), &'static str> {
        if !valid_name(&name) {
            return Err("invalid settlement name");
        }
        if self.find(&name).is_some_and(|record| record.id != id) {
            return Err("settlement name already exists");
        }
        let record = self.records.get_mut(&id).ok_or("settlement not found")?;
        if record.name == name {
            return Ok(());
        }
        let revision = record
            .revision
            .checked_add(1)
            .ok_or("settlement revision exhausted")?;
        record.name = name;
        record.revision = revision;
        self.touched.insert(id);
        Ok(())
    }

    pub fn remove(&mut self, id: SettlementId) -> bool {
        let Some(record) = self.records.remove(&id) else {
            return false;
        };
        for household in record.households.keys() {
            self.household_sizes.remove(household);
        }
        self.counts.remove(&id);
        self.economic_active.remove(&id);
        self.resident_settlement.retain(|_, sid| *sid != id);
        self.household_counts.remove(&id);
        self.resident_household.retain(|_, (sid, _)| *sid != id);
        self.touched.insert(id);
        true
    }

    pub fn take_images(&mut self) -> Vec<RecordImage> {
        std::mem::take(&mut self.touched)
            .into_iter()
            .map(|id| RecordImage {
                id,
                value: self.records.get(&id).cloned(),
            })
            .collect()
    }

    pub fn images(&self) -> Vec<RecordImage> {
        self.records
            .values()
            .map(|record| RecordImage {
                id: record.id,
                value: Some(record.clone()),
            })
            .collect()
    }

    pub(super) fn removed_ids(&self) -> impl Iterator<Item = SettlementId> + '_ {
        self.touched
            .iter()
            .copied()
            .filter(|id| !self.records.contains_key(id))
    }

    /// The persistence boundary validates the row before applying it.
    pub fn apply_image(&mut self, image: RecordImage) {
        self.high_water = self.high_water.max(image.id.0);
        if let Some(mut record) = image.value {
            record.economy.refresh_routes();
            if super::economy::active(&record.economy) {
                self.economic_active.insert(image.id);
            } else {
                self.economic_active.remove(&image.id);
            }
            self.records.insert(image.id, record);
        } else {
            self.economic_active.remove(&image.id);
            if let Some(record) = self.records.remove(&image.id) {
                for household in record.households.keys() {
                    self.household_sizes.remove(household);
                }
            }
            self.counts.remove(&image.id);
            self.resident_settlement.retain(|_, sid| *sid != image.id);
            self.household_counts.remove(&image.id);
            self.resident_household
                .retain(|_, (sid, _)| *sid != image.id);
        }
    }
    pub fn member_count(&self, id: SettlementId) -> usize {
        self.counts.get(&id).copied().unwrap_or(0)
    }

    pub(super) fn clear_counts(&mut self) {
        for count in self.counts.values_mut() {
            *count = 0;
        }
        self.resident_settlement.clear();
        self.household_counts.clear();
        self.resident_household.clear();
    }

    pub(super) fn count_member(&mut self, id: SettlementId) {
        *self.counts.entry(id).or_default() += 1;
        self.radius_pending.insert(id);
    }

    /// Raise radii whose resident count outgrew them; persisted through the
    /// normal record image like every other record change.
    pub(super) fn grow_radii(&mut self) {
        while let Some(id) = self.radius_pending.pop_first() {
            let residents = u32::try_from(self.member_count(id)).unwrap_or(u32::MAX);
            let wanted = GROWTH_PER_RESIDENT
                .saturating_mul(residents)
                .min(MAX_RADIUS);
            if let Some(record) = self.records.get_mut(&id)
                && record.radius < wanted
            {
                record.radius = wanted;
                record.revision = record.revision.saturating_add(1);
                self.touched.insert(id);
            }
        }
    }

    /// Change the occupancy projection only when this entity's share changes.
    pub(super) fn update_resident(&mut self, entity: EntityId, new: Option<SettlementId>) {
        let old = self.resident_settlement.get(&entity).copied();
        if old == new {
            return;
        }
        if let Some(id) = old {
            self.resident_settlement.remove(&entity);
            if let Some(count) = self.counts.get_mut(&id) {
                *count -= 1;
            }
        }
        if let Some(id) = new {
            self.count_member(id);
            self.resident_settlement.insert(entity, id);
        }
    }

    pub(crate) fn household_count(&self, settlement: SettlementId, household: &str) -> usize {
        self.household_counts
            .get(&settlement)
            .and_then(|counts| counts.get(household))
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn household_size(&self, household: &str) -> usize {
        self.household_sizes.get(household).copied().unwrap_or(0)
    }

    pub(super) fn set_household_size(&mut self, household: &str, size: usize) {
        if size == 0 {
            self.household_sizes.remove(household);
        } else if let Some(current) = self.household_sizes.get_mut(household) {
            *current = size;
        } else {
            self.household_sizes.insert(household.to_owned(), size);
        }
    }

    pub(super) fn rebuild_household_sizes(&mut self, sizes: &BTreeMap<String, usize>) {
        self.household_sizes.clear();
        for record in self.records.values() {
            for household in record.households.keys() {
                if let Some(&size) = sizes.get(household)
                    && size > 0
                {
                    self.household_sizes.insert(household.clone(), size);
                }
            }
        }
    }

    pub(super) fn update_household(
        &mut self,
        entity: EntityId,
        new: Option<(SettlementId, &str)>,
        notify: bool,
    ) {
        if self
            .resident_household
            .get(&entity)
            .map(|(id, family)| (*id, family.as_str()))
            == new
        {
            return;
        }
        if let Some((settlement, household)) = self.resident_household.remove(&entity) {
            if let Some(count) = self
                .household_counts
                .get_mut(&settlement)
                .and_then(|counts| counts.get_mut(&household))
            {
                *count -= 1;
            }
            if notify {
                self.household_changes.insert(household);
            }
        }
        if let Some((settlement, household)) = new {
            *self
                .household_counts
                .entry(settlement)
                .or_default()
                .entry(household.to_owned())
                .or_default() += 1;
            self.resident_household
                .insert(entity, (settlement, household.to_owned()));
            if notify && !self.household_changes.contains(household) {
                self.household_changes.insert(household.to_owned());
            }
        }
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
