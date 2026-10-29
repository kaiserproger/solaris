//! Settlement sources for the canonical structure ledger and executor. This
//! adapter owns neither a placement cursor nor a second placement loop.
use super::*;
use crate::settlements::{SettlementId, inventory};

pub(crate) fn owner(id: SettlementId) -> String {
    format!("@settlement:{}", id.0)
}

pub(crate) struct PlannedCell {
    pub at: [i32; 3],
    pub state: u32,
    pub resource: Option<String>,
    pub quantity: u32,
}

pub(crate) fn prepare(
    state: &mut TickState,
    settlement: SettlementId,
    blueprint: String,
    bounds: ScriptWorldBounds,
    cells: Vec<PlannedCell>,
) -> Result<String, Failure> {
    if state.settlements.get(settlement).is_none() || cells.is_empty() || cells.len() > 32768 {
        return Err(Failure::InvalidRequest);
    }
    let mut ledger = ledger(state)?;
    if ledger.structures.values().any(|record| {
        record.snapshot.dimension == state.dimension.name()
            && record.snapshot.state != ScriptStructureState::Cancelled
            && bounds_of_structure(record).is_ok_and(|existing| overlaps(existing, bounds))
            && !(record.settlement == Some(settlement)
                && same_site(state, record, bounds.min)
                && record
                    .snapshot
                    .blueprint_id
                    .strip_prefix("solaris-settlements:")
                    == Some(blueprint.as_str()))
    }) {
        return Err(Failure::Blocked);
    }
    let image = fingerprint(state, bounds, true)?;
    let revision = next(ledger.revision)?;
    let id = format!("settlement-{}-{revision}", settlement.0);
    let cells: Vec<_> = cells
        .into_iter()
        .map(|cell| Cell {
            at: cell.at,
            state: cell.state,
            resource: cell.resource,
            quantity: cell.quantity,
        })
        .collect();
    if cells.iter().any(|cell| {
        (0..3).any(|axis| cell.at[axis] < bounds.min[axis] || cell.at[axis] > bounds.max[axis])
    }) {
        return Err(Failure::InvalidRequest);
    }
    let (clear, build): (Vec<_>, Vec<_>) = cells
        .into_iter()
        .partition(|cell| cell.state == state.registry.states.air);
    let mut counts = BTreeMap::new();
    let mut stages = Vec::with_capacity(2);
    let mut batches = Vec::with_capacity(2);
    for (name, cells) in [("clear", clear), ("build", build)] {
        if cells.is_empty() {
            continue;
        }
        let mut stage_counts = BTreeMap::new();
        for cell in &cells {
            if let Some(resource) = &cell.resource {
                *stage_counts.entry(resource.clone()).or_insert(0) += u64::from(cell.quantity);
                *counts.entry(resource.clone()).or_insert(0) += u64::from(cell.quantity);
            }
        }
        stages.push(ScriptStructureStagePlan::new(
            name.into(),
            cells.len() as u32,
            cells.len() as u64,
            materials(&stage_counts),
        ));
        batches.push(cells);
    }
    let plan = resource_plan(&stages);
    plan.validate().map_err(|_| Failure::InvalidRequest)?;
    let snapshot = ScriptStructureSnapshot::new(
        id.clone(),
        format!("solaris-settlements:{blueprint}"),
        state.dimension.name().into(),
        ScriptStructureState::Prepared,
        revision,
        bounds.min,
        0,
        std::array::from_fn(|axis| bounds.max[axis] - bounds.min[axis] + 1),
        stages,
        0,
        0,
        inventories::resource_plan_hash(&plan),
        None,
        0,
        Vec::new(),
        materials(&counts),
        None,
        Vec::new(),
    );
    snapshot.validate().map_err(|_| Failure::InvalidRequest)?;
    ledger.structures.insert(
        id.clone(),
        Structure {
            owner: owner(settlement),
            settlement: Some(settlement),
            snapshot,
            blueprint_hash: inventories::hash(&batches),
            image,
            cells: batches,
            receipts: Vec::new(),
        },
    );
    ledger.revision = revision;
    save(state, &ledger);
    Ok(id)
}

pub(crate) fn area_reserved(state: &TickState, bounds: ScriptWorldBounds) -> bool {
    ledger(state).map_or(true, |ledger| {
        ledger.structures.values().any(|record| {
            record.snapshot.dimension == state.dimension.name()
                && record.snapshot.state != ScriptStructureState::Cancelled
                && bounds_of_structure(record).is_ok_and(|existing| overlaps(existing, bounds))
        })
    })
}

pub(crate) fn cancel(state: &mut TickState, id: SettlementId) -> Result<(), Failure> {
    let mut ledger = ledger(state)?;
    if !ledger.structures.values().any(|record| {
        record.settlement == Some(id) && record.snapshot.state != ScriptStructureState::Cancelled
    }) {
        return Ok(());
    }
    let revision = next(ledger.revision)?;
    for record in ledger
        .structures
        .values_mut()
        .filter(|record| record.settlement == Some(id))
    {
        record.snapshot.state = ScriptStructureState::Cancelled;
        record.snapshot.revision = revision;
        record.snapshot.pause_reason = None;
        record.snapshot.reservation_ref = None;
    }
    ledger.revision = revision;
    save(state, &ledger);
    Ok(())
}

/// Cancel one prepared structure of this settlement, leaving committed
/// structures and the materials already in stock or carry untouched.
pub(crate) fn cancel_structure(
    state: &mut TickState,
    settlement: SettlementId,
    handle: &str,
) -> Result<(), Failure> {
    let mut ledger = ledger(state)?;
    structure(&ledger, &owner(settlement), handle)?;
    let revision = next(ledger.revision)?;
    let record = ledger.structures.get_mut(handle).ok_or(Failure::NotFound)?;
    record.snapshot.state = ScriptStructureState::Cancelled;
    record.snapshot.revision = revision;
    record.snapshot.pause_reason = None;
    record.snapshot.reservation_ref = None;
    ledger.revision = revision;
    save(state, &ledger);
    Ok(())
}

pub(crate) fn status(
    state: &TickState,
    settlement: SettlementId,
    handle: &str,
) -> Result<ScriptStructureSnapshot, Failure> {
    let ledger = ledger(state)?;
    Ok(structure(&ledger, &owner(settlement), handle)?
        .snapshot
        .clone())
}

/// A foreign edit inside a native site pauses the structure as
/// `footprint_changed`. The next construction admission accepts the world as
/// it now is: re-snapshot the footprint and resume. Remaining cells are placed
/// over whatever stands there; cells already placed are never re-planned.
/// Returns whether a paused structure was resumed.
pub(crate) fn recover(
    state: &mut TickState,
    settlement: SettlementId,
    handle: &str,
) -> Result<bool, Failure> {
    let mut ledger = ledger(state)?;
    let mut record = take_structure(&mut ledger, &owner(settlement), handle)?;
    if record.snapshot.state != ScriptStructureState::Paused
        || record.snapshot.pause_reason.as_deref() != Some("footprint_changed")
    {
        return Ok(false);
    }
    let bounds = bounds_of_structure(&record)?;
    record.image = fingerprint(world(state, &record.snapshot.dimension)?, bounds, true)?;
    record.snapshot.state = ScriptStructureState::Running;
    record.snapshot.pause_reason = None;
    ledger.revision = next(ledger.revision)?;
    record.snapshot.revision = ledger.revision;
    ledger.structures.insert(handle.to_owned(), record);
    save(state, &ledger);
    Ok(true)
}

pub(crate) fn next_cell(
    state: &TickState,
    settlement: SettlementId,
    handle: &str,
) -> Result<Option<[i32; 3]>, Failure> {
    let ledger = ledger(state)?;
    let record = structure(&ledger, &owner(settlement), handle)?;
    Ok(record
        .cells
        .get(record.snapshot.current_stage_index as usize)
        .and_then(|cells| cells.get(record.snapshot.completed_work_units as usize))
        .map(|cell| cell.at))
}

pub(crate) fn advance(
    state: &mut TickState,
    settlement: SettlementId,
    handle: &str,
    builder: uuid::Uuid,
) -> Result<bool, Failure> {
    let snapshot = status(state, settlement, handle)?;
    let stage = snapshot
        .stages
        .get(snapshot.current_stage_index as usize)
        .ok_or(Failure::Blocked)?;
    let terminal = snapshot.current_stage_index as usize + 1 == snapshot.stages.len()
        && snapshot.completed_work_units + 1 == stage.work_units;
    let (_, result) = execute_operation(
        state,
        &owner(settlement),
        &ScriptWorldStructureOperation::AdvanceStructure {
            operation_id: format!("settlement-build-{}", snapshot.revision),
            structure_id: handle.into(),
            stage: stage.stage.clone(),
            reservation_ref: builder.to_string(),
            expected_revision: snapshot.revision,
            work_units: 1,
        },
    )?;
    Ok(terminal && matches!(result, ScriptWorldStructureResult::Receipt { .. }))
}

pub(super) fn authorized(state: &TickState, record: &Structure) -> bool {
    record.settlement.is_some_and(|id| {
        record.owner == owner(id)
            && record.snapshot.dimension == state.dimension.name()
            && state
                .settlements
                .get(id)
                .and_then(|record| record.project.as_ref())
                .is_some_and(|project| project.structure == record.snapshot.structure_id)
    })
}

pub(crate) fn source_images(state: &TickState, id: SettlementId) -> Vec<inventory::Image> {
    let mut images: Vec<inventory::Image> = Vec::new();
    for position in crate::settlements::construction::sources(state, id)
        .into_iter()
        .take(64)
    {
        let endpoint = inventory::Endpoint::Container {
            dimension: state.dimension.name().into(),
            position,
        };
        let Ok(image) = inventory::load(state, &endpoint) else {
            continue;
        };
        // Fences contain physical identity, including both large-chest halves.
        if !images.iter().any(|before| before.fence == image.fence) {
            images.push(image);
        }
    }
    images
}

pub(super) fn debit(
    state: &mut TickState,
    id: SettlementId,
    builder: &str,
    required: &BTreeMap<String, u64>,
) -> Result<(), Failure> {
    let uuid = uuid::Uuid::parse_str(builder).map_err(|_| Failure::InvalidRequest)?;
    let slot = state.entities.slot_of_uuid(uuid).ok_or(Failure::NotFound)?;
    if !state.entities.row(slot).villager.as_ref().is_some_and(|v| {
        v.settlement == Some(id)
            && v.data.profession == crate::villager::VillagerProfession::Builder
    }) {
        return Err(Failure::Forbidden);
    }
    if required.is_empty() {
        return Ok(());
    }
    let before = inventory::load(state, &inventory::Endpoint::VillagerCarry { uuid })?;
    let mut after = before.slots.clone();
    for (resource, quantity) in required {
        let mut remaining = *quantity;
        for stack in &mut after {
            if stack.count > 0
                && state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .is_some_and(|name| name.as_str() == resource)
            {
                let count = remaining.min(stack.count as u64);
                remaining -= count;
                stack.count -= count as i32;
                if stack.count == 0 {
                    *stack = mc_data::ItemStack::EMPTY;
                }
            }
        }
        if remaining != 0 {
            return Err(Failure::InsufficientItems);
        }
    }
    inventory::commit(state, vec![inventory::Change { before, after }])?;
    Ok(())
}

pub(crate) fn missing(
    state: &TickState,
    id: SettlementId,
    handle: &str,
) -> Result<BTreeMap<String, u64>, Failure> {
    let snapshot = status(state, id, handle)?;
    let mut required: BTreeMap<String, u64> = snapshot
        .remaining
        .iter()
        .map(|material| (material.resource.clone(), material.quantity))
        .collect();
    for slot in crate::settlements::construction::residents(state, id) {
        let Some(v) = state.entities.row(slot).villager.as_ref() else {
            continue;
        };
        if v.settlement != Some(id)
            || v.data.profession != crate::villager::VillagerProfession::Builder
        {
            continue;
        }
        for stack in v.inventory.iter().flatten().filter(|stack| stack.count > 0) {
            if let Some(name) = state.registry.items.name_of(stack.item_id)
                && let Some(count) = required.get_mut(name.as_str())
            {
                *count = count.saturating_sub(stack.count as u64);
            }
        }
    }
    required.retain(|_, count| *count > 0);
    missing_stock(state, id, &required)
}

pub(super) fn remaining(record: &Structure) -> Vec<ScriptStructureMaterial> {
    let mut counts = BTreeMap::new();
    let current = record.snapshot.current_stage_index as usize;
    for (index, cells) in record.cells.iter().enumerate().skip(current) {
        let offset = if index == current {
            record.snapshot.completed_work_units as usize
        } else {
            0
        };
        for cell in cells.iter().skip(offset) {
            if let Some(resource) = &cell.resource {
                *counts.entry(resource.clone()).or_insert(0) += u64::from(cell.quantity);
            }
        }
    }
    materials(&counts)
}

pub(crate) fn missing_stock(
    state: &TickState,
    id: SettlementId,
    required: &BTreeMap<String, u64>,
) -> Result<BTreeMap<String, u64>, Failure> {
    let items = required
        .iter()
        .map(|(name, count)| {
            let name = mc_data::Identifier::parse(name).map_err(|_| Failure::InvalidRequest)?;
            let item = state
                .registry
                .items
                .id_of(&name)
                .ok_or(Failure::InvalidRequest)?;
            Ok((item, u32::try_from(*count).map_err(|_| Failure::Capacity)?))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    let images = source_images(state, id);
    if crate::settlements::construction_crafting::can_fund(state, &items, &images) {
        return Ok(BTreeMap::new());
    }
    Ok(
        crate::settlements::construction_crafting::missing_base(state, &items, &images)
            .into_iter()
            .filter_map(|(item, count)| {
                state
                    .registry
                    .items
                    .name_of(item)
                    .map(|name| (name.to_string(), u64::from(count)))
            })
            .collect(),
    )
}

/// Foundation cells extend the bottom of the ledger box, never its top.
/// Recover the exact authored floor from that top and the future footprint;
/// do not treat arbitrary vertically overlapping buildings as the same site.
fn same_site(state: &TickState, record: &Structure, origin: [i32; 3]) -> bool {
    let Some(blueprint) = record
        .snapshot
        .blueprint_id
        .strip_prefix("solaris-settlements:")
        .and_then(|id| crate::settlements::blueprints::get(&state.registry, id))
    else {
        return false;
    };
    let size = blueprint.footprint(crate::settlements::blueprints::Rotation::from_quarter_turns(0));
    bounds_of_structure(record).is_ok_and(|bounds| {
        origin
            == [
                bounds.min[0],
                bounds.max[1] - i32::from(size[1]) + 1,
                bounds.min[2],
            ]
    })
}

pub(crate) fn built_at(
    state: &TickState,
    id: SettlementId,
    blueprint: &str,
    origin: [i32; 3],
) -> bool {
    ledger(state).is_ok_and(|ledger| {
        ledger.structures.values().any(|record| {
            record.settlement == Some(id)
                && record
                    .snapshot
                    .blueprint_id
                    .strip_prefix("solaris-settlements:")
                    == Some(blueprint)
                && same_site(state, record, origin)
                && record.snapshot.state == ScriptStructureState::Committed
        })
    })
}

pub(crate) fn next_material(
    state: &TickState,
    id: SettlementId,
    handle: &str,
) -> Result<Option<(u32, u32)>, Failure> {
    let ledger = ledger(state)?;
    let record = structure(&ledger, &owner(id), handle)?;
    let Some(cell) = record
        .cells
        .get(record.snapshot.current_stage_index as usize)
        .and_then(|cells| cells.get(record.snapshot.completed_work_units as usize))
    else {
        return Ok(None);
    };
    let Some(name) = cell.resource.as_ref() else {
        return Ok(None);
    };
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(name).map_err(|_| Failure::InvalidRequest)?)
        .ok_or(Failure::InvalidRequest)?;
    Ok(Some((item, cell.quantity)))
}

/// Materials for the next cells of the current stage, in first-need order,
/// that carry cannot yet cover, packed into the carry's slot capacity. The
/// first uncovered cell is always planned; later cells join while every
/// distinct item still fits in stacks of the villager's real inventory.
fn haul_plan(
    state: &TickState,
    record: &Structure,
    carry: &[mc_data::ItemStack],
) -> Vec<(u32, u32)> {
    let rules = state.registry.item_rules();
    let stack_limit = |item: u32| rules.max_stack(&mc_data::ItemStack::new(item, 1)).max(1) as u32;
    let mut carried: BTreeMap<u32, u32> = BTreeMap::new();
    for stack in carry.iter().filter(|stack| stack.count > 0) {
        *carried.entry(stack.item_id).or_default() += stack.count as u32;
    }
    let slots = |totals: &BTreeMap<u32, u32>| -> usize {
        totals
            .iter()
            .map(|(item, total)| total.div_ceil(stack_limit(*item)) as usize)
            .sum()
    };
    // Per-item quantity the window needs beyond what is already carried.
    let mut totals = carried.clone();
    let mut need: BTreeMap<u32, u32> = BTreeMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut covered: BTreeMap<u32, u32> = carried;
    let Some(cells) = record
        .cells
        .get(record.snapshot.current_stage_index as usize)
    else {
        return Vec::new();
    };
    for cell in cells
        .iter()
        .skip(record.snapshot.completed_work_units as usize)
    {
        let Some(name) = cell.resource.as_ref() else {
            continue;
        };
        let Some(item) = mc_data::Identifier::parse(name)
            .ok()
            .and_then(|id| state.registry.items.id_of(&id))
        else {
            break;
        };
        let quantity = cell.quantity;
        let have = covered.entry(item).or_default();
        let from_carry = (*have).min(quantity);
        let missing = quantity - from_carry;
        if missing > 0 {
            let mut trial = totals.clone();
            *trial.entry(item).or_default() += missing;
            if !order.is_empty() && slots(&trial) > carry.len() {
                break;
            }
            totals = trial;
            if !need.contains_key(&item) {
                order.push(item);
            }
            *need.entry(item).or_default() += missing;
        }
        *have -= from_carry;
    }
    order.into_iter().map(|item| (item, need[&item])).collect()
}

/// A transfer, not a debit: real items enter resident carry and are debited
/// only by the shared AdvanceStructure transaction alongside the block write.
/// One trip loads the next cells' materials across several item types, up to
/// the carry's slot capacity; `item` (the next cell's material) is mandatory.
pub(crate) fn fetch(
    state: &mut TickState,
    uuid: uuid::Uuid,
    position: [i32; 3],
    item: u32,
) -> Result<(), Failure> {
    let slot = state.entities.slot_of_uuid(uuid).ok_or(Failure::NotFound)?;
    let id = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .and_then(|v| v.settlement)
        .ok_or(Failure::Forbidden)?;
    let carry = inventory::load(state, &inventory::Endpoint::VillagerCarry { uuid })?;
    let plan = {
        let ledger = ledger(state)?;
        let project = state
            .settlements
            .get(id)
            .and_then(|record| record.project.as_ref())
            .ok_or(Failure::Forbidden)?;
        let record = structure(&ledger, &owner(id), &project.structure)?;
        haul_plan(state, record, &carry.slots)
    };
    if !plan.iter().any(|(planned, _)| *planned == item) {
        return Err(Failure::Blocked);
    }
    let images = source_images(state, id);
    if !images.iter().any(|image| {
        matches!(&image.endpoint,
        inventory::Endpoint::Container { position: point, .. } if *point == position)
    }) {
        return Err(Failure::Forbidden);
    }
    let mut stock_after: Vec<Vec<mc_data::ItemStack>> =
        images.iter().map(|image| image.slots.clone()).collect();
    let mut carry_after = carry.slots.clone();
    let rules = state.registry.item_rules();
    for (planned, wanted) in plan {
        // Craft the whole haul; on a shortfall settle for what stock can give.
        let mut amount = wanted;
        let mut crafted = None;
        while amount > 0 {
            let mut trial = stock_after.clone();
            if crate::settlements::construction_crafting::ensure_units(
                state,
                &mut trial,
                planned,
                amount as i32,
            ) {
                crafted = Some(trial);
                break;
            }
            amount /= 2;
        }
        let Some(trial) = crafted else {
            if planned == item {
                return Err(Failure::InsufficientItems);
            }
            continue;
        };
        stock_after = trial;
        let mut moved = 0;
        for source in stock_after
            .iter_mut()
            .flatten()
            .filter(|stack| stack.item_id == planned && stack.count > 0)
        {
            let maximum = rules.max_stack(source);
            while moved < amount as i32 && source.count > 0 {
                let destination = carry_after
                    .iter()
                    .position(|stack| {
                        !stack.is_empty()
                            && mc_data::inventory_semantics_26_1_2::can_stack(stack, source)
                            && stack.count < maximum
                    })
                    .or_else(|| carry_after.iter().position(mc_data::ItemStack::is_empty));
                let Some(destination) = destination else {
                    break;
                };
                let destination = &mut carry_after[destination];
                if destination.is_empty() {
                    *destination = source.clone();
                    destination.count = 0;
                }
                let take = (amount as i32 - moved)
                    .min(source.count)
                    .min(maximum - destination.count);
                if take <= 0 {
                    break;
                }
                destination.count += take;
                source.count -= take;
                moved += take;
            }
            if source.count == 0 {
                *source = mc_data::ItemStack::EMPTY;
            }
        }
        if moved == 0 && planned == item {
            return Err(Failure::Capacity);
        }
    }
    let changes = images
        .into_iter()
        .zip(stock_after)
        .filter_map(|(before, after)| {
            (before.slots != after).then_some(inventory::Change { before, after })
        })
        .chain(std::iter::once(inventory::Change {
            before: carry,
            after: carry_after,
        }))
        .collect();
    inventory::commit(state, changes)?;
    Ok(())
}

/// Return every resident builder's carried stock to canonical storage, so an
/// order or cancellation never hides real materials inside a carry. Stacks
/// that do not fit stay carried; nothing is lost or duplicated.
pub(crate) fn return_carry(state: &mut TickState, id: SettlementId) {
    let builders: Vec<uuid::Uuid> = crate::settlements::construction::residents(state, id)
        .filter_map(|slot| {
            let row = state.entities.row(slot);
            row.villager
                .as_ref()
                .is_some_and(|v| {
                    v.settlement == Some(id)
                        && v.data.profession == crate::villager::VillagerProfession::Builder
                })
                .then_some(row.uuid)
        })
        .collect();
    for uuid in builders {
        let Ok(carry) = inventory::load(state, &inventory::Endpoint::VillagerCarry { uuid }) else {
            continue;
        };
        if carry.slots.iter().all(mc_data::ItemStack::is_empty) {
            continue;
        }
        let images = source_images(state, id);
        let mut post: Vec<Vec<mc_data::ItemStack>> =
            images.iter().map(|image| image.slots.clone()).collect();
        let mut after = carry.slots.clone();
        for stack in after.iter_mut().filter(|stack| !stack.is_empty()) {
            let mut trial = post.clone();
            if crate::settlements::construction_crafting::add_stack(state, &mut trial, stack) {
                post = trial;
                *stack = mc_data::ItemStack::EMPTY;
            }
        }
        let mut changes: Vec<_> = images
            .into_iter()
            .zip(post)
            .filter_map(|(before, after)| {
                (before.slots != after).then_some(inventory::Change { before, after })
            })
            .collect();
        if changes.is_empty() {
            continue;
        }
        changes.push(inventory::Change {
            before: carry,
            after,
        });
        let _ = inventory::commit(state, changes);
    }
}

pub(crate) fn pickup_point(
    state: &TickState,
    id: SettlementId,
    item: u32,
    origin: [f64; 3],
) -> Option<[i32; 3]> {
    let images = source_images(state, id);
    let has_item = |image: &inventory::Image| {
        image
            .slots
            .iter()
            .any(|stack| stack.item_id == item && stack.count > 0)
    };
    let exact = images.iter().any(has_item);
    if !exact && !crate::settlements::construction_crafting::can_craft(state, item, &images) {
        return None;
    }
    images
        .iter()
        .filter(|image| !exact || has_item(image))
        .filter_map(|image| {
            let inventory::Endpoint::Container { position, .. } = &image.endpoint else {
                return None;
            };
            let distance = (origin[0] - f64::from(position[0])).powi(2)
                + (origin[2] - f64::from(position[2])).powi(2);
            Some((*position, distance))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(point, _)| point)
}
