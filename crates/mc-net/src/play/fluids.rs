use std::collections::{HashMap, HashSet, VecDeque};

use mc_data::block_facts::{BlockFactsTable, FluidKind, FluidStateFacts};
use mc_protocol::codec::Identifier;
use mc_world::{
    BlockPos, BlockRegistry, BlockStateId, ChunkPos, SECTION_DIM, ScheduledFluidTick,
    WorldReadSnapshot,
};

use super::{
    AppliedBlockEdit, BlockEdit, BlockPlanningRead, ScheduledFluidTickPlan, SnapshotPlanningWorld,
    air_state_id, fluid_neighbour_positions, named_block_default, push_unique_block_edit,
};

pub(super) const WATER_FLOW_DELAY_TICKS: u64 = 5;
const LAVA_FLOW_DELAY_TICKS: u64 = 30;

/// Blockstate `level` that marks vanilla falling fluid: full strength,
/// full-height rendering, unlimited downward travel, and it spreads
/// sideways at full strength. Not a source — it cannot be scooped and it
/// sustains itself only through the support rules in
/// [`fluid_has_source_path`].
const FALLING_LEVEL: u8 = 8;

/// Flow strength: 0 is full (sources and falling fluid), 1..=7 is the
/// remaining horizontal spread distance of flowing fluid.
fn fluid_flow_level(fluid: FluidStateFacts) -> u8 {
    if fluid.source || fluid.level >= FALLING_LEVEL {
        0
    } else {
        fluid.level
    }
}

pub(super) fn scheduled_fluid_planning_chunks(ticks: &[ScheduledFluidTick]) -> Vec<ChunkPos> {
    let mut positions = HashSet::new();
    for tick in ticks {
        let centre = ChunkPos {
            x: tick.pos.x.div_euclid(SECTION_DIM as i32),
            z: tick.pos.z.div_euclid(SECTION_DIM as i32),
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let (Some(x), Some(z)) = (centre.x.checked_add(dx), centre.z.checked_add(dz))
                else {
                    continue;
                };
                positions.insert(ChunkPos { x, z });
            }
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.z));
    positions
}

pub(super) fn plan_scheduled_fluid_tick_edits(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world_tick: u64,
    snapshot: &WorldReadSnapshot,
    ticks: &[ScheduledFluidTick],
) -> ScheduledFluidTickPlan {
    let mut world = SnapshotPlanningWorld::new(snapshot);
    let mut plan = ScheduledFluidTickPlan::default();
    let mut edit_indexes = HashMap::<BlockPos, usize>::new();
    for tick in ticks {
        let Some(state) = world.get_cached_block(tick.pos) else {
            continue;
        };
        let Some(fluid) = facts.fluid(state.0) else {
            continue;
        };
        if fluid_identifier(fluid.kind) != tick.fluid {
            continue;
        }
        let edits = fluid_tick_edits(blocks, facts, &world, tick.pos, state, fluid);
        if edits
            .iter()
            .any(|edit| world.get_cached_block(edit.pos).is_none())
        {
            continue;
        }
        for edit in edits {
            if world.apply(edit) {
                if let Some(&index) = edit_indexes.get(&edit.pos) {
                    plan.edits[index] = edit;
                } else {
                    edit_indexes.insert(edit.pos, plan.edits.len());
                    plan.edits.push(edit);
                }
            }
        }
    }
    plan.edits
        .retain(|edit| snapshot.get_cached_block(edit.pos) != Some(edit.new_state));
    plan.preconditions = world.preconditions();
    let edited_positions = plan.edits.iter().map(|edit| edit.pos).collect::<Vec<_>>();
    plan.scheduled_fluid_ticks =
        plan_fluid_ticks_near_positions(&world, facts, world_tick, &edited_positions);
    plan
}

pub(super) fn fluid_tick_edits(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    state: BlockStateId,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    let mut edits = fluid_interaction_edits(blocks, facts, world, pos, fluid);
    if !edits.is_empty() {
        return edits;
    }

    if !fluid.source
        && let Some(new_state) = supported_flow_state(blocks, facts, world, pos, fluid)
        && new_state != state
    {
        edits.push(BlockEdit { pos, new_state });
        return edits;
    }

    edits.extend(fluid_spread_edits(blocks, facts, world, pos, fluid));
    edits
}

fn fluid_interaction_edits(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    let mut edits = Vec::new();
    for neighbour in fluid_neighbour_positions(pos) {
        let Some(neighbour_state) = world.get_cached_block(neighbour) else {
            continue;
        };
        let Some(other) = facts.fluid(neighbour_state.0) else {
            continue;
        };
        if other.kind == fluid.kind {
            continue;
        }
        match (fluid.kind, other.kind) {
            (FluidKind::Water, FluidKind::Lava) => {
                if let Some(new_state) = lava_contact_result(blocks, other, pos, neighbour) {
                    push_unique_block_edit(
                        &mut edits,
                        BlockEdit {
                            pos: neighbour,
                            new_state,
                        },
                    );
                }
            }
            (FluidKind::Lava, FluidKind::Water) => {
                if let Some(new_state) = lava_contact_result(blocks, fluid, neighbour, pos) {
                    push_unique_block_edit(&mut edits, BlockEdit { pos, new_state });
                }
            }
            _ => {}
        }
    }
    edits
}

fn lava_contact_result(
    blocks: &BlockRegistry,
    lava: FluidStateFacts,
    water_pos: BlockPos,
    lava_pos: BlockPos,
) -> Option<BlockStateId> {
    if lava.source {
        return named_block_default(blocks, "minecraft:obsidian");
    }
    if water_pos.y > lava_pos.y {
        named_block_default(blocks, "minecraft:stone")
    } else {
        named_block_default(blocks, "minecraft:cobblestone")
    }
}

pub(super) fn supported_flow_state(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    fluid: FluidStateFacts,
) -> Option<BlockStateId> {
    let above = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    if world
        .get_cached_block(above)
        .and_then(|state| facts.fluid(state.0))
        .is_some_and(|above| above.kind == fluid.kind)
    {
        // Vanilla: the same fluid above makes this cell falling water —
        // full strength, supported unconditionally, and it spreads
        // sideways at full strength. This is exactly what downward spread
        // installs, so re-ticking never corrects it back and forth.
        return fluid_state_with_level(blocks, fluid.kind, FALLING_LEVEL);
    }

    let next_level = horizontal_fluid_neighbours(pos)
        .into_iter()
        .filter_map(|neighbour| {
            let state = world.get_cached_block(neighbour)?;
            let other = facts.fluid(state.0)?;
            (other.kind == fluid.kind && fluid_has_source_path(facts, world, neighbour, other, 0))
                .then_some(fluid_flow_level(other))
        })
        .map(|level| level.saturating_add(1))
        .min();

    match next_level {
        Some(level) if level <= max_flow_level(fluid.kind) => {
            fluid_state_with_level(blocks, fluid.kind, level)
        }
        _ => Some(air_state_id(blocks)),
    }
}

/// Vanilla water support: a flowing cell is sustained by a source reached
/// through a strictly-weakening horizontal chain, or by the same fluid
/// above (falling water, which vanilla supports unconditionally). Vertical
/// support is unbounded — only horizontal steps consume the spread-distance
/// budget — so deep columns and pools keep their support path instead of
/// dying to air and being refilled a few ticks later.
fn fluid_has_source_path(
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    fluid: FluidStateFacts,
    depth: u8,
) -> bool {
    let max_depth = max_flow_level(fluid.kind).saturating_add(1);
    let mut pending = VecDeque::from([(pos, fluid, depth)]);
    let mut visited = HashSet::new();
    while let Some((pos, fluid, depth)) = pending.pop_front() {
        if fluid.source || fluid.level >= FALLING_LEVEL {
            return true;
        }
        let above = BlockPos {
            y: pos.y + 1,
            ..pos
        };
        if world
            .get_cached_block(above)
            .and_then(|state| facts.fluid(state.0))
            .is_some_and(|above_fluid| above_fluid.kind == fluid.kind)
        {
            return true;
        }
        if depth > max_depth || !visited.insert(pos) {
            continue;
        }

        let next_depth = depth.saturating_add(1);
        let current_level = fluid_flow_level(fluid);
        pending.extend(
            horizontal_fluid_neighbours(pos)
                .into_iter()
                .filter_map(|neighbour| {
                    let other = world
                        .get_cached_block(neighbour)
                        .and_then(|state| facts.fluid(state.0))?;
                    (other.kind == fluid.kind && fluid_flow_level(other) < current_level)
                        .then_some((neighbour, other, next_depth))
                }),
        );
    }
    false
}

fn fluid_spread_edits(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    fluid: FluidStateFacts,
) -> Vec<BlockEdit> {
    // Vanilla: fluid flowing down always lands at full strength — the
    // falling state, which falls indefinitely and keeps spreading sideways
    // at full strength ("spreads 7 at level 8"). Horizontal flow weakens
    // by one level per step and stops past the max flow level. A falling
    // column whose cell below is already falling just continues, so
    // mid-air columns never grow side sheets; flowing fluid below blocks
    // the fall and turns it into sideways spread instead.
    let below = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let flow_level = fluid_flow_level(fluid);
    let mut targets: Vec<(BlockPos, BlockStateId)> = Vec::new();
    let falls_down = fluid_state_with_level(blocks, fluid.kind, FALLING_LEVEL)
        .filter(|_| can_flow_below(blocks, facts, world, below, fluid.kind));
    match falls_down {
        Some(falling_state) => {
            if world.get_cached_block(below) != Some(falling_state) {
                targets.push((below, falling_state));
            }
        }
        None => {
            let side_level = flow_level.saturating_add(1);
            if side_level <= max_flow_level(fluid.kind)
                && let Some(side_state) = fluid_state_with_level(blocks, fluid.kind, side_level)
            {
                for target in horizontal_fluid_neighbours(pos) {
                    if can_flow_into(blocks, facts, world, target, fluid.kind, side_level) {
                        targets.push((target, side_state));
                    }
                }
            }
        }
    }
    let mut edits: Vec<BlockEdit> = targets
        .iter()
        .map(|&(target, new_state)| BlockEdit {
            pos: target,
            new_state,
        })
        .collect();
    // A washed plant stops supporting whatever stood on it (upper double
    // halves, column segments): pop those with air, same rule as the
    // break-path cascade. Drops resolve at commit from the previous states.
    if fluid.kind == FluidKind::Water {
        let air = air_state_id(blocks);
        for &(target, _) in &targets {
            if world
                .get_cached_block(target)
                .is_some_and(|state| is_water_washable_plant(blocks, state))
            {
                super::block_break::append_vertical_support_cascade(
                    blocks, world, &mut edits, target, air,
                );
            }
        }
    }
    edits
}

/// Plants flowing water displaces in vanilla, breaking them with drops:
/// the shared ground-support set plus the shared column set. The single
/// water-specific rule is seagrass, which lives submerged, so water coexists
/// with it. Lava keeps the old stop-at-plant behavior (queued separately,
/// not a desync).
pub(super) fn is_water_washable_plant(blocks: &BlockRegistry, state: BlockStateId) -> bool {
    let Some(resolved) = blocks.by_id(state) else {
        return false;
    };
    let path = resolved.block.id.path();
    if matches!(path, "seagrass" | "tall_seagrass") {
        return false;
    }
    mc_world::plant_rules_26_1_2::is_ground_support_plant(path)
        || super::block_break::is_vertical_support_cascade_block(path)
}

fn can_flow_into(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    kind: FluidKind,
    new_flow_level: u8,
) -> bool {
    let Some(state) = world.get_cached_block(pos) else {
        return false;
    };
    if state == air_state_id(blocks) {
        return true;
    }
    if kind == FluidKind::Water && is_water_washable_plant(blocks, state) {
        return true;
    }
    facts.fluid(state.0).is_some_and(|fluid| {
        fluid.kind == kind && !fluid.source && fluid_flow_level(fluid) > new_flow_level
    })
}

/// Vanilla down-flow: fluid falls into air (and plants it washes away), and
/// a falling column continues through falling fluid beneath it. Flowing
/// fluid below blocks the fall — the fluid spreads sideways there instead —
/// so sheets never punch falling columns into pools.
fn can_flow_below(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    kind: FluidKind,
) -> bool {
    let Some(state) = world.get_cached_block(pos) else {
        return false;
    };
    if state == air_state_id(blocks) {
        return true;
    }
    if kind == FluidKind::Water && is_water_washable_plant(blocks, state) {
        return true;
    }
    facts
        .fluid(state.0)
        .is_some_and(|fluid| fluid.kind == kind && fluid.level >= FALLING_LEVEL)
}

pub(super) fn plan_fluid_ticks_near_applied(
    world: &impl BlockPlanningRead,
    facts: &BlockFactsTable,
    world_tick: u64,
    applied: &[AppliedBlockEdit],
) -> Vec<ScheduledFluidTick> {
    let positions = applied.iter().map(|edit| edit.pos).collect::<Vec<_>>();
    plan_fluid_ticks_near_positions(world, facts, world_tick, &positions)
}

fn plan_fluid_ticks_near_positions(
    world: &impl BlockPlanningRead,
    facts: &BlockFactsTable,
    world_tick: u64,
    edited_positions: &[BlockPos],
) -> Vec<ScheduledFluidTick> {
    let mut positions = HashSet::new();
    for &edited in edited_positions {
        positions.insert(edited);
        for pos in fluid_neighbour_positions(edited) {
            positions.insert(pos);
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    positions
        .into_iter()
        .filter_map(|pos| {
            let state = world.get_cached_block(pos)?;
            let fluid = facts.fluid(state.0)?;
            Some(ScheduledFluidTick::new(
                pos,
                fluid_identifier(fluid.kind),
                world_tick.wrapping_add(fluid_tick_delay(fluid.kind)),
                0,
            ))
        })
        .collect()
}

fn fluid_tick_delay(kind: FluidKind) -> u64 {
    match kind {
        FluidKind::Water => WATER_FLOW_DELAY_TICKS,
        FluidKind::Lava => LAVA_FLOW_DELAY_TICKS,
    }
}

fn max_flow_level(kind: FluidKind) -> u8 {
    match kind {
        FluidKind::Water => 7,
        FluidKind::Lava => 3,
    }
}

fn horizontal_fluid_neighbours(pos: BlockPos) -> [BlockPos; 4] {
    [
        BlockPos {
            x: pos.x + 1,
            ..pos
        },
        BlockPos {
            x: pos.x - 1,
            ..pos
        },
        BlockPos {
            z: pos.z + 1,
            ..pos
        },
        BlockPos {
            z: pos.z - 1,
            ..pos
        },
    ]
}

fn fluid_identifier(kind: FluidKind) -> Identifier {
    Identifier::parse(match kind {
        FluidKind::Water => "minecraft:water",
        FluidKind::Lava => "minecraft:lava",
    })
    .expect("static identifier")
}

pub(super) fn fluid_state_with_level(
    blocks: &BlockRegistry,
    kind: FluidKind,
    level: u8,
) -> Option<BlockStateId> {
    blocks.by_name_and_props(
        &fluid_identifier(kind),
        &[("level".to_string(), level.to_string())],
    )
}

/// Drops for plants a fluid tick washed away, resolved from the replaced
/// previous states. Upper double halves yield nothing (the lower half owns
/// the loot), matching survival semantics. Called once per committed fluid
/// outcome, so every listed edit is fluid-driven by construction.
pub(super) fn fluid_wash_drops(
    blocks: &BlockRegistry,
    loot: &mc_data::loot::LootTables,
    items: &mc_data::items::ItemRegistry,
    item_facts: &mc_data::item_components::ItemFactsTable,
    applied: &[AppliedBlockEdit],
    loot_seed: u64,
) -> Vec<(BlockPos, mc_entity::EntityItemStack)> {
    let mut drops = Vec::new();
    for edit in applied {
        if !is_water_washable_plant(blocks, edit.previous) {
            continue;
        }
        let Some(resolved) = blocks.by_id(edit.previous) else {
            continue;
        };
        if super::block_state_property(resolved, "half") == Some("upper") {
            continue;
        }
        let seed = loot_seed
            .wrapping_add(edit.pos.x as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((edit.pos.y as u64) << 32 | (edit.pos.z as u64 & 0xffff_ffff));
        for stack in super::survival::block_drop_stacks_with_tool_and_facts_from_seeded(
            loot,
            items,
            item_facts,
            blocks,
            edit.previous,
            None,
            seed,
        ) {
            drops.push((
                edit.pos,
                mc_entity::EntityItemStack::new(stack.item_id, stack.count),
            ));
        }
    }
    drops
}
