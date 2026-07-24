use std::collections::HashSet;

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId, ScheduledBlockTick};

use super::{
    BlockEdit, BlockEditPrecondition, BlockPlanningRead, adjacent_block_positions, air_state_id,
    block_state_property, sibling_state_with_bool_property,
};
use crate::script::ZoneProtectionSnapshot;

const BUTTON_RELEASE_DELAY_TICKS: u64 = 20;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ToggleBlockPlan {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) preconditions: Vec<BlockEditPrecondition>,
    pub(super) scheduled_block_ticks: Vec<ScheduledBlockTick>,
}

#[cfg(test)]
pub(super) fn plan_toggle_block_interaction(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state_id: BlockStateId,
    world_tick: u64,
) -> Option<ToggleBlockPlan> {
    plan_toggle_block_interaction_with_protection(blocks, storage, pos, state_id, world_tick, None)
}

pub(super) fn plan_toggle_block_interaction_with_protection(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state_id: BlockStateId,
    world_tick: u64,
    protection: Option<&ZoneProtectionSnapshot>,
) -> Option<ToggleBlockPlan> {
    let state = blocks.by_id(state_id)?;
    let path = state.block.id.path();
    if is_hand_openable_door(path) && block_state_property(state, "half").is_some() {
        let edits = plan_door_toggle_edits(blocks, storage, pos, state)?;
        return toggle_block_plan(storage, edits, Vec::new());
    }
    if is_hand_openable_single_block(path)
        && let Some(open) = toggled_bool_state(blocks, state, "open")
    {
        return toggle_block_plan(
            storage,
            vec![BlockEdit {
                pos,
                new_state: open,
            }],
            Vec::new(),
        );
    }
    if path.ends_with("_button") {
        return plan_power_control_interaction(
            blocks, storage, pos, state, true, world_tick, protection,
        );
    }
    if path == "lever" {
        return plan_power_control_interaction(
            blocks, storage, pos, state, false, world_tick, protection,
        );
    }
    None
}

fn plan_power_control_interaction(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state: &BlockState,
    momentary: bool,
    world_tick: u64,
    protection: Option<&ZoneProtectionSnapshot>,
) -> Option<ToggleBlockPlan> {
    let currently_powered = block_state_property(state, "powered")? == "true";
    if momentary && currently_powered {
        return Some(ToggleBlockPlan::default());
    }
    let next_powered = if momentary { true } else { !currently_powered };
    let mut edits = vec![BlockEdit {
        pos,
        new_state: sibling_state_with_bool_property(blocks, state, "powered", next_powered)?,
    }];
    extend_adjacent_power_target_edits(blocks, storage, pos, next_powered, protection, &mut edits);
    let scheduled_block_ticks = if momentary {
        let trigger_tick = world_tick.saturating_add(BUTTON_RELEASE_DELAY_TICKS);
        vec![ScheduledBlockTick::new(
            pos,
            state.block.id.clone(),
            trigger_tick,
            0,
        )]
    } else {
        Vec::new()
    };
    let decision_reads = if next_powered {
        Vec::new()
    } else {
        adjacent_power_decision_positions(blocks, storage, pos)
    };
    toggle_block_plan_with_reads(storage, edits, scheduled_block_ticks, decision_reads)
}

fn toggle_block_plan(
    storage: &dyn BlockPlanningRead,
    edits: Vec<BlockEdit>,
    scheduled_block_ticks: Vec<ScheduledBlockTick>,
) -> Option<ToggleBlockPlan> {
    toggle_block_plan_with_reads(storage, edits, scheduled_block_ticks, Vec::new())
}

fn toggle_block_plan_with_reads(
    storage: &dyn BlockPlanningRead,
    edits: Vec<BlockEdit>,
    scheduled_block_ticks: Vec<ScheduledBlockTick>,
    decision_reads: Vec<BlockPos>,
) -> Option<ToggleBlockPlan> {
    let mut seen = HashSet::with_capacity(edits.len());
    let mut preconditions = Vec::with_capacity(edits.len() + decision_reads.len());
    for pos in edits.iter().map(|edit| edit.pos) {
        if !seen.insert(pos) {
            continue;
        }
        preconditions.push(BlockEditPrecondition {
            pos,
            expected_state: storage.get_cached_block(pos)?,
            expected_token: storage.block_mutation_token(pos)?,
        });
    }
    for pos in decision_reads {
        if !seen.insert(pos) {
            continue;
        }
        let Some(expected_state) = storage.get_cached_block(pos) else {
            continue;
        };
        let Some(expected_token) = storage.block_mutation_token(pos) else {
            continue;
        };
        preconditions.push(BlockEditPrecondition {
            pos,
            expected_state,
            expected_token,
        });
    }
    Some(ToggleBlockPlan {
        edits,
        preconditions,
        scheduled_block_ticks,
    })
}

fn adjacent_power_decision_positions(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    source: BlockPos,
) -> Vec<BlockPos> {
    let mut positions = HashSet::new();
    for target in adjacent_block_positions(source) {
        let Some(state_id) = storage.get_cached_block(target) else {
            continue;
        };
        let Some(state) = blocks.by_id(state_id) else {
            continue;
        };
        let path = state.block.id.path();
        if path.ends_with("_trapdoor") || path == "piston" {
            positions.extend(
                adjacent_block_positions(target)
                    .into_iter()
                    .filter(|pos| *pos != source),
            );
        } else if path.ends_with("_door")
            && let Some(other_y) = match block_state_property(state, "half") {
                Some("lower") => target.y.checked_add(1),
                Some("upper") => target.y.checked_sub(1),
                _ => None,
            }
        {
            positions.extend(
                adjacent_block_positions(target)
                    .into_iter()
                    .chain(adjacent_block_positions(BlockPos {
                        y: other_y,
                        ..target
                    }))
                    .filter(|pos| *pos != source),
            );
        }
    }
    let mut positions = positions.into_iter().collect::<Vec<_>>();
    positions.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
    positions
}

fn is_hand_openable_door(path: &str) -> bool {
    path.ends_with("_door") && path != "iron_door"
}

fn is_hand_openable_single_block(path: &str) -> bool {
    (path.ends_with("_trapdoor") && path != "iron_trapdoor") || path.ends_with("_fence_gate")
}

pub(super) fn extend_adjacent_power_target_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    source: BlockPos,
    powered: bool,
    protection: Option<&ZoneProtectionSnapshot>,
    edits: &mut Vec<BlockEdit>,
) {
    let mut edited = edits.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
    for pos in adjacent_block_positions(source) {
        extend_power_target_edits(
            blocks,
            storage,
            source,
            pos,
            powered,
            protection,
            edits,
            &mut edited,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn extend_power_target_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    source: BlockPos,
    pos: BlockPos,
    powered: bool,
    protection: Option<&ZoneProtectionSnapshot>,
    edits: &mut Vec<BlockEdit>,
    edited: &mut HashSet<BlockPos>,
) {
    let Some(state_id) = storage.get_cached_block(pos) else {
        return;
    };
    let Some(state) = blocks.by_id(state_id) else {
        return;
    };
    let path = state.block.id.path();
    if path.ends_with("_door") && block_state_property(state, "half").is_some() {
        if !powered && door_has_adjacent_power_control(blocks, storage, pos, state, source) {
            return;
        }
        if let Some(door_edits) = plan_door_power_edits(blocks, storage, pos, state, powered) {
            for edit in door_edits {
                if edit.new_state != state_id && edited.insert(edit.pos) {
                    edits.push(edit);
                }
            }
        }
    } else if path.ends_with("_trapdoor")
        && (powered || !has_adjacent_power_control(blocks, storage, pos, source))
        && let Some(new_state) = powered_open_state(blocks, state, powered)
        && new_state != state_id
        && edited.insert(pos)
    {
        edits.push(BlockEdit { pos, new_state });
    } else if path == "piston"
        && (powered || !has_adjacent_power_control(blocks, storage, pos, source))
        && let Some(piston_edits) =
            plan_basic_piston_power_edits(blocks, storage, source, pos, state, powered)
        && piston_edits.iter().all(|edit| !edited.contains(&edit.pos))
        && piston_edits.iter().all(|edit| {
            protection.is_none_or(|protection| {
                protection.ambient_block_mutation_allowed("minecraft:overworld", edit.pos)
            })
        })
    {
        for edit in piston_edits {
            edited.insert(edit.pos);
            edits.push(edit);
        }
    }
}

fn plan_basic_piston_power_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    power_source: BlockPos,
    piston_pos: BlockPos,
    piston: &BlockState,
    powered: bool,
) -> Option<Vec<BlockEdit>> {
    let facing = block_state_property(piston, "facing")?;
    let extended = block_state_property(piston, "extended")? == "true";
    if powered == extended {
        return Some(Vec::new());
    }
    let arm_pos = offset_block_pos(piston_pos, facing, 1)?;
    let destination_pos = offset_block_pos(piston_pos, facing, 2)?;
    if power_source == arm_pos || power_source == destination_pos {
        return None;
    }
    let arm_state_id = storage.get_cached_block(arm_pos)?;
    let air = air_state_id(blocks);
    let piston_state = sibling_state_with_bool_property(blocks, piston, "extended", powered)?;

    if powered {
        let mut edits = vec![BlockEdit {
            pos: piston_pos,
            new_state: piston_state,
        }];
        if arm_state_id != air {
            let arm_state = blocks.by_id(arm_state_id)?;
            if !is_basic_piston_movable(arm_state) {
                return None;
            }
            if storage.get_cached_block(destination_pos)? != air {
                return None;
            }
            edits.push(BlockEdit {
                pos: destination_pos,
                new_state: arm_state_id,
            });
        }
        edits.push(BlockEdit {
            pos: arm_pos,
            new_state: piston_head_state(blocks, facing)?,
        });
        return Some(edits);
    }

    let arm_state = blocks.by_id(arm_state_id)?;
    if arm_state.block.id.path() != "piston_head" {
        return None;
    }
    let mut edits = vec![BlockEdit {
        pos: piston_pos,
        new_state: piston_state,
    }];
    edits.push(BlockEdit {
        pos: arm_pos,
        new_state: air,
    });
    Some(edits)
}

fn offset_block_pos(pos: BlockPos, facing: &str, distance: i32) -> Option<BlockPos> {
    let (dx, dy, dz) = match facing {
        "east" => (distance, 0, 0),
        "west" => (-distance, 0, 0),
        "up" => (0, distance, 0),
        "down" => (0, -distance, 0),
        "south" => (0, 0, distance),
        "north" => (0, 0, -distance),
        _ => return None,
    };
    Some(BlockPos {
        x: pos.x.checked_add(dx)?,
        y: pos.y.checked_add(dy)?,
        z: pos.z.checked_add(dz)?,
    })
}

fn piston_head_state(blocks: &BlockRegistry, facing: &str) -> Option<BlockStateId> {
    let head = Identifier::parse("minecraft:piston_head").ok()?;
    blocks.by_name_and_props(
        &head,
        &[
            ("facing".to_string(), facing.to_string()),
            ("short".to_string(), "false".to_string()),
            ("type".to_string(), "normal".to_string()),
        ],
    )
}

fn is_basic_piston_movable(state: &BlockState) -> bool {
    let path = state.block.id.path();
    state.properties.is_empty()
        && (matches!(
            path,
            "stone"
                | "cobblestone"
                | "dirt"
                | "coarse_dirt"
                | "rooted_dirt"
                | "sand"
                | "red_sand"
                | "gravel"
                | "clay"
                | "netherrack"
                | "end_stone"
        ) || path.ends_with("_planks")
            || path.ends_with("_wool")
            || path.ends_with("_concrete")
            || path.ends_with("_concrete_powder")
            || path.ends_with("_terracotta")
            || path.ends_with("_ore"))
}

fn door_has_adjacent_power_control(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state: &BlockState,
    source: BlockPos,
) -> bool {
    has_adjacent_power_control(blocks, storage, pos, source)
        || match block_state_property(state, "half") {
            Some("lower") => has_adjacent_power_control(
                blocks,
                storage,
                BlockPos {
                    y: pos.y + 1,
                    ..pos
                },
                source,
            ),
            Some("upper") => has_adjacent_power_control(
                blocks,
                storage,
                BlockPos {
                    y: pos.y - 1,
                    ..pos
                },
                source,
            ),
            _ => false,
        }
}

fn has_adjacent_power_control(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    target: BlockPos,
    excluded_source: BlockPos,
) -> bool {
    adjacent_block_positions(target)
        .into_iter()
        .filter(|pos| *pos != excluded_source)
        .any(|pos| {
            let Some(state_id) = storage.get_cached_block(pos) else {
                return false;
            };
            let Some(state) = blocks.by_id(state_id) else {
                return false;
            };
            is_power_control_powered(state)
        })
}

fn is_power_control_powered(state: &BlockState) -> bool {
    let path = state.block.id.path();
    (path.ends_with("_button") || path == "lever")
        && block_state_property(state, "powered") == Some("true")
}

fn plan_door_power_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state: &BlockState,
    powered: bool,
) -> Option<Vec<BlockEdit>> {
    let mut edits = Vec::with_capacity(2);
    edits.push(BlockEdit {
        pos,
        new_state: powered_open_state(blocks, state, powered)?,
    });
    let other_y = match block_state_property(state, "half")? {
        "lower" => pos.y + 1,
        "upper" => pos.y - 1,
        _ => return Some(edits),
    };
    let other_pos = BlockPos { y: other_y, ..pos };
    if let Some(other_state_id) = storage.get_cached_block(other_pos)
        && let Some(other_state) = blocks.by_id(other_state_id)
        && other_state.block.id == state.block.id
        && let Some(new_state) = powered_open_state(blocks, other_state, powered)
    {
        edits.push(BlockEdit {
            pos: other_pos,
            new_state,
        });
    }
    Some(edits)
}

fn powered_open_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    powered: bool,
) -> Option<BlockStateId> {
    let mut props = state.properties.clone();
    let (_, powered_prop) = props.iter_mut().find(|(key, _)| key == "powered")?;
    *powered_prop = if powered { "true" } else { "false" }.to_string();
    let (_, open_prop) = props.iter_mut().find(|(key, _)| key == "open")?;
    *open_prop = if powered { "true" } else { "false" }.to_string();
    blocks.by_name_and_props(&state.block.id, &props)
}

fn plan_door_toggle_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state: &BlockState,
) -> Option<Vec<BlockEdit>> {
    let new_open = block_state_property(state, "open")? != "true";
    let mut edits = Vec::with_capacity(2);
    edits.push(BlockEdit {
        pos,
        new_state: sibling_state_with_bool_property(blocks, state, "open", new_open)?,
    });
    let other_y = match block_state_property(state, "half")? {
        "lower" => pos.y + 1,
        "upper" => pos.y - 1,
        _ => return Some(edits),
    };
    let other_pos = BlockPos { y: other_y, ..pos };
    if let Some(other_state_id) = storage.get_cached_block(other_pos)
        && let Some(other_state) = blocks.by_id(other_state_id)
        && other_state.block.id == state.block.id
        && let Some(new_state) =
            sibling_state_with_bool_property(blocks, other_state, "open", new_open)
    {
        edits.push(BlockEdit {
            pos: other_pos,
            new_state,
        });
    }
    Some(edits)
}

pub(super) fn toggled_bool_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    name: &str,
) -> Option<BlockStateId> {
    let next = match block_state_property(state, name)? {
        "true" => false,
        "false" => true,
        _ => return None,
    };
    sibling_state_with_bool_property(blocks, state, name, next)
}
