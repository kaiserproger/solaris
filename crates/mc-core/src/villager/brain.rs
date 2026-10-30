//! Villager schedules and owner-thread transactions. Constants follow the local
//! 26.1.2 villager brain, population, gossip, and village-defense oracle modules.
use super::{Activity, Gossip, VillagerProfession as Profession, VillagerState};
use crate::boundary::{BlockPos, ChangeSet, ChunkPos, EntityEvent, EntityId};
use crate::entities::{EntityKind, EntitySlot};
#[cfg(test)]
use crate::registry::CoreRegistry;
use crate::settlements::allocator::{
    self, JOB_SITE_REACH_SQUARED, PICKUP_REACH, POI_REFRESH_CHUNKS_PER_TICK, Poi, PoiCache,
    PoiKind, active, at, capture_route, centre, distance, free_bed, poi_kind, route_changed,
    standing, threat_range,
};
use crate::tick::{TickSampler, TickState};
#[cfg(test)]
use crate::tick_budget::DECISIONS_PER_TICK;
use crate::tick_budget::{Consumer, Kind};
use mc_data::ItemStack;
use mc_physics::mth;
#[cfg(test)]
use rustc_hash::FxHashSet;
use std::cmp::Reverse;
mod motion;
mod poi_cache;
pub(crate) use motion::*;
mod social;
pub(crate) use social::*;

/// Ticks over which scheduled activity switches are spread per villager.
/// Fixed (once 600 on hosts with <= 2 CPUs, 200 otherwise) so the same inputs
/// produce the same world regardless of the host's core count.
const SCHEDULE_WINDOW: u64 = 200;

// Full decisions spend the global tick allowance shared with settlement
// allocation and other dimensions; the brain never refills it locally.

// Only the ignored all-brains profile enables these timers. Release servers
// compile the expression itself, with no measurement fields or clock reads.
macro_rules! decision_phase {
    ($cache:expr, $index:expr, $body:expr) => {{
        #[cfg(test)]
        let started = $cache.measure_decisions.then(std::time::Instant::now);
        let value = $body;
        #[cfg(test)]
        if let Some(started) = started {
            $cache.decision_current[$index] += started.elapsed().as_nanos();
            $cache.decision_calls[$index] += 1;
        }
        value
    }};
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct BrainCadence {
    period: u64,
    next: u64,
    was_busy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResidentBrain {
    id: EntityId,
    chunk: ChunkPos,
    active: bool,
    near: bool,
    activity: Activity,
    profession: Profession,
    baby_until: u64,
    wake_at: u64,
    search_due: u64,
    panic_until: u64,
    protected_until: u64,
    hurt_at: Option<u64>,
    wake: bool,
    reactive: bool,
    motion: bool,
    clock_changes: bool,
    claims: [Option<BlockPos>; 3],
    cadence: BrainCadence,
    rank: usize,
    due_at: u64,
    due_version: u64,
    ready_band: Option<usize>,
    counted_near: bool,
    schedule_due: u64,
    last_full: u64,
}

impl ResidentBrain {
    fn read(state: &TickState, slot: EntitySlot) -> Self {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let pos = state.entities.pos(slot);
        let chunk = ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        };
        let loaded = state.world.is_active(chunk.x, chunk.z);
        let mut brain = Self {
            id: body.id,
            chunk,
            active: false,
            near: false,
            activity: Activity::Idle,
            profession: Profession::None,
            baby_until: 0,
            wake_at: u64::MAX,
            search_due: u64::MAX,
            panic_until: 0,
            protected_until: 0,
            hurt_at: body.hurt_at,
            wake: false,
            reactive: false,
            motion: false,
            clock_changes: false,
            claims: [None; 3],
            cadence: BrainCadence::default(),
            rank: 0,
            due_at: u64::MAX,
            due_version: 0,
            ready_band: None,
            counted_near: false,
            schedule_due: u64::MAX,
            last_full: state.tick,
        };
        let Some(v) = row.villager.as_ref() else {
            return brain;
        };
        brain.active = loaded && active(state, slot);
        brain.activity = v.activity;
        brain.profession = v.data.profession;
        // This is a no-op proof, not deferred clock projection: every clock
        // which can change still advances through the shared projection now.
        brain.clock_changes =
            v.age != 0 || v.sleeping || matches!(v.activity, Activity::Work | Activity::Panic);
        if v.age < 0 {
            brain.baby_until = state.tick.saturating_add(u64::from(v.age.unsigned_abs()));
        }
        brain.wake_at = v.last_gossip_decay.saturating_add(24_000);
        if v.age != 0 {
            brain.wake_at = brain.wake_at.min(
                state
                    .tick
                    .saturating_add(u64::from(v.age.unsigned_abs()).saturating_sub(1)),
            );
        } else if body.height < 1.0 {
            brain.wake_at = state.tick;
        }
        if v.navigation_target.is_some() {
            brain.wake_at = brain.wake_at.min(v.navigation_repath_at);
        }
        brain.panic_until = v.panic_until;
        brain.protected_until = v
            .panic_until
            .max(body.hurt_at.map_or(0, |at| at.saturating_add(100)));
        brain.reactive = v.partner.is_some() || !v.navigation_path.is_empty();
        brain.motion = !v.opened_doors.is_empty()
            || (!v.sleeping
                && (!state.entities.ground(slot)
                    || state.entities.vel(slot) != [0.0; 3]
                    || state.entities.horizontal_impulse(slot) != [0.0; 2]
                    || !v.navigation_path.is_empty()));
        if body.alive && v.conversion_time.is_none() {
            let dimension = state.dimension.name();
            brain.claims = [
                v.home.filter(|_| v.home_dimension == dimension),
                v.job_site.filter(|_| v.job_site_dimension == dimension),
                v.meeting_point
                    .filter(|_| v.meeting_point_dimension == dimension),
            ];
        }
        brain
    }
}

fn player_chunk_distance(players: &[(i32, i32)], cell: (i32, i32)) -> i32 {
    players
        .iter()
        .map(|&(x, z)| cell.0.abs_diff(x).max(cell.1.abs_diff(z)))
        .min()
        .unwrap_or(i32::MAX as u32)
        .min(i32::MAX as u32) as i32
}

fn brain_period(previous: u64, chunks: i32) -> u64 {
    // Retain a ring one chunk beyond its outer boundary; promote immediately
    // at the inner boundary. A player moving on an edge cannot flap the LOD.
    match previous {
        1 if chunks <= 3 => 1,
        2 if (3..=5).contains(&chunks) => 2,
        4 if (5..=7).contains(&chunks) => 4,
        16 if (7..=9).contains(&chunks) => 16,
        32 if chunks > 8 => 32,
        _ => match chunks {
            0..=2 => 1,
            3..=4 => 2,
            5..=6 => 4,
            7..=8 => 16,
            _ => 32,
        },
    }
}

#[derive(Clone, Copy, Default)]
struct FoodItem {
    points: u8,
    farmer_pickup: bool,
}

#[derive(Default)]
pub(crate) struct FoodTable {
    items: std::sync::Weak<mc_data::items::ItemRegistry>,
    entries: Vec<FoodItem>,
}

impl FoodTable {
    pub(crate) fn refresh(&mut self, items: &std::sync::Arc<mc_data::items::ItemRegistry>) {
        if self.items.as_ptr() == std::sync::Arc::as_ptr(items) {
            return;
        }
        self.entries.clear();
        for (_, id) in items.iter() {
            // The reverse map is authoritative, including duplicate protocol IDs.
            let name = items.name_of(id).expect("registered item has a name");
            let points = match name.path() {
                "bread" => 4,
                "carrot" | "potato" | "beetroot" => 1,
                _ => 0,
            };
            let farmer_pickup = matches!(name.path(), "wheat" | "wheat_seeds" | "beetroot_seeds");
            if points == 0 && !farmer_pickup {
                continue;
            }
            let index = id as usize;
            if self.entries.len() <= index {
                self.entries.resize(index + 1, FoodItem::default());
            }
            self.entries[index] = FoodItem {
                points,
                farmer_pickup,
            };
        }
        // Keeping a weak reference prevents allocation reuse and makes
        // Arc::make_mut extensions acquire a new registry generation.
        self.items = std::sync::Arc::downgrade(items);
    }
}

pub(super) fn bodies_near(
    state: &TickState,
    pos: [f64; 3],
    radius: f64,
    residents_only: bool,
) -> impl Iterator<Item = EntitySlot> + '_ {
    let min_x = ((pos[0] - radius).floor() as i32).div_euclid(16);
    let max_x = ((pos[0] + radius).floor() as i32).div_euclid(16);
    let min_z = ((pos[2] - radius).floor() as i32).div_euclid(16);
    let max_z = ((pos[2] + radius).floor() as i32).div_euclid(16);
    (min_x..=max_x).flat_map(move |x| {
        (min_z..=max_z).flat_map(move |z| {
            let chunk = ChunkPos { x, z };
            let villagers = state.entities.villager_chunks().get(&chunk);
            let sensors = if residents_only {
                None
            } else {
                state.entities.brain_sensor_chunks().get(&chunk)
            };
            villagers
                .into_iter()
                .chain(sensors)
                .flat_map(|ids| ids.iter().copied())
                .filter_map(|id| state.entities.slot_of(id))
        })
    })
}

fn roofed(state: &TickState, pos: BlockPos) -> bool {
    (pos.y + 1..state.world.geometry().max_y()).any(|y| {
        state
            .world
            .block_at(pos.x, y, pos.z)
            .and_then(|id| state.registry.collision_boxes.get(id as usize))
            .is_some_and(|shapes| !shapes.is_empty())
    })
}

/// A roofed bottom stair beside the villager with body room above it (a stair
/// under a low ceiling or inside a wall is not a seat).
fn stair_seat(state: &TickState, slot: EntitySlot) -> Option<BlockPos> {
    let origin = at(state.entities.pos(slot));
    let body = state.entities.body(slot);
    for dx in -1..=1 {
        for dz in -1..=1 {
            for dy in -1..=0 {
                let seat = BlockPos {
                    x: origin.x + dx,
                    y: origin.y + dy,
                    z: origin.z + dz,
                };
                let Some(block) = state
                    .world
                    .block_at(seat.x, seat.y, seat.z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                else {
                    continue;
                };
                if block.block.id.path().ends_with("_stairs")
                    && block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "half" && value == "bottom")
                    && roofed(state, seat)
                    && standing(
                        &state.world,
                        &state.registry,
                        BlockPos {
                            y: seat.y + 1,
                            ..seat
                        },
                        body.half_width,
                        body.height,
                    )
                    .is_some()
                {
                    return Some(seat);
                }
            }
        }
    }
    None
}

fn night_shelter(
    state: &mut TickState,
    cache: &mut PoiCache,
    slot: EntitySlot,
    v: &mut VillagerState,
    pos: [f64; 3],
) -> (Option<BlockPos>, bool) {
    let Some(record) = v.settlement.and_then(|id| state.settlements.get(id)) else {
        return (None, true);
    };
    let body = state.entities.body(slot);
    let (width, height) = (body.half_width, body.height);
    let mut candidates = std::mem::take(&mut cache.candidates);
    candidates.clear();
    for poi in cache.chunks.values().flat_map(|chunk| &chunk.pois) {
        let bed = poi.pos;
        if poi.kind != PoiKind::Home
            || !record.contains([bed.x, bed.y, bed.z])
            || !roofed(state, bed)
        {
            continue;
        }
        let mut cells = [bed; 17];
        let mut count = 1;
        let mut next = 0;
        while next < count && count < cells.len() {
            let origin = cells[next];
            next += 1;
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let target = BlockPos {
                    x: origin.x + dx,
                    y: bed.y,
                    z: origin.z + dz,
                };
                if cells[..count].contains(&target)
                    || !roofed(state, target)
                    || standing(&state.world, &state.registry, target, width, height).is_none()
                {
                    continue;
                }
                cells[count] = target;
                count += 1;
                let reserved = cache.residents.iter().copied().any(|other| {
                    other != slot
                        && state
                            .entities
                            .row(other)
                            .villager
                            .as_ref()
                            .is_some_and(|resident| {
                                resident.navigation_target == Some(target)
                                    || at(state.entities.pos(other)) == target
                            })
                });
                let doorway = state
                    .world
                    .block_at(target.x, target.y, target.z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|block| block.block.id.path().ends_with("_door"));
                if !reserved && !doorway {
                    if v.navigation_target == Some(target) {
                        cache.candidates = candidates;
                        return (Some(target), true);
                    }
                    candidates.push((
                        distance(pos, centre(bed)),
                        Poi {
                            pos: target,
                            kind: PoiKind::Home,
                        },
                    ));
                }
                if count == cells.len() {
                    break;
                }
            }
        }
    }
    candidates.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| (a.1.pos.x, a.1.pos.y, a.1.pos.z).cmp(&(b.1.pos.x, b.1.pos.y, b.1.pos.z)))
    });
    let mut path = std::mem::take(&mut cache.candidate_path);
    let mut selected = None;
    for &(_, poi) in &candidates {
        if !cache.spend_path(state, Consumer::Brain) {
            break;
        }
        let from = at(pos);
        let horizontal = from
            .x
            .abs_diff(poi.pos.x)
            .max(from.z.abs_diff(poi.pos.z))
            .saturating_add(8)
            .clamp(48, 512);
        let vertical = from.y.abs_diff(poi.pos.y).saturating_add(4).clamp(16, 512);
        let mut ctx = allocator::RouteCtx {
            world: &state.world,
            registry: &state.registry,
            dimension: state.dimension,
            consumer: Consumer::Brain,
            budget: &mut state.tick_budget,
        };
        if allocator::find_path_ctx(
            &mut ctx, cache, pos, poi.pos, width, height, &mut path, true, horizontal, vertical,
            true,
        ) {
            std::mem::swap(&mut path, &mut v.navigation_path);
            v.navigation_target = Some(poi.pos);
            v.navigation_repath_at = state.tick + 40;
            capture_route(state, v, pos, true);
            selected = Some(poi.pos);
            break;
        }
    }
    path.clear();
    cache.candidate_path = path;
    candidates.clear();
    cache.candidates = candidates;
    (selected, true)
}

pub(super) fn schedule(age: i32, profession: Profession, day_time: i64) -> Activity {
    let time = day_time.rem_euclid(24_000);
    if profession == Profession::Soldier && age >= 0 {
        return Activity::Work;
    }
    if profession == Profession::Nitwit && age >= 0 {
        return if !(2_000..14_000).contains(&time) {
            Activity::Rest
        } else {
            Activity::Idle
        };
    }
    if age < 0 {
        match time {
            0..=9 | 12_000.. => Activity::Rest,
            3_000..=5_999 | 10_000..=11_999 => Activity::Play,
            _ => Activity::Idle,
        }
    } else {
        match time {
            0..=9 | 12_000.. => Activity::Rest,
            2_000..=8_999 => Activity::Work,
            9_000..=10_999 => Activity::Meet,
            _ => Activity::Idle,
        }
    }
}

fn clear_step(state: &TickState, begin: [f64; 3], end: [f64; 3], width: f64, height: f64) -> bool {
    // A jump needs headroom over the starting column; a drop crosses the
    // edge horizontally before falling. Keep A* and short strolls identical.
    let travel_y = begin[1].max(end[1]);
    allocator::clear_body_for_route(
        &(state).world,
        &(state).registry,
        [begin[0], travel_y, begin[2]],
        width,
        height,
        true,
    ) && allocator::clear_body_for_route(
        &(state).world,
        &(state).registry,
        [
            (begin[0] + end[0]) * 0.5,
            travel_y,
            (begin[2] + end[2]) * 0.5,
        ],
        width,
        height,
        true,
    )
}

fn wander_to(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    target: Option<BlockPos>,
    scheduled: Activity,
) {
    let pos = state.entities.pos(slot);
    let mut unchanged = false;
    if let Some(target) = target {
        let new_target = v.navigation_target != Some(target);
        let due = state.tick >= v.navigation_repath_at;
        unchanged = !v.navigation_path.is_empty() && !route_changed(state, v);
        // Failed/exhausted strolls keep their deadline, even when an unknown
        // neighboring chunk would otherwise report invalidation every tick.
        if new_target || due || (!v.navigation_path.is_empty() && !unchanged) {
            let body = state.entities.body(slot);
            let (width, height) = (body.half_width, body.height);
            let start = at(pos);
            let mut cell = start;
            let nx = start.x.abs_diff(target.x);
            let nz = start.z.abs_diff(target.z);
            let mut walked_x = 0;
            let mut walked_z = 0;
            v.navigation_path.clear();
            if nx.saturating_add(nz) <= 6 {
                while cell.x != target.x || cell.z != target.z {
                    // Cardinal rasterization of the straight segment: never
                    // cut a diagonal corner or search around an obstruction.
                    let along_x = walked_z == nz
                        || (walked_x < nx && (2 * walked_x + 1) * nz <= (2 * walked_z + 1) * nx);
                    let (dx, dz) = if along_x {
                        walked_x += 1;
                        ((target.x - cell.x).signum(), 0)
                    } else {
                        walked_z += 1;
                        (0, (target.z - cell.z).signum())
                    };
                    let begin = allocator::standing_for_route(
                        &(state).world,
                        &(state).registry,
                        cell,
                        width,
                        height,
                        true,
                    )
                    .unwrap_or_else(|| centre(cell));
                    let next = [0, 1, -1, -2, -3].into_iter().find_map(|dy| {
                        let next = BlockPos {
                            x: cell.x + dx,
                            y: cell.y + dy,
                            z: cell.z + dz,
                        };
                        let end = allocator::standing_for_route(
                            &(state).world,
                            &(state).registry,
                            next,
                            width,
                            height,
                            true,
                        )?;
                        clear_step(state, begin, end, width, height).then_some(next)
                    });
                    let Some(next) = next else {
                        // An incomplete stroll must not move toward the wall.
                        v.navigation_path.clear();
                        break;
                    };
                    v.navigation_path.push(next);
                    cell = next;
                }
            }
            v.navigation_path.reverse();
            v.navigation_target = Some(target);
            if new_target || due {
                v.navigation_repath_at = state.tick + 40;
            }
            capture_route(state, v, pos, true);
            unchanged = !route_changed(state, v);
        }
    } else {
        v.navigation_path.clear();
        v.navigation_target = None;
        v.navigation_chunks.clear();
    }
    move_committed_with_route(state, slot, v, scheduled, unchanged);
}

fn nearest_threat(cache: &PoiCache, pos: [f64; 3]) -> Option<[f64; 3]> {
    let range = cache.threat_max_range;
    let min_x = ((pos[0] - range).floor() as i32).div_euclid(16);
    let max_x = ((pos[0] + range).floor() as i32).div_euclid(16);
    let min_z = ((pos[2] - range).floor() as i32).div_euclid(16);
    let max_z = ((pos[2] + range).floor() as i32).div_euclid(16);
    (min_x..=max_x)
        .flat_map(|x| (min_z..=max_z).map(move |z| (x, z)))
        .filter_map(|chunk| cache.threat_chunks.get(&chunk))
        .flatten()
        .filter_map(|&index| {
            let (at, range) = cache.threats[index];
            let d = distance(pos, at);
            (d <= range * range).then_some((d, index, at))
        })
        // Chunk traversal must not change the original sensor-order tie break.
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)))
        .map(|(_, _, at)| at)
}
/// Called after merge effects and conversion, before publication. No player
/// item reservations survive here; all villager mutations have one owner.
pub(crate) fn tick(state: &mut TickState, changes: &mut ChangeSet) {
    #[cfg(test)]
    let brain_start = std::time::Instant::now();
    if state.entities.villager_chunks().is_empty() {
        state.tick_budget.finish(state.dimension, Consumer::Brain);
        return;
    }
    // The allocator ran immediately before this tick and owns every POI
    // memory; the brain never acquires or validates claims itself.
    let mut cache = std::mem::take(&mut state.villager_pois);
    #[cfg(test)]
    let refreshed = brain_start.elapsed().as_micros();
    cache.prepare_brains(state);
    #[cfg(test)]
    let prepared = brain_start.elapsed().as_micros();
    #[cfg(test)]
    let acquired = brain_start.elapsed().as_micros();
    #[cfg(test)]
    let mut full_ns = 0;
    for execution in 0..cache.execution_indices.len() {
        let index = cache.execution_indices[execution];
        if !cache.resident_data[index].active {
            continue;
        }
        let slot = cache.residents[index];
        let id = cache.resident_data[index].id;
        let merchant_busy = cache.merchants.binary_search(&id).is_ok();
        let scheduled = cache.activity_for(
            if state.tick < cache.resident_data[index].baby_until {
                -1
            } else {
                0
            },
            cache.resident_data[index].profession,
            state.day_time,
        );
        let clear_seat = state.entities.row(slot).villager.as_ref().is_some_and(|v| {
            v.seat.is_some_and(|seat| {
                merchant_busy
                    || v.sleeping
                    || state.tick < v.panic_until
                    || !matches!(
                        schedule(v.age, v.data.profession, state.day_time),
                        Activity::Rest | Activity::Idle
                    )
                    || distance(state.entities.pos(slot), centre(seat)) > 4.0
                    || stair_seat(state, slot) != Some(seat)
            })
        });
        if clear_seat {
            state.entities.row_mut(slot).villager.as_mut().unwrap().seat = None;
        }
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .is_some_and(|v| !v.opened_doors.is_empty() || !v.navigation_path.is_empty())
        {
            let mut v = state
                .entities
                .row_mut_unmarked(slot)
                .villager
                .take()
                .unwrap();
            super::doors::interact(state, slot, &mut v, changes);
            state.entities.row_mut_unmarked(slot).villager = Some(v);
        }
        if !cache.admitted(slot) {
            let brain = &mut cache.resident_data[index];
            if brain.clock_changes {
                let was_baby = state
                    .entities
                    .row(slot)
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.age < 0);
                if let Some(panic_until) = state.entities.advance_villager_clocks(
                    slot,
                    state.tick,
                    state.tick.saturating_add(1),
                ) {
                    brain.panic_until = panic_until;
                    brain.protected_until = brain.protected_until.max(panic_until);
                    // Only the baby -> adult crossing changes the wire projection.
                    if was_baby
                        && state
                            .entities
                            .row(slot)
                            .villager
                            .as_ref()
                            .is_some_and(|v| v.age >= 0)
                    {
                        super::sync_appearance(state, slot);
                    }
                }
            }
            if brain.motion || cache.motion_chunks.contains(&brain.chunk) {
                move_skipped(
                    state,
                    slot,
                    cache.motion_chunks.contains(&brain.chunk),
                    merchant_busy,
                    scheduled,
                );
                let row = state.entities.row(slot);
                brain.motion = row.villager.as_ref().is_some_and(|v| {
                    !v.opened_doors.is_empty()
                        || (!v.sleeping
                            && (!state.entities.ground(slot)
                                || state.entities.vel(slot) != [0.0; 3]
                                || state.entities.horizontal_impulse(slot) != [0.0; 2]
                                || !v.navigation_path.is_empty()))
                });
            }
            cache.update_execution(index);
            continue;
        }
        #[cfg(test)]
        if cache.measure_decisions {
            cache.measure_actor = Some(id);
            cache.decision_current = [0; 8];
        }
        #[cfg(test)]
        let full_start = std::time::Instant::now();
        let pos = state.entities.pos(slot);
        let mut v = state.entities.row_mut(slot).villager.take().unwrap();
        let baby = v.age < 0;
        let old_data = v.data;
        let old_activity = v.activity;
        v.age -= v.age.signum();
        decay_gossip(&mut v, state.tick);
        // POI memories are allocator-owned (validated + acquired in the
        // allocator pass immediately before this tick): the brain only reads
        // them through the local-dimension filter below.
        let home = v
            .home
            .filter(|_| v.home_dimension == state.dimension.name());
        let job_site = v
            .job_site
            .filter(|_| v.job_site_dimension == state.dimension.name());
        let meeting_point = v
            .meeting_point
            .filter(|_| v.meeting_point_dimension == state.dimension.name());
        let assigned_profession = job_site.and_then(|job| {
            if v.settlement.is_some() {
                crate::settlements::buildings::job_profession(&state.settlements, v.settlement, job)
            } else {
                state
                    .world
                    .block_at(job.x, job.y, job.z)
                    .and_then(|id| match cache.kind(id) {
                        Some(PoiKind::Job(profession)) => Some(profession),
                        _ => None,
                    })
            }
        });
        if let Some(job) = job_site
            && v.age >= 0
            && !matches!(v.data.profession, Profession::Builder | Profession::Soldier)
            && !crate::settlements::allocator::traded(&v)
            && distance(pos, centre(job)) <= JOB_SITE_REACH_SQUARED
            && let Some(profession) = assigned_profession
            && v.data.profession != profession
            && crate::settlements::allocator::confirm_profession(
                state,
                &cache,
                id,
                v.settlement,
                job,
                profession,
            )
        {
            v.data.profession = profession;
            v.offers.clear();
        }
        let threat = decision_phase!(cache, 2, nearest_threat(&cache, pos));
        if v.data.profession == Profession::Soldier {
            v.panic_until = 0;
        } else {
            if let Some(hurt_at) = state.entities.body(slot).hurt_at {
                v.panic_until = v.panic_until.max(hurt_at.saturating_add(100));
            }
            if threat.is_some() {
                v.panic_until = state.tick.saturating_add(100);
            }
        }
        v.activity = if v.data.profession == Profession::Soldier && v.age >= 0 {
            Activity::Work
        } else if state.tick < v.panic_until {
            Activity::Panic
        } else {
            cache.activity_for(v.age, v.data.profession, state.day_time)
        };
        // Residents with a roofed home bed go to that bed; the rest shelter beside one.
        let shelter_night = v.settlement.is_some()
            && (12_000..24_000).contains(&state.day_time.rem_euclid(24_000))
            && matches!(v.activity, Activity::Rest | Activity::Idle)
            && !home.is_some_and(|bed| roofed(state, bed));
        if shelter_night {
            v.activity = Activity::Rest;
        }
        // Reach the newly assigned workstation before running an old resource
        // role: its independent work target must not prevent profession takeover.
        let awaiting_profession = v.activity == Activity::Work
            && !matches!(v.data.profession, Profession::Builder | Profession::Soldier)
            && !crate::settlements::allocator::traded(&v)
            && assigned_profession.is_some_and(|profession| profession != v.data.profession);
        if old_activity != v.activity {
            v.navigation_target = None;
            v.navigation_repath_at = 0;
            v.navigation_path.clear();
            v.navigation_chunks.clear();
        }
        let sleep_home_allowed = !shelter_night;
        let was_sleeping = v.sleeping;
        if merchant_busy || v.activity != Activity::Rest || home.is_none() || !sleep_home_allowed {
            v.sleeping = false;
        }
        if !merchant_busy
            && sleep_home_allowed
            && !v.sleeping
            && v.activity == Activity::Rest
            && let Some(home) = home
        {
            let available = decision_phase!(cache, 1, {
                !cache.occupied_beds.contains(&home)
                    && state.world.block_at(home.x, home.y, home.z).is_some()
            });
            if available && distance(pos, centre(home)) <= 4.0 {
                v.sleeping = true;
                state.entities.set_pos(
                    slot,
                    [
                        f64::from(home.x) + 0.5,
                        f64::from(home.y) + 0.6875,
                        f64::from(home.z) + 0.5,
                    ],
                );
                state.entities.set_vel(slot, [0.0; 3]);
            }
        }
        if was_sleeping && !v.sleeping {
            let (width, height) = if v.age < 0 {
                (0.15, 0.975)
            } else {
                (0.3, 1.95)
            };
            state.entities.set_half_width(slot, width);
            state.entities.body_mut(slot).height = height;
            let cell = at(pos);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                if let Some(p) = standing(
                    &state.world,
                    &state.registry,
                    BlockPos {
                        x: cell.x + dx,
                        y: cell.y,
                        z: cell.z + dz,
                    },
                    width,
                    height,
                ) {
                    state.entities.set_pos(slot, p);
                    break;
                }
            }
        }
        let changing_seated_activity = old_activity != v.activity && v.seat.is_some();
        let can_sit = !merchant_busy
            && !v.sleeping
            && !changing_seated_activity
            && matches!(v.activity, Activity::Rest | Activity::Idle)
            && v.navigation_path.is_empty();
        if !can_sit
            || v.seat.is_some_and(|seat| {
                distance(state.entities.pos(slot), centre(seat)) > 4.0
                    || stair_seat(state, slot) != Some(seat)
            })
        {
            v.seat = None;
        }
        if can_sit && v.seat.is_none() {
            v.seat = stair_seat(state, slot).filter(|seat| {
                !cache.residents.iter().copied().any(|other| {
                    other != slot
                        && state
                            .entities
                            .row(other)
                            .villager
                            .as_ref()
                            .is_some_and(|resident| resident.seat == Some(*seat))
                })
            });
            if let Some(seat) = v.seat {
                state.entities.set_pos(
                    slot,
                    [
                        f64::from(seat.x) + 0.5,
                        f64::from(seat.y) + 0.5,
                        f64::from(seat.z) + 0.5,
                    ],
                );
                state.entities.set_vel(slot, [0.0; 3]);
                v.navigation_target = None;
            }
        }
        if v.sleeping {
            v.last_slept = Some(state.tick);
        } else if v.seat.is_none() {
            let pickup_pos = state.entities.pos(slot);
            decision_phase!(cache, 3, {
                cache.items_near(pickup_pos);
                pickup(
                    state,
                    &cache.food,
                    &cache.nearby_items,
                    slot,
                    &mut v,
                    changes,
                )
            });
            decision_phase!(cache, 4, {
                super::harvest::work(state, slot, &mut v, changes);
                share_food(state, &cache.food, slot, &mut v, changes)
            });
            let mut worker_exact = false;
            let worker_target = if merchant_busy || awaiting_profession {
                None
            } else {
                match v.data.profession {
                    Profession::Miner => super::mining::work(state, slot, &mut v, changes),
                    Profession::Lumberjack => super::lumber::work(state, slot, &mut v, changes),
                    Profession::Builder => {
                        super::building::work(state, slot, &mut v, changes).map(|target| {
                            worker_exact = target.exact;
                            target.pos
                        })
                    }
                    Profession::Soldier => {
                        super::soldier::work(state, slot, &mut v).map(|(target, exact)| {
                            worker_exact = exact;
                            target
                        })
                    }
                    Profession::Farmer => super::harvest::deposit(state, slot, &mut v)
                        .or_else(|| super::harvest::target(&v)),
                    Profession::Shepherd => super::shepherd::work(state, slot, &mut v, changes),
                    _ => None,
                }
            };
            let mut short_wander = false;
            let mut exact_target = false;
            let target = if shelter_night {
                let (target, exact) = night_shelter(state, &mut cache, slot, &mut v, pos);
                exact_target = exact;
                target
            } else if v.activity == Activity::Panic {
                let retreat = home.or(job_site).or(meeting_point);
                // A flee route that found no step (a shaft or tunnel whose
                // "away" side is rock) would freeze the villager beside the
                // threat for as long as it lasts. Retreat to a known place and
                // keep retreating while that route is progressing, instead of
                // alternating with the unreachable flee target.
                let walled_off = v.navigation_path.is_empty()
                    && v.navigation_target
                        .is_some_and(|goal| distance(pos, centre(goal)) > 16.0);
                let retreating = v.navigation_target == retreat && !v.navigation_path.is_empty();
                if retreat.is_some() && (retreating || walled_off) {
                    retreat
                } else if state.tick >= v.navigation_repath_at || v.navigation_target.is_none() {
                    let source = threat.or_else(|| {
                        state
                            .entities
                            .row(slot)
                            .last_player_hit
                            .and_then(|(id, _)| state.entities.slot_of(id))
                            .map(|s| state.entities.pos(s))
                    });
                    let direction = source
                        .map(|p| [pos[0] - p[0], pos[2] - p[2]])
                        .unwrap_or([1.0, 0.0]);
                    let length = mth::length2(direction[0], direction[1]).max(1e-6);
                    Some(BlockPos {
                        x: (pos[0] + direction[0] / length * 12.0).floor() as i32,
                        y: pos[1].ceil() as i32,
                        z: (pos[2] + direction[1] / length * 12.0).floor() as i32,
                    })
                } else {
                    v.navigation_target
                }
            } else if v.data.profession == Profession::Soldier {
                exact_target = worker_exact;
                worker_target
            } else if let Some(target) = crate::settlements::economy::target(
                state,
                v.settlement,
                state.entities.row(slot).uuid,
            ) {
                exact_target = true;
                Some(target)
            } else if let Some(partner) = v.partner.and_then(|id| state.entities.slot_of(id)) {
                Some(at(state.entities.pos(partner)))
            } else if awaiting_profession {
                job_site
            } else if worker_target.is_some()
                || (v.activity == Activity::Work
                    && matches!(
                        v.data.profession,
                        Profession::Miner | Profession::Lumberjack | Profession::Builder
                    ))
            {
                // A one-cell arrival tolerance can leave a builder inside the
                // future footprint despite its selected perimeter goal.
                exact_target = worker_exact;
                worker_target
            } else {
                match v.activity {
                    Activity::Work
                        if job_site.is_some_and(|job| {
                            distance(pos, centre(job)) > JOB_SITE_REACH_SQUARED
                        }) =>
                    {
                        job_site
                    }
                    Activity::Meet => meeting_point,
                    Activity::Rest => home,
                    Activity::Work | Activity::Idle | Activity::Play => {
                        short_wander = true;
                        if state.tick >= v.navigation_repath_at || v.navigation_target.is_none() {
                            let mut seed =
                                (state.tick / 40) ^ (id.0 as u64).wrapping_mul(0x9E3779B97F4A7C15);
                            // Vary both axes each attempt, including before the
                            // low tick counter reaches the upper 16 bits.
                            seed ^= seed >> 30;
                            seed = seed.wrapping_mul(0xBF58476D1CE4E5B9);
                            seed ^= seed >> 27;
                            // Strolls remain bounded from the current cell,
                            // including Work already within workstation reach.
                            let origin = at(pos);
                            let range = if v.activity == Activity::Work { 5 } else { 13 };
                            let dx = (seed % range) as i32 - range as i32 / 2;
                            let remaining = 6 - dx.abs();
                            let dz = (((seed >> 16) % range) as i32 - range as i32 / 2)
                                .clamp(-remaining, remaining);
                            Some(BlockPos {
                                x: origin.x + dx,
                                y: origin.y,
                                z: origin.z + dz,
                            })
                        } else {
                            v.navigation_target
                        }
                    }
                    Activity::Panic => None,
                }
            };
            if !merchant_busy
                && v.activity == Activity::Work
                && job_site.is_some_and(|p| {
                    distance(state.entities.pos(slot), centre(p)) <= JOB_SITE_REACH_SQUARED
                })
            {
                decision_phase!(
                    cache,
                    6,
                    super::trades::work(&mut v, &state.registry, state.tick, state.day_time)
                );
            }
            let scheduled = cache.activity_for(v.age, v.data.profession, state.day_time);
            decision_phase!(
                cache,
                5,
                if short_wander {
                    wander_to(
                        state,
                        slot,
                        &mut v,
                        if merchant_busy { None } else { target },
                        scheduled,
                    )
                } else {
                    move_to(
                        state,
                        &mut cache,
                        slot,
                        &mut v,
                        if merchant_busy { None } else { target },
                        exact_target,
                    )
                }
            );
        }
        super::doors::interact(state, slot, &mut v, changes);
        let appearance_changed =
            was_sleeping != v.sleeping || baby != (v.age < 0) || old_data != v.data;
        state.entities.row_mut(slot).villager = Some(v);
        if appearance_changed {
            super::sync_appearance(state, slot);
        }
        #[cfg(test)]
        {
            let elapsed = full_start.elapsed().as_nanos();
            full_ns += elapsed;
            if cache.measure_decisions {
                cache.decision_ns.push(elapsed);
                for (samples, elapsed) in
                    cache.decision_phases.iter_mut().zip(cache.decision_current)
                {
                    samples.push(elapsed);
                }
            }
        }
    }
    #[cfg(test)]
    let evaluated = brain_start.elapsed().as_micros();
    interact_gossip(state, &cache);
    courtship(state, &mut cache, changes);
    golems(state, &mut cache, changes);
    // Remove by id/descending slot only after every slot-based transaction.
    cache.removed_items.clear();
    for &slot in cache.items.values().flatten() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive && row.item.is_none() {
            cache.removed_items.push(slot);
        }
    }
    cache.removed_items.sort_unstable_by_key(|s| Reverse(s.0));
    for &slot in &cache.removed_items {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if matches!(body.kind, EntityKind::Item { .. }) && !body.alive && row.item.is_none() {
            let id = body.id;
            state.remove_entity(slot);
            changes.entities_removed.push(id);
        }
    }
    cache.motion_revision = Some(state.world.revision_now());
    #[cfg(test)]
    {
        cache.brain_us = brain_start.elapsed().as_micros();
        cache.phase_us = [
            refreshed,
            prepared - refreshed,
            evaluated - prepared,
            cache.brain_us - evaluated,
            acquired - prepared,
            full_ns / 1_000,
            evaluated - acquired - full_ns / 1_000,
        ];
    }
    state.villager_pois = cache;
    state
        .tick_budget
        .finish(state.dimension, crate::tick_budget::Consumer::Brain);
}

#[cfg(test)]
#[path = "brain_tests.rs"]
mod tests;
