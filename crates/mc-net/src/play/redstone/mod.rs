//! Redstone power model, dust propagation and piston moves.
//!
//! # Model
//!
//! Power lives in the world block states, not in a side table: dust carries
//! `minecraft:redstone_wire` `power` 0..=15 plus its `north/east/south/west`
//! connection shape, torches carry `lit`, pistons carry `extended`, doors and
//! lamps carry `powered`/`open`/`lit`. Every planned change is therefore an
//! ordinary [`BlockEdit`] committed through the existing authoritative block
//! edit path, which keeps persistence, preconditions, lighting and the
//! `ClientboundBlockUpdate`/`SectionBlocksUpdate` broadcast unchanged.
//!
//! Propagation is event driven. A committed edit batch enqueues one scheduled
//! block tick (the existing per-chunk queue) for every reactive redstone
//! component in the edited position's 6-neighbourhood. When such a tick fires,
//! [`Settle`] recomputes only the affected neighbourhood: the connected dust
//! component, the components adjacent to it, and the power-sensitive blocks
//! adjacent to the changed positions. Nothing scans the world per tick.
//!
//! # Fence and fallback
//!
//! A pathological build ("lag machine") must not monopolise a tick, so the work
//! is capped at three levels, all reported by [`metrics`]:
//!
//! - [`MAX_POSITIONS_PER_SETTLE`] (256) bounds the dust positions one settle
//!   pass may expand; the reactive frontier is derived from them (at most six
//!   neighbours each), so total work per pass stays bounded. Torch/dust
//!   relaxation replans that same bounded set at most eight times.
//! - [`MAX_POSITIONS_PER_TICK`] (1024) bounds the positions all settle passes of
//!   one scheduled-block-tick batch may visit, shared across the batch. The
//!   single-tick fallback entry point uses one [`MAX_POSITIONS_PER_SETTLE`] pass
//!   per tick instead, which is tighter still.
//! - [`MAX_TICKS_PER_COMMIT`] (64) bounds the scheduled redstone ticks enqueued
//!   by a single applied edit batch.
//!
//! Fallback: work beyond a cap is *dropped*, not deferred. A dust network larger
//! than the fence stays stale until another event touches it, and a burst of
//! edits beyond the commit cap leaves the extra components asleep until their
//! next neighbouring edit. Both drops increment `budget_drops`/`ticks_dropped`,
//! so the fence is observable instead of silent. This is the deliberate
//! anti-lag trade: bounded tick time in exchange for eventual (not immediate)
//! convergence on absurd builds.
//!
//! The scheduled-tick queue itself is already budgeted
//! (`work_budgets.scheduled_ticks`); the caps above bound the work *inside* each
//! drained tick, so one pathological component cannot fan out without limit.
//!
//! # Deliberate deviations from vanilla
//!
//! - No `ClientboundBlockEvent` piston animation packet is emitted; extend and
//!   retract are visible as block updates of the piston base and head (the head
//!   is the vanilla `piston_head` state, so clients render the extended piston).
//! - Power is not transmitted through solid blocks (no "strongly powered
//!   block" relaying). Sources must touch the dust or component they drive.
//! - An edit that merely lands next to a component (placement, break) is
//!   followed by one scheduled tick before the component reacts, instead of
//!   reacting in the same tick. Interactive toggles are the exception: the
//!   lever/button plan settles its whole network in the same transaction, the
//!   way the pre-existing one-hop fanout did.
//! - Dust connection shapes are recomputed from a documented approximation:
//!   dust connects to every adjacent dust, to every adjacent component that
//!   accepts power, and climbs over any non-air, non-component block that has
//!   dust on top of it. The exact vanilla `isRedstoneConductor` test is not
//!   reproduced.
//! - Pistons refuse to move every block with a block entity (vanilla agrees:
//!   `PistonBaseBlock.isPushable` rejects block entities), and additionally
//!   refuse redstone components and piston bases/heads that vanilla would
//!   destroy or push. Refusing is what keeps the move transaction from
//!   duplicating or destroying items; carrying block entity contents through a
//!   move is deferred.
//! - Repeaters, comparators, observers, rails, note blocks, hoppers and entity
//!   driven pressure plates keep their existing (non-redstone) behaviour.

mod blocks;
mod piston;
mod power;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use mc_world::{BlockPos, BlockRegistry, BlockStateId, ScheduledBlockTick, WorldStorage};

use super::super::{AppliedBlockEdit, BlockEdit, BlockPlanningRead};
use crate::script::ZoneProtectionSnapshot;

use blocks::{Role, role};

/// Positions one settle pass may visit before dropping the rest.
pub(in crate::play) const MAX_POSITIONS_PER_SETTLE: usize = 256;
/// Positions every settle pass of one scheduled-tick batch may visit in total.
pub(in crate::play) const MAX_POSITIONS_PER_TICK: usize = 1_024;
/// Scheduled redstone ticks one applied edit batch may enqueue.
pub(in crate::play) const MAX_TICKS_PER_COMMIT: usize = 64;

static SETTLE_PASSES: AtomicU64 = AtomicU64::new(0);
static POSITIONS_VISITED: AtomicU64 = AtomicU64::new(0);
static BUDGET_DROPS: AtomicU64 = AtomicU64::new(0);
static PISTON_REFUSALS: AtomicU64 = AtomicU64::new(0);
static TICKS_ENQUEUED: AtomicU64 = AtomicU64::new(0);
static TICKS_DROPPED: AtomicU64 = AtomicU64::new(0);

/// Observable fence counters and the caps they enforce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RedstoneMetrics {
    pub settle_passes: u64,
    pub positions_visited: u64,
    pub budget_drops: u64,
    pub piston_refusals: u64,
    pub ticks_enqueued: u64,
    pub ticks_dropped: u64,
    pub max_positions_per_settle: usize,
    pub max_positions_per_tick: usize,
    pub max_ticks_per_commit: usize,
}

pub(crate) fn metrics() -> RedstoneMetrics {
    RedstoneMetrics {
        settle_passes: SETTLE_PASSES.load(Ordering::Relaxed),
        positions_visited: POSITIONS_VISITED.load(Ordering::Relaxed),
        budget_drops: BUDGET_DROPS.load(Ordering::Relaxed),
        piston_refusals: PISTON_REFUSALS.load(Ordering::Relaxed),
        ticks_enqueued: TICKS_ENQUEUED.load(Ordering::Relaxed),
        ticks_dropped: TICKS_DROPPED.load(Ordering::Relaxed),
        max_positions_per_settle: MAX_POSITIONS_PER_SETTLE,
        max_positions_per_tick: MAX_POSITIONS_PER_TICK,
        max_ticks_per_commit: MAX_TICKS_PER_COMMIT,
    }
}

/// Bounded work allowance shared by every settle pass of one tick batch.
#[derive(Debug)]
pub(in crate::play) struct RedstoneBudget {
    remaining: usize,
    dropped: u64,
}

impl RedstoneBudget {
    pub(in crate::play) fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            dropped: 0,
        }
    }

    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            self.dropped = self.dropped.saturating_add(1);
            BUDGET_DROPS.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                metrics = ?metrics(),
                dropped_in_pass = self.dropped,
                "redstone settle work fence reached; remaining work dropped"
            );
            return false;
        }
        self.remaining -= 1;
        true
    }
}

/// Propagate the power change carried by `edits` (whose first entries include
/// `source`, the toggled lever/button/pressure plate) and append every affected
/// edit. Behaviour-preserving replacement for the old one-hop power fanout.
pub(in crate::play) fn extend_power_change_edits(
    blocks: &BlockRegistry,
    read: &(impl BlockPlanningRead + ?Sized),
    source: BlockPos,
    powered: bool,
    protection: Option<&ZoneProtectionSnapshot>,
    edits: &mut Vec<BlockEdit>,
) {
    let mut budget = RedstoneBudget::new(MAX_POSITIONS_PER_SETTLE);
    let planned = edits.clone();
    let mut settle = Settle::new(blocks, read, protection, &mut budget);
    settle.seed_planned(&planned);
    if !settle.is_planned(source)
        && let Some(state_id) = settle.block_at(source)
        && let Some(state) = blocks.by_id(state_id)
        && let Some(new_state) = power::powered_control_state(blocks, state, powered)
    {
        settle.plan(source, new_state);
    }
    settle.settle(&[source], &[]);
    settle.append_to(edits);
}

/// Recompute a reactive redstone component whose scheduled tick fired.
///
/// Returns `None` when the block is not reactive (the caller keeps its existing
/// scheduled-tick behaviour), or the planned edits otherwise.
pub(in crate::play) fn redstone_tick_edits(
    blocks: &BlockRegistry,
    read: &(impl BlockPlanningRead + ?Sized),
    pos: BlockPos,
    state_id: BlockStateId,
    protection: Option<&ZoneProtectionSnapshot>,
    budget: &mut RedstoneBudget,
) -> Option<Vec<BlockEdit>> {
    let state = blocks.by_id(state_id)?;
    if !is_reactive(role(state)) {
        return None;
    }
    let mut settle = Settle::new(blocks, read, protection, budget);
    settle.settle(&[pos], &[pos]);
    Some(settle.into_edits())
}

/// Enqueue one scheduled redstone tick for every reactive component next to an
/// applied edit so the next tick recomputes that neighbourhood.
///
/// Returns the number of ticks enqueued. Positions in chunks that are not
/// resident are skipped so planning never materialises a chunk.
pub(in crate::play) fn schedule_redstone_ticks_near_applied(
    storage: &mut WorldStorage,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) -> usize {
    let blocks = storage.registry_arc();
    let mut candidates = HashSet::new();
    for edit in applied {
        candidates.insert(edit.pos);
        for neighbour in super::super::adjacent_block_positions(edit.pos) {
            candidates.insert(neighbour);
        }
    }
    if candidates.is_empty() {
        return 0;
    }
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));

    let trigger_tick = world_tick.saturating_add(1);
    let mut enqueued = 0;
    for pos in candidates {
        if enqueued >= MAX_TICKS_PER_COMMIT {
            TICKS_DROPPED.fetch_add(1, Ordering::Relaxed);
            break;
        }
        let Some(state_id) = storage.get_cached_block(pos) else {
            continue;
        };
        let Some(state) = blocks.by_id(state_id) else {
            continue;
        };
        if !is_reactive(role(state)) {
            continue;
        }
        let chunk = mc_world::ChunkPos {
            x: pos.x.div_euclid(mc_world::SECTION_DIM as i32),
            z: pos.z.div_euclid(mc_world::SECTION_DIM as i32),
        };
        if storage.cached_chunk_snapshot(chunk).is_none() {
            continue;
        }
        let tick = ScheduledBlockTick::new(pos, state.block.id.clone(), trigger_tick, 0);
        if storage.schedule_block_tick(tick).unwrap_or(false) {
            enqueued += 1;
        }
    }
    TICKS_ENQUEUED.fetch_add(enqueued as u64, Ordering::Relaxed);
    enqueued
}

fn is_reactive(role: Role) -> bool {
    matches!(
        role,
        Role::Dust | Role::Torch | Role::Piston { .. } | Role::Consumer
    )
}

/// Record one bounded settle pass.
fn record_settle_pass(visited: u64) {
    SETTLE_PASSES.fetch_add(1, Ordering::Relaxed);
    POSITIONS_VISITED.fetch_add(visited, Ordering::Relaxed);
}

/// Record one refused piston move.
fn record_piston_refusal() {
    PISTON_REFUSALS.fetch_add(1, Ordering::Relaxed);
}

use power::Settle;
