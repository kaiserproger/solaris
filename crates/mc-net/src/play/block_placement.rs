use std::cell::RefCell;

use mc_data::Identifier;
use mc_data::block_placement_26_1_2::{
    PlacementBlockState, SignState, StairCell, StairNeighborState, StairProperties,
    apply_waterlogged_state, can_merge_slab, merge_slab_state, opposite, resolve_stair_shape,
    sign_state_for_direction, torch_state_for_direction,
};
use mc_domain::Direction;
use mc_nbt::{ListTag, Tag};
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId, WorldReadSnapshot};

use super::{
    BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition, BlockPlanningRead, PendingSignEdit,
    PlayerPose, sibling_state_with_property,
};
use mc_world::plant_rules_26_1_2::vertical_plant_can_survive_at;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PlannedBlockPlacement {
    pub(super) edits: Vec<BlockEdit>,
    pub(super) additional_preconditions: Vec<BlockEditPrecondition>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PlannedStairTransition {
    pub(super) target_state: BlockStateId,
    pub(super) neighbor_edits: Vec<BlockEdit>,
    pub(super) dependency_preconditions: Vec<BlockEditPrecondition>,
}

pub(super) fn placement_snapshot_positions(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    pos: BlockPos,
) -> Option<Vec<BlockPos>> {
    let placed = blocks.by_id(placed_state)?;
    if is_editable_sign_state(placed) || placed.block.id.path().ends_with("_wall_sign") {
        return Some(Vec::new());
    }
    if placed.block.id.path().ends_with("_door") {
        let mut positions = vec![BlockPos {
            y: pos.y + 1,
            ..pos
        }];
        positions.extend(stair_shape_dependency_positions(pos));
        return Some(positions);
    }
    if is_stair_identity(placed) {
        return Some(stair_shape_dependency_positions(pos));
    }

    let below = BlockPos {
        y: pos.y - 1,
        ..pos
    };
    let mut positions = vec![pos, below];
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        positions.push(BlockPos {
            x: pos.x + dx,
            z: pos.z + dz,
            ..pos
        });
        positions.push(BlockPos {
            x: below.x + dx,
            z: below.z + dz,
            ..below
        });
    }
    for dependency in stair_shape_dependency_positions(pos) {
        if !positions.contains(&dependency) {
            positions.push(dependency);
        }
    }
    Some(positions)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_block_placement(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    snapshot: Option<&WorldReadSnapshot>,
    pos: BlockPos,
    player_pose: PlayerPose,
    direction: Direction,
    target_relative_hit_y: f32,
    air: BlockStateId,
) -> Option<PlannedBlockPlacement> {
    let placed = blocks.by_id(placed_state)?;
    if let Some(new_state) = sign_placement_state(blocks, placed, player_pose, direction) {
        return Some(PlannedBlockPlacement {
            edits: vec![BlockEdit { pos, new_state }],
            additional_preconditions: Vec::new(),
        });
    }
    let placed_state = oriented_stair_or_slab_state(
        blocks,
        placed_state,
        placed,
        player_pose,
        direction,
        target_relative_hit_y,
    );

    let snapshot = snapshot?;
    let target_state = snapshot.get_cached_block(pos)?;
    let placed_state = merge_or_waterlogged(blocks, target_state, placed_state)?;
    let placed = blocks.by_id(placed_state)?;
    if is_stair_identity(placed) {
        if !matches!(
            classify_stair_state(blocks, placed_state),
            StairState::Valid(_)
        ) {
            return None;
        }
        return plan_stair_placement(blocks, snapshot, pos, target_state, placed_state);
    }
    if placed.block.id.path() == "torch" {
        let (new_state, support_precondition) =
            torch_placement_state(blocks, placed_state, snapshot, pos, direction)?;
        return append_stair_transition_to_placement(
            blocks,
            snapshot,
            pos,
            target_state,
            PlannedBlockPlacement {
                edits: vec![BlockEdit { pos, new_state }],
                additional_preconditions: vec![support_precondition],
            },
        );
    }
    if !placed.block.id.path().ends_with("_door") {
        if !vertical_plant_can_survive_at(blocks, snapshot, pos, placed_state) {
            return None;
        }
        if placed.block.id.path() == "cactus" && cactus_has_side_neighbor(blocks, snapshot, pos) {
            return None;
        }
        let mut edits = vec![BlockEdit {
            pos,
            new_state: placed_state,
        }];
        append_cactus_side_neighbor_cascades(blocks, snapshot, &mut edits, pos, placed_state, air);
        let mut additional_preconditions = Vec::with_capacity(edits.len().saturating_sub(1));
        for edit in edits.iter().skip(1) {
            let expected_state = snapshot.get_cached_block(edit.pos)?;
            let expected_token = snapshot.block_mutation_token(edit.pos)?;
            additional_preconditions.push(BlockEditPrecondition {
                pos: edit.pos,
                expected_state,
                expected_token,
            });
        }
        return append_stair_transition_to_placement(
            blocks,
            snapshot,
            pos,
            target_state,
            PlannedBlockPlacement {
                edits,
                additional_preconditions,
            },
        );
    }

    let upper_pos = BlockPos {
        y: pos.y + 1,
        ..pos
    };
    let upper_token = match snapshot.get_cached_block(upper_pos) {
        Some(state_id) if state_id == air => snapshot.block_mutation_token(upper_pos),
        Some(_) | None => None,
    }?;
    let facing = horizontal_facing_from_yaw(player_pose.yaw);
    let lower = door_half_state(blocks, placed, "lower", facing)?;
    let upper = door_half_state(blocks, placed, "upper", facing)?;
    append_stair_transition_to_placement(
        blocks,
        snapshot,
        pos,
        target_state,
        PlannedBlockPlacement {
            edits: vec![
                BlockEdit {
                    pos,
                    new_state: lower,
                },
                BlockEdit {
                    pos: upper_pos,
                    new_state: upper,
                },
            ],
            additional_preconditions: vec![BlockEditPrecondition {
                pos: upper_pos,
                expected_state: air,
                expected_token: upper_token,
            }],
        },
    )
}

pub(super) fn same_slab_can_replace(
    blocks: &BlockRegistry,
    held_state: BlockStateId,
    clicked_state: BlockStateId,
    clicked_face: Direction,
    clicked_relative_hit_y: f32,
) -> bool {
    let Some(held) = blocks.by_id(held_state) else {
        return false;
    };
    let Some(clicked) = blocks.by_id(clicked_state) else {
        return false;
    };
    if !can_merge_slab(&to_neutral(held), &to_neutral(clicked)) {
        return false;
    }
    match property(clicked, "type") {
        Some("bottom") => {
            clicked_face == Direction::Up
                || (is_horizontal(clicked_face) && clicked_relative_hit_y > 0.5)
        }
        Some("top") => {
            clicked_face == Direction::Down
                || (is_horizontal(clicked_face)
                    && !matches!(
                        clicked_relative_hit_y.partial_cmp(&0.5),
                        Some(std::cmp::Ordering::Greater)
                    ))
        }
        Some("double") | None | Some(_) => false,
    }
}

pub(super) fn adjacent_placement_target_is_replaceable(
    blocks: &BlockRegistry,
    held_state: BlockStateId,
    target_state: BlockStateId,
    air: BlockStateId,
) -> bool {
    let held = blocks.by_id(held_state);
    let target = blocks.by_id(target_state);
    target_state == air
        || is_water_state(blocks, target_state)
        || held
            .zip(target)
            .is_some_and(|(held, target)| can_merge_slab(&to_neutral(held), &to_neutral(target)))
}

fn is_water_state(blocks: &BlockRegistry, state: BlockStateId) -> bool {
    blocks
        .by_id(state)
        .is_some_and(|state| state.block.id.as_str() == "minecraft:water")
}

fn merge_or_waterlogged(
    blocks: &BlockRegistry,
    target_state: BlockStateId,
    placed_state: BlockStateId,
) -> Option<BlockStateId> {
    let target = blocks.by_id(target_state)?;
    let placed = blocks.by_id(placed_state)?;
    let resolved = merge_slab_state(&to_neutral(target), &to_neutral(placed))
        .unwrap_or_else(|| apply_waterlogged_state(&to_neutral(placed), &to_neutral(target)));
    to_state_id(blocks, &resolved)
}

fn to_neutral(state: &BlockState) -> PlacementBlockState {
    PlacementBlockState {
        block_id: state.block.id.as_str().to_string(),
        properties: state.properties.clone(),
    }
}

fn to_state_id(blocks: &BlockRegistry, state: &PlacementBlockState) -> Option<BlockStateId> {
    blocks.by_name_and_props(&Identifier::parse(&state.block_id).ok()?, &state.properties)
}

fn plan_stair_placement(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pos: BlockPos,
    previous_state: BlockStateId,
    placed_state: BlockStateId,
) -> Option<PlannedBlockPlacement> {
    let transition =
        plan_stair_state_transition(blocks, snapshot, pos, previous_state, placed_state)?;
    let mut edits = vec![BlockEdit {
        pos,
        new_state: transition.target_state,
    }];
    edits.extend(transition.neighbor_edits);
    Some(PlannedBlockPlacement {
        edits,
        additional_preconditions: transition.dependency_preconditions,
    })
}

fn append_stair_transition_to_placement(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pos: BlockPos,
    previous_state: BlockStateId,
    mut plan: PlannedBlockPlacement,
) -> Option<PlannedBlockPlacement> {
    let target_edit = plan.edits.iter_mut().find(|edit| edit.pos == pos)?;
    let transition =
        plan_stair_state_transition(blocks, snapshot, pos, previous_state, target_edit.new_state)?;
    target_edit.new_state = transition.target_state;
    for edit in transition.neighbor_edits {
        push_unique_block_edit(&mut plan.edits, edit);
    }
    for precondition in transition.dependency_preconditions {
        if !plan
            .additional_preconditions
            .iter()
            .any(|existing| existing.pos == precondition.pos)
        {
            plan.additional_preconditions.push(precondition);
        }
    }
    Some(plan)
}

pub(super) fn plan_stair_state_transition(
    blocks: &BlockRegistry,
    world: &impl BlockPlanningRead,
    pos: BlockPos,
    previous_state: BlockStateId,
    new_state: BlockStateId,
) -> Option<PlannedStairTransition> {
    let previous_stair = classify_stair_state(blocks, previous_state);
    let new_stair = classify_stair_state(blocks, new_state);
    if matches!(previous_stair, StairState::Malformed) || matches!(new_stair, StairState::Malformed)
    {
        return None;
    }
    if previous_state == new_state {
        return Some(PlannedStairTransition {
            target_state: new_state,
            neighbor_edits: Vec::new(),
            dependency_preconditions: Vec::new(),
        });
    }
    let ordinary_transition =
        matches!(previous_stair, StairState::NotStair) && matches!(new_stair, StairState::NotStair);

    let read_positions = RefCell::new(Vec::new());
    let provisional_block_at =
        |read_pos| transition_block_at(world, &read_positions, pos, new_state, read_pos);
    let target_state = match new_stair {
        StairState::Valid(_) => {
            let neighbors = build_stair_neighbor_state(blocks, &provisional_block_at, pos)?;
            stair_state_with_resolved_shape(blocks, new_state, neighbors)?
        }
        StairState::NotStair => new_state,
        StairState::Malformed => return None,
    };
    let block_at =
        |read_pos| transition_block_at(world, &read_positions, pos, target_state, read_pos);
    let mut neighbor_edits = Vec::new();
    for direction in HORIZONTAL_DIRECTIONS {
        let neighbor_pos = relative(pos, direction);
        let neighbor_state = if ordinary_transition {
            world.get_cached_block(neighbor_pos)
        } else {
            block_at(neighbor_pos)
        };
        let Some(neighbor_state) = neighbor_state else {
            if ordinary_transition {
                continue;
            }
            return None;
        };
        match classify_stair_state(blocks, neighbor_state) {
            StairState::NotStair => continue,
            StairState::Malformed => return None,
            StairState::Valid(_) => {}
        }
        if ordinary_transition {
            let mut positions = read_positions.borrow_mut();
            if !positions.contains(&neighbor_pos) {
                positions.push(neighbor_pos);
            }
        }
        let neighbors = match build_stair_neighbor_state(blocks, &block_at, neighbor_pos) {
            Some(neighbors) => neighbors,
            None if ordinary_transition => continue,
            None => return None,
        };
        let Some(updated) = stair_state_with_resolved_shape(blocks, neighbor_state, neighbors)
        else {
            if ordinary_transition {
                continue;
            }
            return None;
        };
        if updated != neighbor_state {
            neighbor_edits.push(BlockEdit {
                pos: neighbor_pos,
                new_state: updated,
            });
        }
    }

    let dependency_preconditions = read_positions
        .into_inner()
        .into_iter()
        .map(|dependency| {
            Some(BlockEditPrecondition {
                pos: dependency,
                expected_state: world.get_cached_block(dependency)?,
                expected_token: world.block_mutation_token(dependency)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PlannedStairTransition {
        target_state,
        neighbor_edits,
        dependency_preconditions,
    })
}

fn transition_block_at(
    world: &impl BlockPlanningRead,
    read_positions: &RefCell<Vec<BlockPos>>,
    transition_pos: BlockPos,
    transition_state: BlockStateId,
    read_pos: BlockPos,
) -> Option<BlockStateId> {
    if read_pos == transition_pos {
        return Some(transition_state);
    }
    let state = world.get_cached_block(read_pos)?;
    let mut positions = read_positions.borrow_mut();
    if !positions.contains(&read_pos) {
        positions.push(read_pos);
    }
    drop(positions);
    Some(state)
}

#[cfg(test)]
fn stair_state_with_shape(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    stair_pos: BlockPos,
    stair_state: BlockStateId,
    placed_pos: BlockPos,
    placed_state: BlockStateId,
) -> Option<BlockStateId> {
    let block_at = |read_pos| {
        (read_pos == placed_pos)
            .then_some(placed_state)
            .or_else(|| snapshot.get_cached_block(read_pos))
    };
    let neighbors = build_stair_neighbor_state(blocks, &block_at, stair_pos)?;
    stair_state_with_resolved_shape(blocks, stair_state, neighbors)
}

fn stair_state_with_resolved_shape(
    blocks: &BlockRegistry,
    stair_state: BlockStateId,
    neighbors: StairNeighborState,
) -> Option<BlockStateId> {
    let StairState::Valid(current) = classify_stair_state(blocks, stair_state) else {
        return None;
    };
    let shape = resolve_stair_shape(current, neighbors)?;
    sibling_state_with_property(
        blocks,
        blocks.by_id(stair_state)?,
        "shape",
        shape.property_value(),
    )
}

#[cfg(test)]
fn stairs_shape(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pos: BlockPos,
    state: BlockStateId,
    placed_pos: BlockPos,
    placed_state: BlockStateId,
) -> Option<&'static str> {
    let block_at = |read_pos| {
        (read_pos == placed_pos)
            .then_some(placed_state)
            .or_else(|| snapshot.get_cached_block(read_pos))
    };
    let neighbors = build_stair_neighbor_state(blocks, &block_at, pos)?;
    let StairState::Valid(current) = classify_stair_state(blocks, state) else {
        return None;
    };
    resolve_stair_shape(current, neighbors).map(|shape| shape.property_value())
}

fn build_stair_neighbor_state(
    blocks: &BlockRegistry,
    block_at: &impl Fn(BlockPos) -> Option<BlockStateId>,
    pos: BlockPos,
) -> Option<StairNeighborState> {
    Some(StairNeighborState {
        north: classify_stair_cell(blocks, block_at(relative(pos, Direction::North))?),
        south: classify_stair_cell(blocks, block_at(relative(pos, Direction::South))?),
        west: classify_stair_cell(blocks, block_at(relative(pos, Direction::West))?),
        east: classify_stair_cell(blocks, block_at(relative(pos, Direction::East))?),
    })
}

fn classify_stair_cell(blocks: &BlockRegistry, state: BlockStateId) -> StairCell {
    match classify_stair_state(blocks, state) {
        StairState::NotStair => StairCell::NotStair,
        StairState::Valid(properties) => StairCell::Stair(properties),
        StairState::Malformed => StairCell::Malformed,
    }
}

enum StairState {
    NotStair,
    Valid(StairProperties),
    Malformed,
}

fn classify_stair_state(blocks: &BlockRegistry, state: BlockStateId) -> StairState {
    let Some(state) = blocks.by_id(state) else {
        return StairState::Malformed;
    };
    if !is_stair_identity(state) {
        return StairState::NotStair;
    }
    if !has_canonical_stair_schema(state)
        || !matches!(
            property(state, "shape"),
            Some("straight" | "inner_left" | "inner_right" | "outer_left" | "outer_right")
        )
        || !matches!(property(state, "waterlogged"), Some("false" | "true"))
    {
        return StairState::Malformed;
    }
    let Some(facing) = property(state, "facing").and_then(horizontal_direction) else {
        return StairState::Malformed;
    };
    let top = match property(state, "half") {
        Some("bottom") => false,
        Some("top") => true,
        Some(_) | None => return StairState::Malformed,
    };
    StairState::Valid(StairProperties { facing, top })
}

fn is_stair_identity(state: &BlockState) -> bool {
    state.block.id.namespace() == "minecraft" && state.block.id.path().ends_with("_stairs")
}

fn has_canonical_stair_schema(state: &BlockState) -> bool {
    let properties = &state.block.properties;
    properties.len() == 4
        && properties
            .iter()
            .any(|(name, values)| name == "facing" && values == &["north", "south", "west", "east"])
        && properties
            .iter()
            .any(|(name, values)| name == "half" && values == &["top", "bottom"])
        && properties.iter().any(|(name, values)| {
            name == "shape"
                && values
                    == &[
                        "straight",
                        "inner_left",
                        "inner_right",
                        "outer_left",
                        "outer_right",
                    ]
        })
        && properties
            .iter()
            .any(|(name, values)| name == "waterlogged" && values == &["true", "false"])
}

const HORIZONTAL_DIRECTIONS: [Direction; 4] = [
    Direction::North,
    Direction::South,
    Direction::West,
    Direction::East,
];

fn stair_shape_dependency_positions(pos: BlockPos) -> Vec<BlockPos> {
    let mut positions = Vec::with_capacity(13);
    for dx in -2_i32..=2 {
        for dz in -2_i32..=2 {
            if dx.abs() + dz.abs() <= 2 {
                positions.push(BlockPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    ..pos
                });
            }
        }
    }
    positions
}

fn property<'a>(state: &'a BlockState, name: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find_map(|(property, value)| (property == name).then_some(value.as_str()))
}

fn horizontal_direction(value: &str) -> Option<Direction> {
    match value {
        "north" => Some(Direction::North),
        "south" => Some(Direction::South),
        "west" => Some(Direction::West),
        "east" => Some(Direction::East),
        _ => None,
    }
}

fn relative(pos: BlockPos, direction: Direction) -> BlockPos {
    let (dx, dy, dz) = direction.normal();
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

fn is_horizontal(direction: Direction) -> bool {
    matches!(
        direction,
        Direction::North | Direction::South | Direction::West | Direction::East
    )
}

fn torch_placement_state(
    blocks: &BlockRegistry,
    standing_state: BlockStateId,
    snapshot: &WorldReadSnapshot,
    pos: BlockPos,
    direction: Direction,
) -> Option<(BlockStateId, BlockEditPrecondition)> {
    let torch = torch_state_for_direction(direction)?;
    let new_state = if torch.block_id == "minecraft:torch" {
        standing_state
    } else {
        let wall_torch = Identifier::parse(torch.block_id).ok()?;
        blocks.by_name_and_props(
            &wall_torch,
            &[("facing".to_string(), torch.facing?.to_string())],
        )?
    };
    let support = relative(pos, opposite(direction));
    let support_state = snapshot.get_cached_block(support)?;
    if !has_full_sturdy_face(blocks, support_state, direction) {
        return None;
    }
    Some((
        new_state,
        BlockEditPrecondition {
            pos: support,
            expected_state: support_state,
            expected_token: snapshot.block_mutation_token(support)?,
        },
    ))
}

fn has_full_sturdy_face(blocks: &BlockRegistry, state_id: BlockStateId, face: Direction) -> bool {
    let Some(state) = blocks.by_id(state_id) else {
        return false;
    };
    let face = match face {
        Direction::Down => mc_data::block_facts::SturdyFace::Down,
        Direction::Up => mc_data::block_facts::SturdyFace::Up,
        Direction::North => mc_data::block_facts::SturdyFace::North,
        Direction::South => mc_data::block_facts::SturdyFace::South,
        Direction::West => mc_data::block_facts::SturdyFace::West,
        Direction::East => mc_data::block_facts::SturdyFace::East,
    };
    mc_data::block_facts::has_full_sturdy_face(state_id.0, &state.block.id, &state.properties, face)
}

fn oriented_stair_or_slab_state(
    blocks: &BlockRegistry,
    placed_state: BlockStateId,
    state: &BlockState,
    player_pose: PlayerPose,
    direction: Direction,
    target_relative_hit_y: f32,
) -> BlockStateId {
    let path = state.block.id.path();
    let mut properties = state.properties.clone();
    if is_stair_identity(state) {
        set_prop_if_present(
            &mut properties,
            "facing",
            horizontal_facing_from_yaw(player_pose.yaw),
        );
        set_prop_if_present(
            &mut properties,
            "half",
            stair_or_slab_half(direction, target_relative_hit_y),
        );
    } else if path.ends_with("_slab") {
        set_prop_if_present(
            &mut properties,
            "type",
            stair_or_slab_half(direction, target_relative_hit_y),
        );
    } else {
        return placed_state;
    }

    blocks
        .by_name_and_props(&state.block.id, &properties)
        .unwrap_or(placed_state)
}

fn stair_or_slab_half(direction: Direction, target_relative_hit_y: f32) -> &'static str {
    if direction == Direction::Down || (direction != Direction::Up && target_relative_hit_y > 0.5) {
        "top"
    } else {
        "bottom"
    }
}

pub(super) fn append_cactus_side_neighbor_cascades(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    edits: &mut Vec<BlockEdit>,
    placed: BlockPos,
    placed_state: BlockStateId,
    air: BlockStateId,
) {
    if blocks
        .by_id(placed_state)
        .is_none_or(|state| !is_known_cactus_side_obstructor(state.block.id.path()))
    {
        return;
    }

    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let mut y = placed.y;
        loop {
            let pos = BlockPos {
                x: placed.x + dx,
                y,
                z: placed.z + dz,
            };
            let Some(state_id) = snapshot.get_cached_block(pos) else {
                break;
            };
            let Some(state) = blocks.by_id(state_id) else {
                break;
            };
            if state.block.id.path() != "cactus" {
                break;
            }
            push_unique_block_edit(
                edits,
                BlockEdit {
                    pos,
                    new_state: air,
                },
            );
            y += 1;
        }
    }
}

fn cactus_has_side_neighbor(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    placed: BlockPos,
) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let pos = BlockPos {
                x: placed.x + dx,
                y: placed.y,
                z: placed.z + dz,
            };
            let Some(state_id) = snapshot.get_cached_block(pos) else {
                return false;
            };
            blocks
                .by_id(state_id)
                .is_some_and(|state| state.block.id.path() == "cactus")
        })
}

fn is_known_cactus_side_obstructor(path: &str) -> bool {
    matches!(
        path,
        "stone"
            | "granite"
            | "polished_granite"
            | "diorite"
            | "polished_diorite"
            | "andesite"
            | "polished_andesite"
            | "deepslate"
            | "cobbled_deepslate"
            | "tuff"
            | "calcite"
            | "dripstone_block"
            | "grass_block"
            | "dirt"
            | "coarse_dirt"
            | "podzol"
            | "rooted_dirt"
            | "mud"
            | "clay"
            | "sand"
            | "red_sand"
            | "gravel"
            | "cobblestone"
            | "mossy_cobblestone"
            | "obsidian"
            | "crying_obsidian"
            | "bedrock"
            | "netherrack"
            | "basalt"
            | "smooth_basalt"
            | "blackstone"
            | "end_stone"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
    ) || path.ends_with("_planks")
        || path.ends_with("_log")
        || path.ends_with("_wood")
        || path.ends_with("_stem")
        || path.ends_with("_hyphae")
        || path.ends_with("_leaves")
        || path.ends_with("_wool")
        || path.ends_with("_terracotta")
        || path.ends_with("_concrete")
        || path.ends_with("_concrete_powder")
}

pub(super) fn sign_placement_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    player_pose: PlayerPose,
    direction: Direction,
) -> Option<BlockStateId> {
    let sign_state = sign_state_for_direction(direction, player_pose.yaw)?;
    let path = state.block.id.path();
    if path.ends_with("_wall_sign") {
        let SignState::Wall { facing } = sign_state else {
            return None;
        };
        return sibling_state_with_property(blocks, state, "facing", facing);
    }
    if is_editable_sign_state(state) {
        let SignState::Standing { rotation } = sign_state else {
            return None;
        };
        return sibling_state_with_property(blocks, state, "rotation", &rotation.to_string());
    }
    None
}

pub(super) fn placed_sign_edit(
    blocks: &BlockRegistry,
    outcome: &BlockEditBatchOutcome,
) -> Option<PendingSignEdit> {
    outcome.applied.iter().find_map(|edit| {
        blocks
            .by_id(edit.new_state)
            .filter(|state| is_editable_sign_state(state))
            .and_then(|_| {
                outcome
                    .resulting_tokens
                    .get(&edit.pos)
                    .copied()
                    .map(|token| PendingSignEdit {
                        position: edit.pos,
                        state: edit.new_state,
                        token,
                        is_front_text: true,
                    })
            })
    })
}

fn is_editable_sign_state(state: &BlockState) -> bool {
    let path = state.block.id.path();
    path.ends_with("_sign") && !path.ends_with("_hanging_sign")
}

pub(super) fn sign_block_entity_update_nbt(lines: &[String], is_front_text: bool) -> Tag {
    let text = sign_text_nbt(lines);
    let empty = sign_text_nbt(&[]);
    Tag::Compound(vec![
        (
            "front_text".into(),
            if is_front_text {
                text.clone()
            } else {
                empty.clone()
            },
        ),
        ("back_text".into(), if is_front_text { empty } else { text }),
        ("is_waxed".into(), Tag::Byte(0)),
    ])
}

pub(super) fn sign_block_entity_persistent_nbt(pos: BlockPos, update_tag: &Tag) -> Tag {
    let Tag::Compound(fields) = update_tag else {
        unreachable!("sign update tag is always a compound")
    };
    let mut fields = fields.clone();
    fields.extend([
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("id".into(), Tag::String("minecraft:sign".into())),
    ]);
    Tag::Compound(fields)
}

fn sign_text_nbt(lines: &[String]) -> Tag {
    let messages = (0..4)
        .map(|idx| Tag::String(lines.get(idx).cloned().unwrap_or_default()))
        .collect();
    Tag::Compound(vec![
        (
            "messages".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::STRING,
                elements: messages,
            }),
        ),
        ("color".into(), Tag::String("black".into())),
        ("has_glowing_text".into(), Tag::Byte(0)),
    ])
}

pub(super) fn door_half_state(
    blocks: &BlockRegistry,
    state: &BlockState,
    half: &str,
    facing: &str,
) -> Option<BlockStateId> {
    let mut props = state.properties.clone();
    set_prop_if_present(&mut props, "half", half);
    set_prop_if_present(&mut props, "facing", facing);
    set_prop_if_present(&mut props, "open", "false");
    set_prop_if_present(&mut props, "powered", "false");
    blocks.by_name_and_props(&state.block.id, &props)
}

fn set_prop_if_present(props: &mut [(String, String)], name: &str, value: &str) {
    if let Some((_, current)) = props.iter_mut().find(|(key, _)| key == name) {
        *current = value.to_string();
    }
}

pub(super) fn horizontal_facing_from_yaw(yaw: f32) -> &'static str {
    match ((yaw.rem_euclid(360.0) / 90.0).round() as i32).rem_euclid(4) {
        0 => "south",
        1 => "west",
        2 => "north",
        _ => "east",
    }
}

fn push_unique_block_edit(edits: &mut Vec<BlockEdit>, edit: BlockEdit) {
    if edits.iter().any(|existing| existing.pos == edit.pos) {
        return;
    }
    edits.push(edit);
}

#[cfg(test)]
#[path = "block_placement_support_tests.rs"]
mod support_tests;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use mc_data::Identifier;
    use mc_data::block_placement_26_1_2::{counter_clockwise, opposite};
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_protocol::packets::play::Direction;
    use mc_world::{BlockPos, BlockRegistry, BlockStateId};

    use super::super::use_item_on_adapter::{
        conditional_placement_rejects_test_mutation, placement_snapshot_for_test,
    };
    use super::super::{BlockEdit, PlayerPose};
    use super::{
        placement_snapshot_positions, plan_block_placement, relative, same_slab_can_replace,
        stair_shape_dependency_positions, stair_state_with_shape, stairs_shape,
    };

    fn simple_block(id: u32, name: &str) -> BlockReport {
        BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id,
                default: true,
                properties: BTreeMap::new(),
            }],
        }
    }

    fn state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
        BlockStateReport {
            id,
            default,
            properties: properties
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    fn prop_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(name, values)| {
                (
                    (*name).to_string(),
                    values.iter().map(|value| (*value).to_string()).collect(),
                )
            })
            .collect()
    }

    fn slab_and_stair_registry() -> Arc<BlockRegistry> {
        let slab = BlockReport {
            id: Identifier::parse("minecraft:oak_slab").unwrap(),
            properties: prop_schema(&[
                ("type", &["bottom", "top", "double"]),
                ("waterlogged", &["false", "true"]),
            ]),
            states: ["bottom", "top", "double"]
                .into_iter()
                .flat_map(|slab_type| {
                    ["false", "true"].map(move |waterlogged| (slab_type, waterlogged))
                })
                .enumerate()
                .map(|(offset, (slab_type, waterlogged))| {
                    state(
                        1 + offset as u32,
                        slab_type == "bottom" && waterlogged == "false",
                        &[("type", slab_type), ("waterlogged", waterlogged)],
                    )
                })
                .collect(),
        };
        let mut stair_states = Vec::new();
        let mut id = 7;
        for facing in ["north", "south", "west", "east"] {
            for half in ["bottom", "top"] {
                for shape in [
                    "straight",
                    "inner_left",
                    "inner_right",
                    "outer_left",
                    "outer_right",
                ] {
                    for waterlogged in ["false", "true"] {
                        stair_states.push(state(
                            id,
                            facing == "north"
                                && half == "bottom"
                                && shape == "straight"
                                && waterlogged == "false",
                            &[
                                ("facing", facing),
                                ("half", half),
                                ("shape", shape),
                                ("waterlogged", waterlogged),
                            ],
                        ));
                        id += 1;
                    }
                }
            }
        }
        let stairs = BlockReport {
            id: Identifier::parse("minecraft:oak_stairs").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north", "south", "west", "east"]),
                ("half", &["top", "bottom"]),
                (
                    "shape",
                    &[
                        "straight",
                        "inner_left",
                        "inner_right",
                        "outer_left",
                        "outer_right",
                    ],
                ),
                ("waterlogged", &["true", "false"]),
            ]),
            states: stair_states,
        };
        Arc::new(
            BlockRegistry::from_report(&[
                simple_block(0, "minecraft:air"),
                slab,
                stairs,
                simple_block(id, "minecraft:stone"),
                simple_block(id + 1, "minecraft:malformed_stairs"),
            ])
            .unwrap(),
        )
    }

    fn block_state(
        blocks: &BlockRegistry,
        name: &str,
        properties: &[(&str, &str)],
    ) -> BlockStateId {
        blocks
            .by_name_and_props(
                &Identifier::parse(name).unwrap(),
                &properties
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect::<Vec<_>>(),
            )
            .unwrap()
    }

    fn pose_with_yaw(yaw: f32) -> PlayerPose {
        let mut pose = PlayerPose::new(0.0, 64.0, 0.0);
        pose.yaw = yaw;
        pose
    }

    fn stair_state(
        blocks: &BlockRegistry,
        facing: &str,
        half: &str,
        shape: &str,
        waterlogged: &str,
    ) -> BlockStateId {
        block_state(
            blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", facing),
                ("half", half),
                ("shape", shape),
                ("waterlogged", waterlogged),
            ],
        )
    }

    fn direction_name(direction: Direction) -> &'static str {
        match direction {
            Direction::North => "north",
            Direction::South => "south",
            Direction::West => "west",
            Direction::East => "east",
            Direction::Down | Direction::Up => unreachable!("stair direction is horizontal"),
        }
    }

    fn clockwise(direction: Direction) -> Direction {
        opposite(counter_clockwise(direction).expect("stair direction is horizontal"))
    }

    #[test]
    fn same_slab_replacement_matches_clicked_face_and_hit_half() {
        let blocks = slab_and_stair_registry();
        let held = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let bottom = held;
        let top = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "top"), ("waterlogged", "false")],
        );

        assert!(same_slab_can_replace(
            &blocks,
            held,
            bottom,
            Direction::Up,
            0.1
        ));
        assert!(same_slab_can_replace(
            &blocks,
            held,
            bottom,
            Direction::North,
            0.75
        ));
        assert!(!same_slab_can_replace(
            &blocks,
            held,
            bottom,
            Direction::North,
            0.25
        ));
        assert!(same_slab_can_replace(
            &blocks,
            held,
            top,
            Direction::Down,
            0.9
        ));
        assert!(same_slab_can_replace(
            &blocks,
            held,
            top,
            Direction::North,
            0.25
        ));
        assert!(!same_slab_can_replace(
            &blocks,
            held,
            top,
            Direction::North,
            0.75
        ));
    }

    #[test]
    fn top_and_bottom_slabs_convert_to_a_dry_double() {
        let blocks = slab_and_stair_registry();
        let held = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let double = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "double"), ("waterlogged", "false")],
        );
        let pos = BlockPos { x: 4, y: 64, z: 4 };

        for (existing, clicked_face) in [
            (held, Direction::Up),
            (
                block_state(
                    &blocks,
                    "minecraft:oak_slab",
                    &[("type", "top"), ("waterlogged", "true")],
                ),
                Direction::Down,
            ),
        ] {
            let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, existing)]);
            let plan = plan_block_placement(
                &blocks,
                held,
                Some(&snapshot),
                pos,
                pose_with_yaw(0.0),
                clicked_face,
                0.5,
                BlockStateId(0),
            )
            .unwrap();

            assert_eq!(plan.edits[0].new_state, double);
        }
    }

    #[test]
    fn double_slab_rejects_same_cell_replacement() {
        let blocks = slab_and_stair_registry();
        let held = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let double = block_state(
            &blocks,
            "minecraft:oak_slab",
            &[("type", "double"), ("waterlogged", "false")],
        );

        assert!(!same_slab_can_replace(
            &blocks,
            held,
            double,
            Direction::Up,
            0.5
        ));
    }

    #[test]
    fn placed_stair_uses_vanilla_outer_corner_shape() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let north = BlockPos { z: 3, ..pos };
        let west_stair = block_state(
            &blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", "west"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(north, west_stair)]);
        let plan = plan_block_placement(
            &blocks,
            blocks
                .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
                .unwrap()
                .default,
            Some(&snapshot),
            pos,
            pose_with_yaw(180.0),
            Direction::Up,
            0.5,
            BlockStateId(0),
        )
        .unwrap();
        let expected = block_state(
            &blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "outer_left"),
                ("waterlogged", "false"),
            ],
        );

        assert_eq!(plan.edits[0].new_state, expected);
    }

    #[test]
    fn placement_updates_existing_stair_to_vanilla_inner_corner() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let north = BlockPos { z: 3, ..pos };
        let north_stair = blocks
            .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
            .unwrap()
            .default;
        let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(north, north_stair)]);
        let plan = plan_block_placement(
            &blocks,
            north_stair,
            Some(&snapshot),
            pos,
            pose_with_yaw(90.0),
            Direction::Up,
            0.5,
            BlockStateId(0),
        )
        .unwrap();
        let expected_neighbor = block_state(
            &blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "inner_left"),
                ("waterlogged", "false"),
            ],
        );

        assert!(
            plan.edits
                .iter()
                .any(|edit| { edit.pos == north && edit.new_state == expected_neighbor })
        );
    }

    #[test]
    fn stairblock_oracle_truth_table_covers_all_shapes_facings_and_halves() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let air = BlockStateId(0);

        for facing in [
            Direction::North,
            Direction::South,
            Direction::West,
            Direction::East,
        ] {
            for half in ["bottom", "top"] {
                for previous_shape in [
                    "straight",
                    "inner_left",
                    "inner_right",
                    "outer_left",
                    "outer_right",
                ] {
                    let center = stair_state(
                        &blocks,
                        direction_name(facing),
                        half,
                        previous_shape,
                        "true",
                    );
                    for (expected_shape, neighbor_pos, neighbor_facing) in [
                        ("straight", None, None),
                        (
                            "outer_left",
                            Some(relative(pos, facing)),
                            Some(counter_clockwise(facing).expect("stair direction is horizontal")),
                        ),
                        (
                            "outer_right",
                            Some(relative(pos, facing)),
                            Some(clockwise(facing)),
                        ),
                        (
                            "inner_left",
                            Some(relative(pos, opposite(facing))),
                            Some(counter_clockwise(facing).expect("stair direction is horizontal")),
                        ),
                        (
                            "inner_right",
                            Some(relative(pos, opposite(facing))),
                            Some(clockwise(facing)),
                        ),
                    ] {
                        let mut states = vec![(pos, center)];
                        if let (Some(neighbor_pos), Some(neighbor_facing)) =
                            (neighbor_pos, neighbor_facing)
                        {
                            states.push((
                                neighbor_pos,
                                stair_state(
                                    &blocks,
                                    direction_name(neighbor_facing),
                                    half,
                                    "straight",
                                    "false",
                                ),
                            ));
                        }
                        let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &states);
                        let updated = stair_state_with_shape(
                            &blocks,
                            &snapshot,
                            pos,
                            center,
                            BlockPos {
                                y: pos.y + 8,
                                ..pos
                            },
                            air,
                        )
                        .expect("all oracle-table states are canonical stairs");
                        assert_eq!(
                            updated,
                            stair_state(
                                &blocks,
                                direction_name(facing),
                                half,
                                expected_shape,
                                "true",
                            ),
                            "facing={facing:?}, half={half}, prior_shape={previous_shape}, expected={expected_shape}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn stairblock_is_different_stairs_matches_half_nonstair_parallel_and_perpendicular_cases() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let center = stair_state(&blocks, "north", "bottom", "straight", "false");
        let behind = relative(pos, Direction::North);
        let guard = relative(pos, Direction::East);
        let west_bottom = stair_state(&blocks, "west", "bottom", "straight", "false");
        let west_top = stair_state(&blocks, "west", "top", "straight", "false");
        let north_bottom = stair_state(&blocks, "north", "bottom", "straight", "false");
        let expected_outer_left = "outer_left";

        for (name, neighbor, guard_state, expected) in [
            ("perpendicular", west_bottom, None, expected_outer_left),
            ("mixed_half", west_top, None, "straight"),
            ("parallel", north_bottom, None, "straight"),
            (
                "is_different_stairs_guard",
                west_bottom,
                Some(north_bottom),
                "straight",
            ),
        ] {
            let mut states = vec![(pos, center), (behind, neighbor)];
            if let Some(guard_state) = guard_state {
                states.push((guard, guard_state));
            }
            let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &states);
            assert_eq!(
                stairs_shape(
                    &blocks,
                    &snapshot,
                    pos,
                    center,
                    BlockPos {
                        y: pos.y + 8,
                        ..pos
                    },
                    BlockStateId(0),
                ),
                Some(expected),
                "{name}"
            );
        }

        let snapshot = placement_snapshot_for_test(
            Arc::clone(&blocks),
            &[
                (pos, center),
                (
                    behind,
                    blocks
                        .block(&Identifier::parse("minecraft:stone").unwrap())
                        .unwrap()
                        .default,
                ),
            ],
        );
        assert_eq!(
            stairs_shape(
                &blocks,
                &snapshot,
                pos,
                center,
                BlockPos {
                    y: pos.y + 8,
                    ..pos
                },
                BlockStateId(0),
            ),
            Some("straight"),
            "non-stair neighbor"
        );
    }

    #[test]
    fn malformed_or_unloaded_stair_dependencies_fail_closed() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let placed = blocks
            .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
            .unwrap()
            .default;
        let malformed = blocks
            .block(&Identifier::parse("minecraft:malformed_stairs").unwrap())
            .unwrap()
            .default;

        let malformed_snapshot = placement_snapshot_for_test(
            Arc::clone(&blocks),
            &[
                (pos, BlockStateId(0)),
                (BlockPos { x: 3, ..pos }, malformed),
            ],
        );
        assert!(
            plan_block_placement(
                &blocks,
                placed,
                Some(&malformed_snapshot),
                pos,
                pose_with_yaw(180.0),
                Direction::Up,
                0.5,
                BlockStateId(0),
            )
            .is_none(),
            "malformed stair neighbor cannot be interpreted as non-stair"
        );

        let boundary_pos = BlockPos { x: 15, ..pos };
        let unloaded_snapshot =
            placement_snapshot_for_test(Arc::clone(&blocks), &[(boundary_pos, BlockStateId(0))]);
        assert!(
            plan_block_placement(
                &blocks,
                placed,
                Some(&unloaded_snapshot),
                boundary_pos,
                pose_with_yaw(180.0),
                Direction::Up,
                0.5,
                BlockStateId(0),
            )
            .is_none(),
            "the missing east chunk is a required shape dependency"
        );
    }

    #[test]
    fn straight_stair_plan_is_a_noop_for_neighbours_and_preserves_properties() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let placed = stair_state(&blocks, "north", "bottom", "inner_left", "true");
        let snapshot = placement_snapshot_for_test(Arc::clone(&blocks), &[(pos, BlockStateId(0))]);
        let plan = plan_block_placement(
            &blocks,
            placed,
            Some(&snapshot),
            pos,
            pose_with_yaw(180.0),
            Direction::Up,
            0.5,
            BlockStateId(0),
        )
        .expect("unrelated old shape does not prevent placement");

        assert_eq!(plan.edits.len(), 1);
        assert_eq!(
            plan.edits[0].new_state,
            stair_state(&blocks, "north", "bottom", "straight", "false")
        );
    }

    #[test]
    fn ordinary_non_stair_placement_recomputes_adjacent_stair_without_mutating_it() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let north = BlockPos { z: 3, ..pos };
        let existing = stair_state(&blocks, "north", "bottom", "straight", "true");
        let stone = blocks
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .default;
        let snapshot = placement_snapshot_for_test(
            Arc::clone(&blocks),
            &[(pos, BlockStateId(0)), (north, existing)],
        );

        let plan = plan_block_placement(
            &blocks,
            stone,
            Some(&snapshot),
            pos,
            pose_with_yaw(0.0),
            Direction::Up,
            0.5,
            BlockStateId(0),
        )
        .expect("ordinary placement beside a valid stair");

        assert_eq!(
            plan.edits,
            vec![BlockEdit {
                pos,
                new_state: stone,
            }]
        );
        assert!(
            plan.additional_preconditions
                .iter()
                .any(|precondition| precondition.pos == north),
            "the unchanged stair is still an updateShape dependency"
        );
    }

    #[test]
    fn stale_stair_shape_neighbor_rejects_the_entire_edit_batch() {
        let blocks = slab_and_stair_registry();
        let pos = BlockPos { x: 4, y: 64, z: 4 };
        let north = BlockPos { z: 3, ..pos };
        let east = BlockPos { x: 5, ..pos };
        let west_stair = block_state(
            &blocks,
            "minecraft:oak_stairs",
            &[
                ("facing", "west"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        );
        let placed_state = blocks
            .block(&Identifier::parse("minecraft:oak_stairs").unwrap())
            .unwrap()
            .default;

        assert!(conditional_placement_rejects_test_mutation(
            Arc::clone(&blocks),
            &[(pos, BlockStateId(0)), (north, west_stair)],
            pos,
            (east, BlockStateId(47)),
            |snapshot| plan_block_placement(
                &blocks,
                placed_state,
                Some(snapshot),
                pos,
                pose_with_yaw(180.0),
                Direction::Up,
                0.5,
                BlockStateId(0),
            ),
        ));
    }

    #[test]
    fn snapshot_positions_preserve_each_placement_category() {
        let blocks = BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:oak_sign"),
            simple_block(2, "minecraft:oak_wall_sign"),
            simple_block(3, "minecraft:oak_door"),
            simple_block(4, "minecraft:stone"),
            simple_block(5, "minecraft:cactus"),
        ])
        .unwrap();
        let pos = BlockPos {
            x: 10,
            y: 64,
            z: -4,
        };
        let below = BlockPos { y: 63, ..pos };
        let ordinary = vec![
            pos,
            below,
            BlockPos { x: 11, ..pos },
            BlockPos { x: 11, ..below },
            BlockPos { x: 9, ..pos },
            BlockPos { x: 9, ..below },
            BlockPos { z: -3, ..pos },
            BlockPos { z: -3, ..below },
            BlockPos { z: -5, ..pos },
            BlockPos { z: -5, ..below },
            BlockPos { x: 8, ..pos },
            BlockPos { x: 9, z: -5, ..pos },
            BlockPos { x: 9, z: -3, ..pos },
            BlockPos { z: -6, ..pos },
            BlockPos { z: -2, ..pos },
            BlockPos {
                x: 11,
                z: -5,
                ..pos
            },
            BlockPos {
                x: 11,
                z: -3,
                ..pos
            },
            BlockPos { x: 12, ..pos },
        ];
        let cases = [
            (BlockStateId(1), Vec::new()),
            (BlockStateId(2), Vec::new()),
            (
                BlockStateId(3),
                std::iter::once(BlockPos { y: 65, ..pos })
                    .chain(stair_shape_dependency_positions(pos))
                    .collect(),
            ),
            (BlockStateId(4), ordinary.clone()),
            (BlockStateId(5), ordinary),
        ];

        for (state, expected) in cases {
            assert_eq!(
                placement_snapshot_positions(&blocks, state, pos),
                Some(expected)
            );
        }
    }
}
