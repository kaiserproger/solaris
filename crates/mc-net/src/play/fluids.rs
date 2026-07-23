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
        return fluid_state_with_level(blocks, fluid.kind, 1);
    }

    let next_level = horizontal_fluid_neighbours(pos)
        .into_iter()
        .filter_map(|neighbour| {
            let state = world.get_cached_block(neighbour)?;
            let other = facts.fluid(state.0)?;
            (other.kind == fluid.kind && fluid_has_source_path(facts, world, neighbour, other, 0))
                .then_some(other)
        })
        .map(|other| other.level.saturating_add(1))
        .min();

    match next_level {
        Some(level) if level <= max_flow_level(fluid.kind) => {
            fluid_state_with_level(blocks, fluid.kind, level)
        }
        _ => Some(air_state_id(blocks)),
    }
}

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
        if fluid.source {
            return true;
        }
        if depth > max_depth || !visited.insert(pos) {
            continue;
        }

        let next_depth = depth.saturating_add(1);
        let above = BlockPos {
            y: pos.y + 1,
            ..pos
        };
        if let Some(above_fluid) = world
            .get_cached_block(above)
            .and_then(|state| facts.fluid(state.0))
            .filter(|above_fluid| above_fluid.kind == fluid.kind)
        {
            pending.push_back((above, above_fluid, next_depth));
        }

        pending.extend(
            horizontal_fluid_neighbours(pos)
                .into_iter()
                .filter_map(|neighbour| {
                    let other = world
                        .get_cached_block(neighbour)
                        .and_then(|state| facts.fluid(state.0))?;
                    (other.kind == fluid.kind && other.level < fluid.level)
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
    let next_level = if fluid.source { 1 } else { fluid.level + 1 };
    if next_level > max_flow_level(fluid.kind) {
        return Vec::new();
    }
    let Some(next_state) = fluid_state_with_level(blocks, fluid.kind, next_level) else {
        return Vec::new();
    };
    let below = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    if can_flow_into(blocks, facts, world, below, fluid.kind, next_level) {
        return vec![BlockEdit {
            pos: below,
            new_state: next_state,
        }];
    }

    horizontal_fluid_neighbours(pos)
        .into_iter()
        .filter(|&target| can_flow_into(blocks, facts, world, target, fluid.kind, next_level))
        .map(|target| BlockEdit {
            pos: target,
            new_state: next_state,
        })
        .collect()
}

fn can_flow_into(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    kind: FluidKind,
    new_level: u8,
) -> bool {
    let Some(state) = world.get_cached_block(pos) else {
        return false;
    };
    if state == air_state_id(blocks) {
        return true;
    }
    facts
        .fluid(state.0)
        .is_some_and(|fluid| fluid.kind == kind && !fluid.source && fluid.level > new_level)
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
