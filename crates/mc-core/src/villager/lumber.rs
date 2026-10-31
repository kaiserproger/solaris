//! Settlement lumberjack: bounded incremental tree search, one trunk block
//! per work attempt, matching sapling replant and storage deposit.
//!
//! A bound [`VillagerProfession::Lumberjack`] with a certified `LumberCamp`
//! scans its settlement radius a fixed number of columns per attempt, walks
//! to the first mature tree (log column on valid soil with a wild
//! `persistent=false` leaf crown outside every other certified building
//! footprint), cuts the trunk bottom-up across ticks and replants the
//! matching sapling. Protected bases and decorative crowns are rejected at
//! search and rechecked before every cut. Target and cursor survive
//! full-inventory and day-end deposit trips through the shared
//! [`super::mining`] helpers.
use super::mining;
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::buildings;
use crate::tick::TickState;
use mc_data::ItemStack;

const PERIOD: u64 = 20;
/// Simulated tree columns scanned per work attempt; the cursor lives in [`Progress`].
pub(super) const SEARCH_PER_TICK: u32 = 128;
/// Inactive chunk strips skipped per work attempt, one lookup each.
const SKIP_PER_ATTEMPT: u32 = 1024;
const REACH: f64 = 4.0;
const MIN_TRUNK: u16 = 4;
const MAX_TRUNK: u16 = 384;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "StoredProgress")]
pub struct Progress {
    pub building: u64,
    pub revision: u64,
    pub cursor: u32,
    pub base: [i32; 3],
    pub sapling: u32,
    pub height: u16,
    pub layer: u16,
    pub has_target: bool,
    pub returning: bool,
}

/// Stored shape: [`Progress`] plus `vertical`, the search band that older
/// worlds wrote and nothing reads any more. Unknown fields stay rejected.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredProgress {
    building: u64,
    revision: u64,
    cursor: u32,
    #[serde(default)]
    #[allow(dead_code)]
    vertical: u32,
    base: [i32; 3],
    sapling: u32,
    height: u16,
    layer: u16,
    has_target: bool,
    returning: bool,
}

impl From<StoredProgress> for Progress {
    fn from(stored: StoredProgress) -> Self {
        Self {
            building: stored.building,
            revision: stored.revision,
            cursor: stored.cursor,
            base: stored.base,
            sapling: stored.sapling,
            height: stored.height,
            layer: stored.layer,
            has_target: stored.has_target,
            returning: stored.returning,
        }
    }
}

fn block_path(state: &TickState, x: i32, y: i32, z: i32) -> Option<&str> {
    let id = state.world.block_at(x, y, z)?;
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .map(|definition| definition.block.id.path())
}

fn is_log(path: &str) -> bool {
    path.ends_with("_log") && !path.starts_with("stripped_")
}

fn is_leaves(path: &str) -> bool {
    path.ends_with("_leaves")
}

fn is_soil(path: &str) -> bool {
    matches!(
        path,
        "grass_block"
            | "dirt"
            | "coarse_dirt"
            | "podzol"
            | "rooted_dirt"
            | "mud"
            | "farmland"
            | "moss_block"
    )
}

fn column_height(state: &TickState, x: i32, y: i32, z: i32) -> u16 {
    let mut height = 0;
    while height < MAX_TRUNK && block_path(state, x, y + i32::from(height), z).is_some_and(is_log) {
        height += 1;
    }
    height
}

/// Only wild crowns count: every accepted leaf state carries
/// `persistent=false`. Hand-placed decoration (`persistent=true`) never
/// marks a trunk as a tree.
fn natural_leaves(state: &TickState, x: i32, y: i32, z: i32) -> bool {
    let Some(id) = state.world.block_at(x, y, z) else {
        return false;
    };
    let Some(definition) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return false;
    };
    if !is_leaves(definition.block.id.path()) {
        return false;
    }
    definition
        .properties
        .iter()
        .any(|(name, value)| name == "persistent" && value == "false")
}

fn has_leaves(state: &TickState, x: i32, top: i32, z: i32) -> bool {
    for dx in -2..=2 {
        for dz in -2..=2 {
            for dy in -2..=2 {
                if natural_leaves(state, x + dx, top + dy, z + dz) {
                    return true;
                }
            }
        }
    }
    false
}

/// Certificates protect every base except this worker's designated natural grove.
fn base_protected(
    state: &TickState,
    settlement: Option<crate::settlements::SettlementId>,
    camp: u64,
    base: [i32; 3],
) -> bool {
    let Some(settlement) = settlement else {
        return false;
    };
    let Some(record) = state.settlements.get(settlement) else {
        return false;
    };
    record.buildings.values().any(|building| {
        !(building.id.0 == camp && building.natural_zone)
            && (0..3)
                .all(|i| base[i] >= building.bounds.min[i] && base[i] <= building.bounds.max[i])
    })
}

/// `(sapling block, trunk height)` when `(x, y, z)` is a mature tree base: a
/// log over valid soil with a contiguous log column and leaves at its
/// crown. Nether stems never match: they are not `_log` and have no sapling.
fn mature_tree(state: &TickState, x: i32, y: i32, z: i32) -> Option<(u32, u16)> {
    let path = block_path(state, x, y, z)?;
    if !is_log(path) {
        return None;
    }
    if !block_path(state, x, y - 1, z).is_some_and(is_soil) {
        return None;
    }
    let height = column_height(state, x, y, z);
    if height < MIN_TRUNK {
        return None;
    }
    if !has_leaves(state, x, y + i32::from(height) - 1, z) {
        return None;
    }
    let stem = path.strip_suffix("_log")?;
    let sapling_name = if stem == "mangrove" {
        "mangrove_propagule".to_owned()
    } else {
        format!("{stem}_sapling")
    };
    let id = mc_data::Identifier::parse(format!("minecraft:{sapling_name}")).ok()?;
    let sapling = state.registry.blocks.block(&id)?.default.0;
    Some((sapling, height))
}

fn columns_total(radius: u32) -> u32 {
    let side = radius.saturating_mul(2).saturating_add(1);
    side.saturating_mul(side).max(1)
}

fn column_at(anchor: [i32; 3], radius: u32, index: u32) -> [i32; 2] {
    let side = radius.saturating_mul(2).saturating_add(1).max(1);
    let slot = index % columns_total(radius);
    let dx = (slot / side) as i32 - radius as i32;
    let dz = (slot % side) as i32 - radius as i32;
    [anchor[0] + dx, anchor[2] + dz]
}

/// The first mature unprotected tree in one column of the settlement sphere.
/// A trunk is at least [`MIN_TRUNK`] logs tall, so probing every fourth block
/// of the sphere's vertical chord touches every trunk; the base is found by
/// walking down its log column.
fn scan_column(
    state: &TickState,
    anchor: [i32; 3],
    radius: u32,
    settlement: Option<crate::settlements::SettlementId>,
    camp: u64,
    [x, z]: [i32; 2],
) -> Option<([i32; 3], u32, u16)> {
    let dx = i64::from(x) - i64::from(anchor[0]);
    let dz = i64::from(z) - i64::from(anchor[2]);
    let chord = (i64::from(radius).pow(2) - dx * dx - dz * dz).max(-1);
    if chord < 0 {
        return None;
    }
    let half = (chord as u64).isqrt() as i32;
    let lowest = anchor[1] - half;
    let mut y = anchor[1] + half;
    while y >= lowest {
        if !block_path(state, x, y, z).is_some_and(is_log) {
            y -= i32::from(MIN_TRUNK);
            continue;
        }
        let mut base = y;
        while y - base < i32::from(MAX_TRUNK)
            && block_path(state, x, base - 1, z).is_some_and(is_log)
        {
            base -= 1;
        }
        if let Some((sapling, height)) = mature_tree(state, x, base, z) {
            // A certified footprint over the base protects even a
            // natural-looking tree: player buildings are never harvestable.
            if !base_protected(state, settlement, camp, [x, base, z]) {
                return Some(([x, base, z], sapling, height));
            }
        }
        y = base - 1;
    }
    None
}

/// Scan at most [`SEARCH_PER_TICK`] simulated settlement columns from the
/// persisted cursor, wrapping the radius square. Unloaded columns read as
/// absent, never as trees, and columns outside the simulated area are never
/// targets: a worker routed to a tree in a chunk that never ticks would freeze
/// at its border with the target held. An inactive chunk strip is skipped in one
/// step (bounded by [`SKIP_PER_ATTEMPT`]) so the sweep spends its budget only on
/// columns that can actually be harvested.
fn search(
    state: &TickState,
    anchor: [i32; 3],
    radius: u32,
    settlement: Option<crate::settlements::SettlementId>,
    progress: &mut Progress,
) -> Option<([i32; 3], u32, u16)> {
    let total = columns_total(radius);
    let side = radius.saturating_mul(2).saturating_add(1).max(1);
    let (mut scanned, mut skipped) = (0, 0);
    while scanned < SEARCH_PER_TICK && skipped < SKIP_PER_ATTEMPT {
        let slot = progress.cursor % total;
        let [x, z] = column_at(anchor, radius, slot);
        if !state.world.is_active(x.div_euclid(16), z.div_euclid(16)) {
            let strip = (16 - z.rem_euclid(16)) as u32;
            progress.cursor = (slot + strip.min(side - slot % side)) % total;
            skipped += 1;
            continue;
        }
        progress.cursor = (slot + 1) % total;
        scanned += 1;
        if let Some(tree) =
            scan_column(state, anchor, radius, settlement, progress.building, [x, z])
        {
            return Some(tree);
        }
    }
    None
}

/// Clear a stale tree target; the radius cursor is kept, never rewound.
fn drop_target(v: &mut VillagerState) {
    v.lumber_progress.has_target = false;
    v.lumber_progress.base = [0, 0, 0];
    v.lumber_progress.sapling = 0;
    v.lumber_progress.height = 0;
    v.lumber_progress.layer = 0;
}

/// The bound settlement still owns the tree after radius/edit churn.
fn within_radius(state: &TickState, v: &VillagerState, base: BlockPos) -> bool {
    let Some(settlement) = v.settlement else {
        return false;
    };
    let Some(record) = state.settlements.get(settlement) else {
        return false;
    };
    let squared = [base.x, base.y, base.z]
        .into_iter()
        .zip(record.anchor)
        .map(|(n, a)| i64::from(n) - i64::from(a))
        .map(|d| d * d)
        .sum::<i64>();
    squared <= i64::from(record.radius) * i64::from(record.radius)
}

/// Plant the stored sapling into cleared air over valid soil, then release
/// the target. Soil or space lost mid-trunk skips the planting, never loops.
fn replant(state: &mut TickState, changes: &mut ChangeSet, v: &mut VillagerState) {
    let [x, y, z] = v.lumber_progress.base;
    let sapling = v.lumber_progress.sapling;
    drop_target(v);
    let empty = matches!(
        block_path(state, x, y, z),
        Some("air" | "cave_air" | "void_air")
    );
    let soil = block_path(state, x, y - 1, z).is_some_and(is_soil);
    if empty && soil {
        mining::replace(state, changes, BlockPos { x, y, z }, sapling);
    }
}

pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Lumberjack
        || v.age < 0
        || v.sleeping
        || v.activity == Activity::Panic
    {
        return None;
    }
    // Owned binding snapshot: the borrow of `state` ends before `v` mutates.
    let bound = buildings::building_for_job(state, v)
        .filter(|building| building.kind == buildings::BuildingKind::LumberCamp)
        .map(|building| (building.id.0, building.revision, building.natural_zone));
    let Some((id, revision, natural)) = bound else {
        // Unbound or rebound away from the camp: never route to a stale tree.
        drop_target(v);
        return None;
    };
    if v.lumber_progress.building != id {
        v.lumber_progress = Progress {
            building: id,
            revision,
            ..Progress::default()
        };
        if natural {
            let record = state.settlements.get(v.settlement?)?;
            let entrance = v.job_site?;
            // The certified grove is already a known resource source. Start
            // the same bounded radius sweep there, not at an unloaded corner.
            let radius = i64::from(record.radius);
            let side = radius * 2 + 1;
            let x =
                (i64::from(entrance.x) - i64::from(record.anchor[0]) + radius).clamp(0, side - 1);
            let z =
                (i64::from(entrance.z) - i64::from(record.anchor[2]) + radius).clamp(0, side - 1);
            v.lumber_progress.cursor = (x * side + z) as u32;
        }
    }
    v.lumber_progress.revision = revision;
    let working =
        v.activity == Activity::Work && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000));
    if !working
        || mining::needs_deposit(v)
        || (state.tick.is_multiple_of(PERIOD)
            && v.settlement.is_some_and(|id| {
                crate::settlements::construction::candidate_funded_by_delivery(
                    state,
                    id,
                    &v.inventory,
                )
            }))
    {
        v.lumber_progress.returning = true;
    }
    if v.lumber_progress.returning {
        let target = mining::deposit(state, slot, v);
        v.lumber_progress.returning = v.inventory.iter().any(Option::is_some);
        return target;
    }
    if !working
        || !crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING)
    {
        return None;
    }
    if !v.lumber_progress.has_target {
        let record = state.settlements.get(v.settlement?)?;
        let (anchor, radius) = (record.anchor, record.radius);
        if state.tick < v.harvest_at {
            return None;
        }
        v.harvest_at = state.tick + PERIOD;
        let settlement = v.settlement;
        let (base, sapling, height) =
            search(state, anchor, radius, settlement, &mut v.lumber_progress)?;
        v.lumber_progress.base = base;
        v.lumber_progress.sapling = sapling;
        v.lumber_progress.height = height;
        v.lumber_progress.layer = 0;
        v.lumber_progress.has_target = true;
    }
    let base = BlockPos {
        x: v.lumber_progress.base[0],
        y: v.lumber_progress.base[1],
        z: v.lumber_progress.base[2],
    };
    // A shrunk radius or a moved camp must not keep routing to the old tree.
    if !within_radius(state, v, base) {
        drop_target(v);
        return None;
    }
    if !mining::near(state, slot, base, REACH) {
        // The brain has exhausted its approach and reached a retry deadline
        // without getting into harvesting reach. Keep the sweep cursor, not
        // an inaccessible tree that would pin this worker for every workday.
        if v.navigation_target == Some(base)
            && v.navigation_search_failed
            && v.navigation_path.is_empty()
            && state.tick >= v.navigation_repath_at
        {
            drop_target(v);
            return None;
        }
        return Some(base);
    }
    if state.tick < v.harvest_at {
        return Some(base);
    }
    v.harvest_at = state.tick + PERIOD;
    if v.lumber_progress.height == 0 {
        drop_target(v);
        return None;
    }
    let layer = v.lumber_progress.layer;
    if layer >= v.lumber_progress.height {
        replant(state, changes, v);
        return None;
    }
    // Recheck the stored crown and all certificates at the actual cut
    // deadline, not on every intervening navigation tick.
    let top = base.y + i32::from(v.lumber_progress.height) - 1;
    if base_protected(
        state,
        v.settlement,
        v.lumber_progress.building,
        v.lumber_progress.base,
    ) || !has_leaves(state, base.x, top, base.z)
    {
        drop_target(v);
        return None;
    }
    let pos = BlockPos {
        y: base.y + i32::from(layer),
        ..base
    };
    let item = (|| {
        let path = block_path(state, pos.x, pos.y, pos.z)?;
        if !is_log(path) {
            return None;
        }
        mining::item_id(state, path)
    })();
    let Some(item) = item else {
        // Trunk changed under us; release the target, keep the cursor.
        drop_target(v);
        return None;
    };
    if mining::room(state, v, &ItemStack::new(item, 1)) < 1 {
        v.lumber_progress.returning = true;
        return mining::deposit(state, slot, v);
    }
    let air = state.registry.states.air;
    mining::collect(state, v, item, 1);
    mining::replace(state, changes, pos, air);
    v.lumber_progress.layer += 1;
    if v.lumber_progress.layer >= v.lumber_progress.height {
        replant(state, changes, v);
        return None;
    }
    Some(base)
}
