//! Physical settlement extraction: a resumable staircase followed by 1x2
//! serpentine branches. One block is removed per work attempt; probes are fixed.
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockChange, BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::settlements::{buildings, inventory};
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_script::ScriptWorldBounds;

const PERIOD: u64 = 20;
/// Most open cells a connected tunnel may have to still count as the mine's own
/// (a comb level is a few hundred cells), and the fewest that make one rather
/// than a dent in the ground.
const TUNNEL_CELLS: usize = 1_200;
const MIN_TUNNEL_CELLS: usize = 6;
/// Ticks a miner waits for a hostile beside its next dig cell to leave, or for
/// a route to the cell to open, before it abandons that lane for another.
const STALL_PATIENCE: u64 = 2_400;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub building: u64,
    pub revision: u64,
    pub bounds: Option<ScriptWorldBounds>,
    pub last_open: Option<[i32; 3]>,
    pub lane: u32,
    #[serde(default)]
    pub level: u8,
    /// Which staircase of the quarry: pass `n` starts where pass `n - 1`'s
    /// lane-0 staircase ended, one flight deeper.
    #[serde(default)]
    pub pass: u8,
    pub cell: u32,
    pub layer: u8,
    pub returning: bool,
    /// Tick the miner first stopped making progress on this lane (a hostile
    /// beside the next dig cell, or a dig cell it cannot walk to); zero while it
    /// does. A stall that outlasts [`STALL_PATIENCE`] closes the lane.
    #[serde(default)]
    pub stalled_since: u64,
}

pub(super) fn item_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?)
}

pub(super) fn room(state: &TickState, v: &VillagerState, stack: &ItemStack) -> i32 {
    let maximum = state.registry.item_rules().max_stack(stack);
    v.inventory
        .iter()
        .map(|slot| match slot {
            None => maximum,
            Some(existing) if mc_data::inventory_semantics_26_1_2::can_stack(existing, stack) => {
                (maximum - existing.count).max(0)
            }
            _ => 0,
        })
        .sum()
}

pub(super) fn collect(state: &TickState, v: &mut VillagerState, item: u32, count: i32) -> bool {
    let mut stack = ItemStack::new(item, count);
    if room(state, v, &stack) < count {
        return false;
    }
    let maximum = state.registry.item_rules().max_stack(&stack);
    super::brain::add_inventory(v, &mut stack, maximum);
    true
}

pub(super) fn replace(state: &mut TickState, changes: &mut ChangeSet, pos: BlockPos, id: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, id);
    changes.blocks.push(BlockChange {
        position: pos,
        state: id,
    });
}

pub(super) fn near(state: &TickState, slot: EntitySlot, pos: BlockPos, reach: f64) -> bool {
    let p = state.entities.pos(slot);
    (p[0] - f64::from(pos.x) - 0.5).powi(2)
        + (p[1] - f64::from(pos.y)).powi(2)
        + (p[2] - f64::from(pos.z) - 0.5).powi(2)
        <= reach * reach
}

pub(super) fn needs_deposit(v: &VillagerState) -> bool {
    v.inventory
        .iter()
        .any(|slot| slot.as_ref().is_some_and(|s| s.count >= 64))
}

/// A miner's load is mixed (dirt from the grass surface plus a little of every
/// stone never fills one stack), so a total as heavy as a stack also sends it
/// home; with the day clock stopped nothing else ever would.
fn heavy_load(v: &VillagerState) -> bool {
    needs_deposit(v) || v.inventory.iter().flatten().map(|s| s.count).sum::<i32>() >= 64
}

/// Container fences, reservation floors, double chests, viewers and the block
/// entity WAL all go through the existing native inventory authority. Carry is
/// detached by the brain; only remove it after the container commit succeeds.
pub(super) fn deposit(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
) -> Option<BlockPos> {
    if v.inventory.iter().all(Option::is_none) {
        return None;
    }
    let settlement = v.settlement?;
    let points = buildings::storage_points(state, settlement);
    let mut destination = None;
    for position in points {
        let endpoint = inventory::Endpoint::Container {
            dimension: state.dimension.name().into(),
            position,
        };
        let Ok(before) = inventory::load(state, &endpoint) else {
            continue;
        };
        let mut after = before.slots.clone();
        let mut carried = v.inventory.clone();
        let mut moved = false;
        for stack in &mut carried {
            let Some(moving) = stack else {
                continue;
            };
            let maximum = state.registry.item_rules().max_stack(moving);
            // Merge first, then use empty slots: never overwrite unrelated stock.
            for empty in [false, true] {
                for stored in &mut after {
                    if moving.count == 0 {
                        break;
                    }
                    if stored.is_empty() != empty {
                        continue;
                    }
                    if !stored.is_empty()
                        && !mc_data::inventory_semantics_26_1_2::can_stack(stored, moving)
                    {
                        continue;
                    }
                    let count = moving
                        .count
                        .min((maximum - if stored.is_empty() { 0 } else { stored.count }).max(0));
                    if count == 0 {
                        continue;
                    }
                    if stored.is_empty() {
                        *stored = moving.clone();
                        stored.count = 0;
                    }
                    stored.count += count;
                    moving.count -= count;
                    moved = true;
                }
            }
            if moving.count == 0 {
                *stack = None;
            }
        }
        if !moved {
            continue;
        }
        let pos = BlockPos {
            x: position[0],
            y: position[1],
            z: position[2],
        };
        if !near(state, slot, pos, 2.5) {
            let p = state.entities.pos(slot);
            let distance = (p[0] - f64::from(pos.x)).powi(2)
                + (p[1] - f64::from(pos.y)).powi(2)
                + (p[2] - f64::from(pos.z)).powi(2);
            if destination.is_none_or(|(_, best)| distance < best) {
                destination = Some((pos, distance));
            }
            continue;
        }
        if inventory::commit(state, vec![inventory::Change { before, after }]).is_ok() {
            v.inventory = carried;
            return if v.inventory.iter().all(Option::is_none) {
                None
            } else {
                Some(pos)
            };
        }
    }
    destination.map(|(pos, _)| pos)
}

fn inside(bounds: ScriptWorldBounds, p: BlockPos) -> bool {
    [p.x, p.y, p.z]
        .into_iter()
        .zip(bounds.min.into_iter().zip(bounds.max))
        .all(|(n, (min, max))| n >= min && n <= max)
}

/// O(1) cursor decoding: descend along X, extend to the wall, then alternate
/// parallel X branches separated by three Z blocks, joined at their ends. Each
/// `level` raises the whole comb by three blocks so successive levels cover the
/// zone's full height.
/// Direction, length and floor depth of one level's staircase.
fn staircase(bounds: ScriptWorldBounds, entrance: BlockPos, level: u8) -> Option<(i32, u32, i32)> {
    let left = bounds.min[0] + 1;
    let right = bounds.max[0] - 1;
    let direction = if right - entrance.x >= entrance.x - left {
        1
    } else {
        -1
    };
    let edge = if direction > 0 { right } else { left };
    let run = (edge - entrance.x).unsigned_abs();
    if run == 0 || entrance.y <= bounds.min[1] + 1 {
        return None;
    }
    let depth = (entrance.y - bounds.min[1] - 1).min(run as i32) - 3 * i32::from(level);
    (depth >= 1).then_some((direction, run, depth))
}

/// Side of the zone the rows of a comb extend toward, from the lane's column.
fn z_direction(bounds: ScriptWorldBounds, z: i32) -> i32 {
    if bounds.max[2] - 1 - z >= z - bounds.min[2] - 1 {
        1
    } else {
        -1
    }
}

/// A deeper flight's staircase runs two columns beside the open end of the
/// flight above, so it never undercuts that flight's stairs; two lead-in cells
/// join the open end to it. Flights alternate sides, so any number of them fit
/// the zone's width.
const FLIGHT_OFFSET: i32 = 2;

/// Direction of the lead-in and shift of flight `pass` from `base` along Z.
fn flight_side(bounds: ScriptWorldBounds, base: BlockPos, pass: u8) -> i32 {
    let toward_far_side = z_direction(bounds, base.z);
    if pass % 2 == 1 {
        toward_far_side
    } else {
        -toward_far_side
    }
}

fn stair_base(bounds: ScriptWorldBounds, origin: BlockPos, pass: u8) -> BlockPos {
    if pass == 0 {
        origin
    } else {
        BlockPos {
            z: origin.z + flight_side(bounds, origin, pass) * FLIGHT_OFFSET,
            ..origin
        }
    }
}

/// `cell` of flight `pass`: its lead-in cells, then the shifted staircase.
fn cell_in_flight(
    bounds: ScriptWorldBounds,
    origin: BlockPos,
    pass: u8,
    index: u32,
    level: u8,
) -> Option<BlockPos> {
    if pass == 0 {
        return cell(bounds, origin, index, level);
    }
    let lead = FLIGHT_OFFSET as u32;
    if index < lead {
        let at = BlockPos {
            z: origin.z + flight_side(bounds, origin, pass) * (index as i32 + 1),
            ..origin
        };
        return (inside(bounds, at) && at.z > bounds.min[2] && at.z < bounds.max[2]).then_some(at);
    }
    cell(
        bounds,
        stair_base(bounds, origin, pass),
        index - lead,
        level,
    )
}

/// Where flight `pass` starts: pass 0 at the entrance, each later pass at the
/// bottom of the previous pass's staircase (the deepest cell of its level-0
/// row). `None` once a flight no longer fits above the bottom of the zone.
fn pass_entrance(bounds: ScriptWorldBounds, entrance: BlockPos, pass: u8) -> Option<BlockPos> {
    let mut base = entrance;
    for flight in 0..pass {
        let stair = stair_base(bounds, base, flight);
        let (direction, run, depth) = staircase(bounds, stair, 0)?;
        base = BlockPos {
            x: stair.x + direction * run as i32,
            y: stair.y - depth,
            z: stair.z,
        };
    }
    staircase(bounds, stair_base(bounds, base, pass), 0).map(|_| base)
}

/// `at` lies in the open staircase of a lower level of this lane: the miner's
/// own earlier shaft, not a cave.
fn lower_staircase(bounds: ScriptWorldBounds, entrance: BlockPos, level: u8, at: BlockPos) -> bool {
    (0..level).any(|lower| {
        let Some((direction, run, depth)) = staircase(bounds, entrance, lower) else {
            return false;
        };
        let index = (at.x - entrance.x) * direction - 1;
        at.z == entrance.z
            && (0..run as i32).contains(&index)
            && (entrance.y - (index + 1).min(depth)..entrance.y - (index + 1).min(depth) + 3)
                .contains(&at.y)
    })
}

fn cell(bounds: ScriptWorldBounds, entrance: BlockPos, index: u32, level: u8) -> Option<BlockPos> {
    let left = bounds.min[0] + 1;
    let right = bounds.max[0] - 1;
    let (direction, run, depth) = staircase(bounds, entrance, level)?;
    let edge = if direction > 0 { right } else { left };
    let z_direction = z_direction(bounds, entrance.z);
    let p = if index < run {
        let past_stairs = index as i32 + 1 > depth;
        BlockPos {
            x: entrance.x + direction * (index as i32 + 1),
            y: entrance.y - (index as i32 + 1).min(depth),
            // A higher level's stair is the lower levels' open staircase; its
            // corridor beside the lane keeps a solid floor, where the same
            // cells above the continuing stair would hang over its air.
            z: entrance.z
                + if past_stairs && level > 0 {
                    z_direction
                } else {
                    0
                },
        }
    } else {
        let width = right - left;
        if width <= 0 {
            return None;
        }
        let n = index - run;
        let stride = width as u32 + 3;
        let row = n / stride;
        if row > (bounds.max[2] - bounds.min[2]) as u32 {
            return None;
        }
        let offset = n % stride;
        let from = if row.is_multiple_of(2) {
            edge
        } else if edge == right {
            left
        } else {
            right
        };
        let across = if from == right { -1 } else { 1 };
        BlockPos {
            x: from + across * (offset as i32 - 2).max(0),
            y: entrance.y - depth,
            z: entrance.z + z_direction * (row as i32 * 3 + (offset as i32 + 1).min(3)),
        }
    };
    (inside(bounds, p) && p.z > bounds.min[2] && p.z < bounds.max[2]).then_some(p)
}

fn block_path(state: &TickState, pos: BlockPos) -> Option<&str> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    Some(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(id))?
            .block
            .id
            .path(),
    )
}

fn natural(path: &str) -> bool {
    matches!(
        path,
        "stone"
            | "deepslate"
            | "granite"
            | "diorite"
            | "andesite"
            | "tuff"
            | "dirt"
            | "grass_block"
            | "gravel"
            | "sand"
            | "calcite"
    ) || path.ends_with("_ore")
}

fn drop_name(path: &str) -> &str {
    match path {
        "grass_block" => "dirt",
        "stone" => "cobblestone",
        "deepslate" => "cobbled_deepslate",
        "coal_ore" | "deepslate_coal_ore" => "coal",
        "iron_ore" | "deepslate_iron_ore" => "raw_iron",
        "copper_ore" | "deepslate_copper_ore" => "raw_copper",
        "gold_ore" | "deepslate_gold_ore" => "raw_gold",
        "diamond_ore" | "deepslate_diamond_ore" => "diamond",
        "emerald_ore" | "deepslate_emerald_ore" => "emerald",
        "redstone_ore" | "deepslate_redstone_ore" => "redstone",
        "lapis_ore" | "deepslate_lapis_ore" => "lapis_lazuli",
        other => other,
    }
}

fn clear(path: &str) -> bool {
    matches!(path, "air" | "cave_air" | "void_air")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Obstruction {
    Unloaded,
    Permanent,
}

fn probe(state: &TickState, p: BlockPos) -> Result<(u32, &str), Obstruction> {
    let id = state
        .world
        .block_at(p.x, p.y, p.z)
        .ok_or(Obstruction::Unloaded)?;
    let path = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .ok_or(Obstruction::Permanent)?
        .block
        .id
        .path();
    Ok((id, path))
}

fn unsafe_fluid(state: &TickState, id: u32, path: &str) -> bool {
    crate::fluids::contained(state, id).is_some()
        || matches!(
            path,
            "kelp" | "kelp_plant" | "seagrass" | "tall_seagrass" | "bubble_column" | "powder_snow"
        )
}

/// Reject fluids, voids and caves before opening any face. Missing chunks
/// pause the same cell; permanent obstructions abandon this staircase lane.
#[allow(clippy::too_many_arguments)]
fn safe(
    state: &TickState,
    bounds: ScriptWorldBounds,
    entrance: BlockPos,
    current: BlockPos,
    previous: BlockPos,
    height: i32,
    layer: u8,
    level: u8,
) -> Result<(), Obstruction> {
    for y in current.y..current.y + height {
        let p = BlockPos { y, ..current };
        let (id, path) = probe(state, p)?;
        // Surface entrances can use air above the certificate, never excavate
        // that space. Every actual block removal remains inside the mine.
        if !inside(bounds, p) && !clear(path) {
            return Err(Obstruction::Permanent);
        }
        if unsafe_fluid(state, id, path) {
            return Err(Obstruction::Permanent);
        }
        let already = y > current.y + height - 1 - i32::from(layer);
        let surface_air = clear(path) && y >= entrance.y;
        let own = clear(path)
            && (lower_staircase(bounds, entrance, level, p)
                || own_tunnel(state, bounds, entrance, p));
        if (already && !clear(path)) || (!already && !surface_air && !own && !natural(path)) {
            return Err(Obstruction::Permanent);
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let neighbor = BlockPos {
                x: p.x + dx,
                y: p.y + dy,
                z: p.z + dz,
            };
            if neighbor.x == current.x
                && neighbor.z == current.z
                && (current.y..current.y + height).contains(&neighbor.y)
            {
                continue;
            }
            let (id, path) = probe(state, neighbor)?;
            if unsafe_fluid(state, id, path)
                || (clear(path)
                    && neighbor.y < entrance.y
                    && !(neighbor.x == previous.x
                        && neighbor.z == previous.z
                        && (previous.y..previous.y + 3).contains(&neighbor.y))
                    && !lower_staircase(bounds, entrance, level, neighbor)
                    && !own_tunnel(state, bounds, entrance, neighbor))
            {
                return Err(Obstruction::Permanent);
            }
        }
    }
    Ok(())
}

/// Check the first actual staircase cut before installing a natural mine job.
pub(crate) fn initial_cut_safe(
    state: &TickState,
    bounds: ScriptWorldBounds,
    entrance: BlockPos,
) -> bool {
    cell_in_flight(bounds, entrance, 0, 0, 0)
        .is_some_and(|current| safe(state, bounds, entrance, current, entrance, 3, 0, 0).is_ok())
}

/// Open cells of an earlier tunnel of this mine, flooded from `start`: it
/// stays inside the zone, is small, opens onto the surface under the entrance
/// (never a sealed pocket) and touches no fluid. Anything else is a cave.
fn own_tunnel(
    state: &TickState,
    bounds: ScriptWorldBounds,
    entrance: BlockPos,
    start: BlockPos,
) -> bool {
    let mut seen = rustc_hash::FxHashSet::default();
    let mut stack = vec![start];
    let mut surface = false;
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if seen.len() > TUNNEL_CELLS || !inside(bounds, p) {
            return false;
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let q = BlockPos {
                x: p.x + dx,
                y: p.y + dy,
                z: p.z + dz,
            };
            let Ok((id, path)) = probe(state, q) else {
                return false;
            };
            if unsafe_fluid(state, id, path) {
                return false;
            }
            if !clear(path) {
                continue;
            }
            if q.y >= entrance.y {
                surface = true;
            } else {
                stack.push(q);
            }
        }
    }
    surface && seen.len() >= MIN_TUNNEL_CELLS
}

/// Every cell of the column is already open air with no fluid in it or beside
/// it: the miner's own earlier tunnel, which `safe` would reject as a cave.
fn open_and_dry(state: &TickState, current: BlockPos, height: i32) -> bool {
    (current.y..current.y + height).all(|y| {
        [(0, 0, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)]
            .into_iter()
            .chain(
                (y == current.y)
                    .then_some((0, -1, 0))
                    .into_iter()
                    .chain((y == current.y + height - 1).then_some((0, 1, 0))),
            )
            .all(|(dx, dy, dz)| {
                let p = BlockPos {
                    x: current.x + dx,
                    y: y + dy,
                    z: current.z + dz,
                };
                probe(state, p).is_ok_and(|(id, path)| {
                    !unsafe_fluid(state, id, path) && ((dx, dy, dz) != (0, 0, 0) || clear(path))
                })
            })
    })
}

/// A hostile mob within its sensor reach of `cell` (plus the cell's own height
/// and the miner's dig reach). It never has to reach the miner: residents panic
/// at any hostile in range (a zombie in a cave under the quarry), so the miner
/// would flee back and forth from a cell it can never dig.
fn hostile_near(state: &TickState, cell: BlockPos) -> bool {
    let centre = [
        f64::from(cell.x) + 0.5,
        f64::from(cell.y) + 0.5,
        f64::from(cell.z) + 0.5,
    ];
    let reach = |type_id: u32| {
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).and_then(
            |contract| {
                crate::settlements::allocator::threat_range(
                    contract
                        .name
                        .strip_prefix("minecraft:")
                        .unwrap_or(contract.name),
                )
            },
        )
    };
    state
        .entities
        .mob_slots_matching(|type_id| reach(type_id).is_some())
        .into_iter()
        .any(|mob| {
            let body = state.entities.body(mob);
            let crate::entities::EntityKind::Mob { type_id } = body.kind else {
                return false;
            };
            let at = state.entities.pos(mob);
            let range = reach(type_id).unwrap_or(0.0) + 3.0;
            body.alive
                && (at[0] - centre[0]).powi(2)
                    + (at[1] - centre[1]).powi(2)
                    + (at[2] - centre[2]).powi(2)
                    <= range * range
        })
}

/// Each permanently blocked branch retries a different surface entrance column,
/// without treating the skipped solid cell as a traversable route waypoint.
fn lane_entrance(bounds: ScriptWorldBounds, entrance: BlockPos, lane: u32) -> Option<BlockPos> {
    let width = bounds.max[2] - bounds.min[2] - 1;
    if width <= 0 || lane >= width as u32 {
        return None;
    }
    let offset = (entrance.z - bounds.min[2] - 1).clamp(0, width - 1);
    Some(BlockPos {
        z: bounds.min[2] + 1 + (offset + lane as i32) % width,
        ..entrance
    })
}

/// The standing surface of an alternate entrance column: ground with two clear
/// cells above it. Terrain slopes, so the original entrance's height would put
/// the new branch's origin underground, where the miner seals itself in.
fn surface_origin(
    state: &TickState,
    bounds: ScriptWorldBounds,
    column: BlockPos,
) -> Option<BlockPos> {
    let open = |y: i32| block_path(state, BlockPos { y, ..column }).is_some_and(clear);
    (bounds.min[1] + 2..=bounds.max[1] - 1)
        .rev()
        .find(|&y| {
            !open(y - 1)
                && block_path(state, BlockPos { y: y - 1, ..column }).is_some()
                && open(y)
                && open(y + 1)
        })
        .map(|y| BlockPos { y, ..column })
}

/// How far below its certified floor a mine may quarry, as long as the cut
/// stays inside the settlement sphere: flight after flight, so the mine keeps
/// producing instead of running dry after a few levels.
const QUARRY_DEPTH: i32 = 48;

/// A built entrance certifies the surface, not a pre-dug quarry, and a natural
/// zone's depth is finite: derive the underground work volume without enlarging
/// the building's reserved footprint or persisting another bounds authority.
fn operational_bounds(
    state: &TickState,
    building: &buildings::Building,
) -> Option<ScriptWorldBounds> {
    let mut bounds = building.bounds;
    let record = state.settlements.get(building.settlement)?;
    let dx = u64::from(
        bounds.min[0]
            .abs_diff(record.anchor[0])
            .max(bounds.max[0].abs_diff(record.anchor[0])),
    );
    let dz = u64::from(
        bounds.min[2]
            .abs_diff(record.anchor[2])
            .max(bounds.max[2].abs_diff(record.anchor[2])),
    );
    let horizontal = dx.checked_mul(dx)?.checked_add(dz.checked_mul(dz)?);
    let vertical = horizontal.and_then(|h| u64::from(record.radius).pow(2).checked_sub(h));
    let Some(vertical) = vertical else {
        // A natural zone may lie beyond the sphere: it keeps its own depth.
        return building.natural_zone.then_some(bounds);
    };
    let sphere_floor = (i64::from(record.anchor[1]) - vertical.isqrt() as i64)
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    bounds.min[1] = bounds.min[1]
        .saturating_sub(QUARRY_DEPTH)
        .max(sphere_floor.min(bounds.min[1]));
    Some(bounds)
}

pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    changes: &mut ChangeSet,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Miner
        || v.age < 0
        || v.sleeping
        || v.activity == Activity::Panic
    {
        return None;
    }
    let building = buildings::building_for_job(state, v)?;
    if building.kind != buildings::BuildingKind::Mine {
        return None;
    }
    let bounds = operational_bounds(state, building)?;
    // The shaft starts in the floor interior, inside the footprint the
    // upgrade fences: digging it mid-upgrade pauses the structure for good.
    let (id, revision, surface_bounds, upgrading) = (
        building.id.0,
        building.revision,
        if building.natural_zone { building.bounds } else { bounds },
        state
            .settlements
            .get(building.settlement)
            .and_then(|record| record.project.as_ref())
            .is_some_and(|project| project.building == Some(building.id)),
    );
    let entrance = v.job_site?;
    if v.mining_progress.building != id || v.mining_progress.bounds != Some(bounds) {
        v.mining_progress = Progress {
            building: id,
            revision,
            bounds: Some(bounds),
            ..Progress::default()
        };
    }
    v.mining_progress.revision = revision;
    let working =
        v.activity == Activity::Work && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000));
    if !working {
        v.mining_progress.stalled_since = 0;
    }
    if !working || heavy_load(v) {
        v.mining_progress.returning = true;
    }
    if v.mining_progress.returning {
        let target = deposit(state, slot, v);
        v.mining_progress.returning = v.inventory.iter().any(Option::is_some);
        return target;
    }
    if !working
        || upgrading
        || !crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING)
    {
        return None;
    }
    let level = v.mining_progress.level;
    let pass = v.mining_progress.pass;
    let Some(base) = pass_entrance(bounds, entrance, pass) else {
        // Every flight down to the bottom of the zone has been quarried; what
        // the miner still carries goes to storage instead of staying with it.
        v.mining_progress.returning = v.inventory.iter().any(Option::is_some);
        return deposit(state, slot, v);
    };
    if staircase(bounds, stair_base(bounds, base, pass), level).is_none() {
        // Every level of this flight is done: the next flight starts where this
        // one ended, deeper, so the quarry reaches the bottom of the zone.
        v.mining_progress.pass = pass.saturating_add(1);
        v.mining_progress.level = 0;
        v.mining_progress.lane = 0;
        v.mining_progress.cell = 0;
        v.mining_progress.layer = 0;
        v.mining_progress.last_open = None;
        return None;
    }
    let column = if pass == 0 || v.mining_progress.lane == 0 {
        lane_entrance(bounds, base, v.mining_progress.lane)
    } else {
        // Deeper flights hang off the lane-0 tunnel only: another lane's
        // flight end is not known to be open.
        None
    };
    let Some(column) = column else {
        // Every lane is closed at this level (no ground, caves, hostiles):
        // the next level lies higher, where the same lanes may be open.
        v.mining_progress.level = level.saturating_add(1);
        v.mining_progress.lane = 0;
        v.mining_progress.cell = 0;
        v.mining_progress.layer = 0;
        v.mining_progress.last_open = None;
        return None;
    };
    let origin = if column == base {
        base
    } else if pass == 0
        && let Some(origin) = surface_origin(state, surface_bounds, column)
    {
        origin
    } else {
        // No ground at this column inside the zone: try the next one.
        v.mining_progress.lane += 1;
        v.mining_progress.cell = 0;
        v.mining_progress.layer = 0;
        v.mining_progress.last_open = None;
        return None;
    };
    let stair = stair_base(bounds, origin, pass);
    let Some(current) = cell_in_flight(bounds, origin, pass, v.mining_progress.cell, level) else {
        // This comb is finished: continue one level higher through the same
        // lane (its open staircase is walked, not re-dug), where new ore and
        // stone lie, until no level fits any more.
        v.mining_progress.level = level.saturating_add(1);
        v.mining_progress.cell = 0;
        v.mining_progress.layer = 0;
        v.mining_progress.last_open = None;
        return None;
    };
    let previous = v
        .mining_progress
        .last_open
        .map(|[x, y, z]| BlockPos { x, y, z })
        .unwrap_or(origin);
    let planned_previous = v
        .mining_progress
        .cell
        .checked_sub(1)
        .and_then(|n| cell_in_flight(bounds, origin, pass, n, level))
        .unwrap_or(origin);
    let height = if current.y < planned_previous.y { 3 } else { 2 };
    if i32::from(v.mining_progress.layer) >= height {
        return None;
    }
    if state.tick < v.harvest_at {
        return Some(previous);
    }
    let hostile = hostile_near(state, current);
    let walking = !hostile && !near(state, slot, current, 3.0);
    if hostile || walking {
        // Villagers panic at any hostile in range, so the cell can never be
        // dug while it stays; a route may also be impossible (a pit under a
        // level's row, a pocket). Give it time, then close the lane.
        if v.mining_progress.stalled_since == 0 {
            v.mining_progress.stalled_since = state.tick.max(1);
        }
        if state.tick.saturating_sub(v.mining_progress.stalled_since) >= STALL_PATIENCE {
            v.mining_progress.stalled_since = 0;
            v.mining_progress.lane += 1;
            v.mining_progress.cell = 0;
            v.mining_progress.layer = 0;
            v.mining_progress.last_open = None;
            return None;
        }
        if hostile {
            return None;
        }
        // Walk toward the cell to dig once beside the open end: the route's
        // one-cell arrival tolerance around `current` always lies inside dig
        // reach, while tolerance around `previous` can strand the miner
        // diagonally out of reach. Far away, walk to the known-reachable open
        // end first: a route to the solid cell falls back to the closest
        // reachable point, which can be a pocket beneath it.
        return Some(if near(state, slot, previous, 2.0) {
            current
        } else {
            previous
        });
    }
    v.mining_progress.stalled_since = 0;
    v.harvest_at = state.tick + PERIOD;
    // Only cells inside an earlier level's own staircase are walked through.
    // A higher level's corridor ends on the cell its first row starts from.
    let walkthrough = current == previous
        || (current.y..current.y + height)
            .all(|y| lower_staircase(bounds, stair, level, BlockPos { y, ..current }));
    if walkthrough && v.mining_progress.layer == 0 && open_and_dry(state, current, height) {
        // An earlier level's own tunnel: walk through it.
        v.mining_progress.cell += 1;
        v.mining_progress.last_open = Some([current.x, current.y, current.z]);
        return Some(current);
    }
    match safe(
        state,
        bounds,
        stair,
        current,
        previous,
        height,
        v.mining_progress.layer,
        level,
    ) {
        Ok(()) => {}
        Err(Obstruction::Unloaded) => return Some(previous),
        Err(Obstruction::Permanent) => {
            v.mining_progress.lane += 1;
            v.mining_progress.cell = 0;
            v.mining_progress.layer = 0;
            v.mining_progress.last_open = None;
            return Some(previous);
        }
    }
    let pos = BlockPos {
        y: current.y + height - 1 - i32::from(v.mining_progress.layer),
        ..current
    };
    let path = block_path(state, pos)?;
    if !clear(path) {
        let item = item_id(state, drop_name(path))?;
        if room(state, v, &ItemStack::new(item, 1)) < 1 {
            v.mining_progress.returning = true;
            return deposit(state, slot, v);
        }
        let air = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:air").ok()?)?
            .default
            .0;
        collect(state, v, item, 1);
        replace(state, changes, pos, air);
    }
    v.mining_progress.layer += 1;
    if i32::from(v.mining_progress.layer) == height {
        v.mining_progress.layer = 0;
        v.mining_progress.cell += 1;
        v.mining_progress.last_open = Some([current.x, current.y, current.z]);
        return Some(current);
    }
    Some(previous)
}
