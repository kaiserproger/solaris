use std::collections::HashSet;

use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId, ScheduledBlockTick};

use super::{
    BlockEdit, BlockEditPrecondition, BlockPlanningRead, adjacent_block_positions,
    block_state_property, sibling_state_with_bool_property,
};

const BUTTON_RELEASE_DELAY_TICKS: u64 = 20;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ToggleBlockPlan {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) preconditions: Vec<BlockEditPrecondition>,
    pub(super) scheduled_block_ticks: Vec<ScheduledBlockTick>,
}

pub(super) fn plan_toggle_block_interaction(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    pos: BlockPos,
    state_id: BlockStateId,
    world_tick: u64,
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
        return plan_power_control_interaction(blocks, storage, pos, state, true, world_tick);
    }
    if path == "lever" {
        return plan_power_control_interaction(blocks, storage, pos, state, false, world_tick);
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
    extend_adjacent_power_target_edits(blocks, storage, pos, next_powered, &mut edits);
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
    toggle_block_plan(storage, edits, scheduled_block_ticks)
}

fn toggle_block_plan(
    storage: &dyn BlockPlanningRead,
    edits: Vec<BlockEdit>,
    scheduled_block_ticks: Vec<ScheduledBlockTick>,
) -> Option<ToggleBlockPlan> {
    let mut seen = HashSet::with_capacity(edits.len());
    let mut preconditions = Vec::with_capacity(edits.len());
    for edit in &edits {
        if !seen.insert(edit.pos) {
            continue;
        }
        preconditions.push(BlockEditPrecondition {
            pos: edit.pos,
            expected_state: storage.get_cached_block(edit.pos)?,
            expected_token: storage.block_mutation_token(edit.pos)?,
        });
    }
    Some(ToggleBlockPlan {
        edits,
        preconditions,
        scheduled_block_ticks,
    })
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
    edits: &mut Vec<BlockEdit>,
) {
    let mut edited = edits.iter().map(|edit| edit.pos).collect::<HashSet<_>>();
    for pos in adjacent_block_positions(source) {
        extend_power_target_edits(blocks, storage, source, pos, powered, edits, &mut edited);
    }
}

fn extend_power_target_edits(
    blocks: &BlockRegistry,
    storage: &dyn BlockPlanningRead,
    source: BlockPos,
    pos: BlockPos,
    powered: bool,
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
    }
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
