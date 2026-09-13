//! Dust power relaxation, component power sensing and the bounded settle pass.

use std::collections::{HashMap, HashSet, VecDeque};

use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId};

use super::super::super::{
    BlockEdit, BlockPlanningRead, block_state_property, sibling_state_with_bool_property,
};
use super::blocks::{self, HORIZONTAL, MAX_DUST_POWER, NEIGHBOURS, Role, is_air, offset, role};
use super::piston;
use super::{RedstoneBudget, record_piston_refusal, record_settle_pass};

/// Alternating torch/dust relaxation passes per settle. Torches invert their
/// support, so dust and torch states can depend on each other; a bounded number
/// of passes converges for real builds and refuses to spin on a clock loop.
const MAX_SETTLE_PASSES: usize = 8;
use crate::script::ZoneProtectionSnapshot;

fn pos_key(pos: &BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}

/// Read-through planning view: overlay of pending states over the authoritative
/// read source, plus the planned edit list under construction.
pub(super) struct Settle<'a, R: BlockPlanningRead + ?Sized> {
    pub(super) blocks: &'a BlockRegistry,
    read: &'a R,
    protection: Option<&'a ZoneProtectionSnapshot>,
    overlay: HashMap<BlockPos, BlockStateId>,
    planned: HashSet<BlockPos>,
    seeded: HashSet<BlockPos>,
    edits: Vec<BlockEdit>,
    budget: &'a mut RedstoneBudget,
    visited: usize,
}

impl<'a, R: BlockPlanningRead + ?Sized> Settle<'a, R> {
    pub(super) fn new(
        blocks: &'a BlockRegistry,
        read: &'a R,
        protection: Option<&'a ZoneProtectionSnapshot>,
        budget: &'a mut RedstoneBudget,
    ) -> Self {
        Self {
            blocks,
            read,
            protection,
            overlay: HashMap::new(),
            planned: HashSet::new(),
            seeded: HashSet::new(),
            edits: Vec::new(),
            budget,
            visited: 0,
        }
    }

    /// State of `pos`, seeing this pass's own planned edits.
    pub(super) fn block_at(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.overlay
            .get(&pos)
            .copied()
            .or_else(|| self.read.get_cached_block(pos))
    }

    pub(super) fn state_at(&self, pos: BlockPos) -> Option<&'a BlockState> {
        let state_id = self.block_at(pos)?;
        self.blocks.by_id(state_id)
    }

    /// Adopt edits the caller already planned (the toggled source and any
    /// hand-toggle edits) so this pass neither contradicts nor duplicates them.
    pub(super) fn seed_planned(&mut self, edits: &[BlockEdit]) {
        for edit in edits {
            self.overlay.insert(edit.pos, edit.new_state);
            self.planned.insert(edit.pos);
            self.seeded.insert(edit.pos);
        }
    }

    pub(super) fn is_planned(&self, pos: BlockPos) -> bool {
        self.planned.contains(&pos)
    }

    /// Plan a state change, ignoring no-ops, duplicate positions and positions
    /// the zone protection refuses. Returns whether anything changed.
    pub(super) fn plan(&mut self, pos: BlockPos, new_state: BlockStateId) -> bool {
        if self.planned.contains(&pos) || self.block_at(pos) == Some(new_state) {
            return false;
        }
        if let Some(protection) = self.protection
            && !protection.ambient_block_mutation_allowed("minecraft:overworld", pos)
        {
            return false;
        }
        self.overlay.insert(pos, new_state);
        self.planned.insert(pos);
        self.edits.push(BlockEdit { pos, new_state });
        true
    }

    /// Re-plan a state this pass already planned (never a caller-seeded edit).
    fn replan(&mut self, pos: BlockPos, new_state: BlockStateId) -> bool {
        if self.seeded.contains(&pos) {
            return false;
        }
        if self.block_at(pos) == Some(new_state) {
            return false;
        }
        if self.planned.contains(&pos) {
            self.overlay.insert(pos, new_state);
            if let Some(edit) = self.edits.iter_mut().find(|edit| edit.pos == pos) {
                edit.new_state = new_state;
            }
            return true;
        }
        self.plan(pos, new_state)
    }

    pub(super) fn protection_allows(&self, pos: BlockPos) -> bool {
        self.protection.is_none_or(|protection| {
            protection.ambient_block_mutation_allowed("minecraft:overworld", pos)
        })
    }

    pub(super) fn append_to(self, edits: &mut Vec<BlockEdit>) {
        edits.extend(self.edits);
    }

    pub(super) fn into_edits(self) -> Vec<BlockEdit> {
        self.edits
    }

    /// Recompute the neighbourhood affected by `seeds`.
    ///
    /// `forced` positions are recomputed even when they are not a neighbour of a
    /// seed (the scheduled tick's own component).
    pub(super) fn settle(&mut self, seeds: &[BlockPos], forced: &[BlockPos]) {
        let mut dust = self.dust_component(seeds);
        dust.sort_unstable_by_key(pos_key);

        let mut frontier: Vec<BlockPos> = Vec::new();
        let mut seen: HashSet<BlockPos> = HashSet::new();
        for pos in &dust {
            push_neighbours(&mut frontier, &mut seen, *pos);
        }
        for seed in seeds {
            if seen.insert(*seed) {
                frontier.push(*seed);
            }
            push_neighbours(&mut frontier, &mut seen, *seed);
        }
        for pos in forced {
            if seen.insert(*pos) {
                frontier.push(*pos);
            }
        }
        frontier.sort_unstable_by_key(pos_key);

        // Dust and torches feed each other, so relax both to a bounded fixed
        // point before planning the blocks that only consume power.
        for _ in 0..MAX_SETTLE_PASSES {
            let mut changed = false;
            for pos in &frontier {
                changed |= self.settle_torch(*pos);
            }
            changed |= self.settle_dust(&dust);
            for pos in &frontier {
                changed |= self.settle_consumer(*pos);
            }
            if !changed {
                break;
            }
        }
        // Pistons move blocks, so they run once against the settled power.
        for pos in &frontier {
            self.settle_piston(*pos);
        }
        record_settle_pass(self.visited as u64);
    }

    /// Relax every dust state in the component to its settled power and shape.
    fn settle_dust(&mut self, dust: &[BlockPos]) -> bool {
        let mut power: HashMap<BlockPos, u8> = dust.iter().map(|pos| (*pos, 0u8)).collect();
        // Power relaxes to the maximum over every path from a source; each sweep
        // can only raise a value, and 15 sweeps cover the longest possible path.
        for _ in 0..MAX_DUST_POWER {
            let mut changed = false;
            for pos in dust {
                let next = self.dust_input_power(*pos, &power);
                if power[pos] != next {
                    power.insert(*pos, next);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut changed = false;
        for pos in dust {
            let Some(state) = self.state_at(*pos) else {
                continue;
            };
            let mut shape = [""; 4];
            for (index, (_, delta)) in HORIZONTAL.iter().enumerate() {
                shape[index] = self.dust_shape_toward(*pos, *delta);
            }
            if let Some(new_state) = blocks::dust_state(self.blocks, state, power[pos], shape) {
                changed |= self.replan(*pos, new_state);
            }
        }
        changed
    }

    /// Recompute a torch's lit state from the power reaching its support.
    fn settle_torch(&mut self, pos: BlockPos) -> bool {
        let Some(state) = self.state_at(pos) else {
            return false;
        };
        if !matches!(role(state), Role::Torch) {
            return false;
        }
        let Some(support) = self.torch_support(pos, state) else {
            return false;
        };
        let lit = self.received_power(support, Some(pos)) == 0;
        sibling_state_with_bool_property(self.blocks, state, "lit", lit)
            .is_some_and(|new_state| self.replan(pos, new_state))
    }

    /// Recompute a door/trapdoor/fence gate/lamp reaction to received power.
    fn settle_consumer(&mut self, pos: BlockPos) -> bool {
        let Some(state) = self.state_at(pos) else {
            return false;
        };
        if !matches!(role(state), Role::Consumer) {
            return false;
        }
        let powered = self.received_power(pos, None) > 0;
        let mut changed = false;
        if let Some(new_state) = powered_consumer_state(self.blocks, state, powered) {
            changed |= self.replan(pos, new_state);
        }
        // Doors move as a pair; the other half follows the same power.
        let Some(half) = block_state_property(state, "half") else {
            return changed;
        };
        let delta = match half {
            "lower" => (0, 1, 0),
            "upper" => (0, -1, 0),
            _ => return changed,
        };
        let block_id = state.block.id.clone();
        let Some(other_pos) = offset(pos, delta) else {
            return changed;
        };
        let Some(other) = self.state_at(other_pos) else {
            return changed;
        };
        if other.block.id != block_id {
            return changed;
        }
        if let Some(new_state) = powered_consumer_state(self.blocks, other, powered) {
            changed |= self.replan(other_pos, new_state);
        }
        changed
    }

    /// Extend or retract a piston against the settled power.
    fn settle_piston(&mut self, pos: BlockPos) {
        if self.is_planned(pos) {
            return;
        }
        let Some(state) = self.state_at(pos) else {
            return;
        };
        let Role::Piston { sticky } = role(state) else {
            return;
        };
        let powered = self.received_power(pos, None) > 0;
        if let Err(refusal) = piston::plan_move(self, pos, state, sticky, powered) {
            record_piston_refusal();
            tracing::debug!(?refusal, ?pos, "piston move refused");
        }
    }

    /// Support block whose power switches a torch off.
    fn torch_support(&self, pos: BlockPos, state: &BlockState) -> Option<BlockPos> {
        match block_state_property(state, "facing") {
            Some(facing) => {
                let (dx, dy, dz) = blocks::facing_delta(facing)?;
                offset(pos, (-dx, -dy, -dz))
            }
            None => offset(pos, (0, -1, 0)),
        }
    }

    /// Power delivered into `pos` by adjacent emitters and dust. `excluded`
    /// keeps a torch from latching on its own emission.
    pub(super) fn received_power(&self, pos: BlockPos, excluded: Option<BlockPos>) -> u8 {
        let mut best = 0u8;
        for delta in NEIGHBOURS {
            let Some(neighbour) = offset(pos, delta) else {
                continue;
            };
            if Some(neighbour) == excluded {
                continue;
            }
            let Some(state) = self.state_at(neighbour) else {
                continue;
            };
            if let Some(emitted) = blocks::emitted_power(state) {
                best = best.max(emitted);
            } else if blocks::is_dust(state) {
                best = best.max(blocks::dust_power(state));
            }
        }
        best
    }

    /// Dust connected to the seeds, bounded by the pass budget.
    fn dust_component(&mut self, seeds: &[BlockPos]) -> Vec<BlockPos> {
        let mut seen: HashSet<BlockPos> = HashSet::new();
        let mut queue: VecDeque<BlockPos> = VecDeque::new();
        for seed in seeds {
            for delta in std::iter::once((0, 0, 0)).chain(NEIGHBOURS) {
                let Some(candidate) = offset(*seed, delta) else {
                    continue;
                };
                if self.is_dust(candidate) && seen.insert(candidate) {
                    queue.push_back(candidate);
                }
            }
        }
        let mut component = Vec::new();
        while let Some(pos) = queue.pop_front() {
            if !self.budget.take() {
                break;
            }
            self.visited += 1;
            component.push(pos);
            for neighbour in self.dust_neighbours(pos) {
                if seen.insert(neighbour) {
                    queue.push_back(neighbour);
                }
            }
        }
        component
    }

    fn is_dust(&self, pos: BlockPos) -> bool {
        self.state_at(pos).is_some_and(blocks::is_dust)
    }

    /// Dust reachable from `pos`: 6-neighbourhood dust, plus the riser hops over
    /// a solid block with a wire on top (both directions).
    fn dust_neighbours(&self, pos: BlockPos) -> Vec<BlockPos> {
        let mut neighbours = Vec::new();
        for delta in NEIGHBOURS {
            if let Some(neighbour) = offset(pos, delta)
                && self.is_dust(neighbour)
            {
                neighbours.push(neighbour);
            }
        }
        let below = offset(pos, (0, -1, 0));
        for (_, (dx, _, dz)) in HORIZONTAL.iter().map(|(name, d)| (name, *d)) {
            // Climb: a solid block in this direction with dust on top of it.
            if let Some(riser) = offset(pos, (dx, 0, dz))
                && self.is_riser(riser)
                && let Some(above) = offset(riser, (0, 1, 0))
                && self.is_dust(above)
            {
                neighbours.push(above);
            }
            // Descend: this dust sits on a riser, so lower wires beside the
            // riser are connected.
            if let Some(below) = below
                && self.is_riser(below)
                && let Some(lower) = offset(below, (dx, 0, dz))
                && self.is_dust(lower)
            {
                neighbours.push(lower);
            }
        }
        neighbours
    }

    fn is_riser(&self, pos: BlockPos) -> bool {
        self.state_at(pos)
            .is_some_and(|state| !is_air(state) && role(state) == Role::Other)
    }

    fn dust_shape_toward(&self, pos: BlockPos, delta: (i32, i32, i32)) -> &'static str {
        let Some(neighbour_pos) = offset(pos, delta) else {
            return "none";
        };
        let Some(neighbour) = self.state_at(neighbour_pos) else {
            return "none";
        };
        let has_dust_above =
            offset(neighbour_pos, (0, 1, 0)).is_some_and(|above| self.is_dust(above));
        blocks::dust_shape_toward(neighbour, has_dust_above)
    }

    /// Strongest input into a dust position from emitters, adjacent dust and
    /// riser-connected dust.
    fn dust_input_power(&self, pos: BlockPos, power: &HashMap<BlockPos, u8>) -> u8 {
        let mut best = 0u8;
        for delta in NEIGHBOURS {
            let Some(neighbour) = offset(pos, delta) else {
                continue;
            };
            let Some(state) = self.state_at(neighbour) else {
                continue;
            };
            if let Some(emitted) = blocks::emitted_power(state) {
                best = best.max(emitted);
            } else if let Some(value) = power.get(&neighbour) {
                best = best.max(value.saturating_sub(1));
            }
        }
        let below = offset(pos, (0, -1, 0));
        for (_, (dx, _, dz)) in HORIZONTAL.iter().map(|(name, d)| (name, *d)) {
            if let Some(riser) = offset(pos, (dx, 0, dz))
                && self.is_riser(riser)
                && let Some(above) = offset(riser, (0, 1, 0))
                && let Some(value) = power.get(&above)
            {
                best = best.max(value.saturating_sub(1));
            }
            if let Some(below) = below
                && self.is_riser(below)
                && let Some(lower) = offset(below, (dx, 0, dz))
                && let Some(value) = power.get(&lower)
            {
                best = best.max(value.saturating_sub(1));
            }
        }
        best.min(MAX_DUST_POWER)
    }
}

/// Push the six neighbours of `pos` onto the frontier, deduplicated.
fn push_neighbours(frontier: &mut Vec<BlockPos>, seen: &mut HashSet<BlockPos>, pos: BlockPos) {
    for delta in NEIGHBOURS {
        if let Some(neighbour) = offset(pos, delta)
            && seen.insert(neighbour)
        {
            frontier.push(neighbour);
        }
    }
}

/// Powered/open/lit state matching received power. Returns `None` when the block
/// exposes none of those properties.
fn powered_consumer_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    powered: bool,
) -> Option<BlockStateId> {
    let value = if powered { "true" } else { "false" };
    let mut properties = state.properties.clone();
    let mut switched = false;
    for (key, current) in properties.iter_mut() {
        if key == "powered" || key == "open" || key == "lit" {
            *current = value.to_string();
            switched = true;
        }
    }
    if !switched {
        return None;
    }
    blocks.by_name_and_props(&state.block.id, &properties)
}

/// `powered` state for a lever/button/plate (the interactive source).
pub(super) fn powered_control_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    powered: bool,
) -> Option<BlockStateId> {
    sibling_state_with_bool_property(blocks, state, "powered", powered)
}
