//! Shared `/settlement building` selection, authorization and certification.
//!
//! Chat commands resolve the target settlement and owner/operator admission
//! here before calling the same native certification API used by dialog UI.
//! Replies are formatted for chat; world checks and mutation belong to
//! `settlements::buildings`.

use crate::boundary::EntityId;
use crate::tick::TickState;

/// Nobody here: the executor stands outside every settlement radius sphere
/// (or has no position at all, e.g. the console).
pub(crate) const NO_SETTLEMENT: &str = "No settlement contains your position.";
/// Mutations without an authenticated actor refuse outright.
pub(crate) const ADMISSION_UNAVAILABLE: &str = "Settlement command admission is unavailable.";

#[cfg(test)]
#[path = "building_commands_tests.rs"]
mod tests;

fn executor_pos(state: &TickState, executor: EntityId) -> Option<[f64; 3]> {
    state
        .entities
        .slot_of(executor)
        .map(|slot| state.entities.pos(slot))
}

fn distance_squared_to(pose: [f64; 3], anchor: [i32; 3]) -> f64 {
    let dx = pose[0] - f64::from(anchor[0]);
    let dy = pose[1] - f64::from(anchor[1]);
    let dz = pose[2] - f64::from(anchor[2]);
    dx * dx + dy * dy + dz * dz
}

/// Settlement containing the executor (within its radius sphere).
/// Deterministic: the closest anchor wins, then the smallest settlement id
/// (records iterate in id order, so only a strict improvement replaces).
pub(crate) fn containing(
    state: &TickState,
    executor: EntityId,
) -> Option<crate::settlements::SettlementId> {
    let pose = executor_pos(state, executor)?;
    let mut best: Option<(crate::settlements::SettlementId, f64)> = None;
    for record in state.settlements.records() {
        let distance = distance_squared_to(pose, record.anchor);
        if distance <= f64::from(record.radius).powi(2)
            && best.is_none_or(|(_, known)| distance < known)
        {
            best = Some((record.id, distance));
        }
    }
    best.map(|(id, _)| id)
}

/// Certify `kind` over `min..=max` in the executor's settlement (nearest
/// anchor containing the executor). Owner or operator of a claimed settlement
/// only: an autonomous settlement grants neither, so it is refused with the
/// claim instruction instead of an owner/operator message. Returns the
/// chat-ready success line.
pub(crate) fn certify(
    state: &mut TickState,
    executor: EntityId,
    actor: Option<&str>,
    operator: bool,
    kind: &str,
    min: [i32; 3],
    max: [i32; 3],
) -> Result<String, String> {
    use crate::settlements::buildings::{BuildingKind, certify as certify_building};

    let Some(building_kind) = BuildingKind::parse(kind) else {
        return Err(format!("Unknown building kind '{kind}'."));
    };
    let kind_name = building_kind.name();
    let bounds =
        mc_script::ScriptWorldBounds::new(min, max).map_err(|_| "Invalid building bounds.")?;
    let Some(id) = containing(state, executor) else {
        return Err(NO_SETTLEMENT.to_owned());
    };
    let Some(record) = state.settlements.get(id) else {
        return Err("Settlement not found.".to_owned());
    };
    let Some(actor) = actor else {
        return Err(ADMISSION_UNAVAILABLE.to_owned());
    };
    if record.owner.is_none() {
        return Err(format!(
            "{} is an autonomous settlement; claim it before managing it.",
            record.name
        ));
    }
    if !record.can_manage(actor, operator) {
        return Err("Only the owner or an operator certifies buildings.".to_owned());
    }
    match certify_building(state, id, building_kind, bounds) {
        Ok(building) => {
            let level = state
                .settlements
                .get(id)
                .and_then(|record| record.buildings.get(&building))
                .expect("certified building")
                .level;
            Ok(format!(
                "Certified {kind_name} #{} (level {level}).",
                building.0
            ))
        }
        Err(failure) => Err(failure.to_string()),
    }
}

/// Upgrade a globally addressed building, authorized against its settlement.
pub(crate) fn upgrade(
    state: &mut TickState,
    actor: Option<&str>,
    operator: bool,
    id: u64,
) -> Result<String, String> {
    use crate::settlements::buildings::{BuildingId, upgrade as upgrade_building};

    let sought = BuildingId(id);
    let home = crate::settlements::SettlementId(id >> 32);
    let record = state
        .settlements
        .get(home)
        .filter(|record| record.buildings.contains_key(&sought))
        .ok_or_else(|| "Building not found.".to_owned())?;
    let Some(actor) = actor else {
        return Err(ADMISSION_UNAVAILABLE.to_owned());
    };
    if !record.can_manage(actor, operator) {
        return Err("Only the owner or an operator upgrades buildings.".to_owned());
    }
    match upgrade_building(state, sought) {
        Ok(()) => {
            let level = state
                .settlements
                .get(home)
                .and_then(|record| record.buildings.get(&sought))
                .expect("upgraded building")
                .level;
            Ok(format!("Upgraded building #{id} to level {level}."))
        }
        Err(failure) => Err(failure.to_string()),
    }
}

/// List the executor's settlement buildings. Public: no actor needed.
/// Every entry shows id, kind, level and bounds; autonomous natural worksites
/// (a wild grove or quarry, not a built structure) end with `natural`.
pub(crate) fn list(state: &TickState, executor: EntityId) -> Result<String, String> {
    let Some(id) = containing(state, executor) else {
        return Err(NO_SETTLEMENT.to_owned());
    };
    let Some(record) = state.settlements.get(id) else {
        return Err("Settlement not found.".to_owned());
    };
    if record.buildings.is_empty() {
        return Ok("No buildings.".to_owned());
    }
    let mut entries = Vec::with_capacity(record.buildings.len());
    for (building_id, building) in &record.buildings {
        entries.push(format!(
            "#{} {} level {} {:?}-{:?}{}",
            building_id.0,
            building.kind.name(),
            building.level,
            building.bounds.min,
            building.bounds.max,
            if building.natural_zone {
                " natural"
            } else {
                ""
            },
        ));
    }
    Ok(format!("Buildings: {}.", entries.join(", ")))
}
