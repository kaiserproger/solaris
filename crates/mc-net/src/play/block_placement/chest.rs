use super::{
    BlockEdit, BlockEditPrecondition, BlockPos, BlockRegistry, BlockState, BlockStateId, Direction,
    PlannedBlockPlacement, PlayerPose, WorldReadSnapshot, horizontal_direction,
    horizontal_facing_from_yaw, property, relative, set_prop_if_present,
};

pub(super) fn is_chest(state: &BlockState) -> bool {
    state.block.id.namespace() == "minecraft"
        && matches!(state.block.id.path(), "chest" | "trapped_chest")
}

fn clockwise(direction: Direction) -> Direction {
    match direction {
        Direction::North => Direction::East,
        Direction::East => Direction::South,
        Direction::South => Direction::West,
        Direction::West => Direction::North,
        _ => direction,
    }
}

fn opposite(direction: Direction) -> Direction {
    let direction = clockwise(direction);
    clockwise(direction)
}

fn state_with(
    blocks: &BlockRegistry,
    state: &BlockState,
    facing: &str,
    kind: &str,
) -> Option<BlockStateId> {
    let mut properties = state.properties.clone();
    set_prop_if_present(&mut properties, "facing", facing);
    set_prop_if_present(&mut properties, "type", kind);
    blocks.by_name_and_props(&state.block.id, &properties)
}

fn facing_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::East => "east",
        Direction::South => "south",
        Direction::West => "west",
        _ => unreachable!("chest facing is horizontal"),
    }
}

pub(super) fn placement(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pos: BlockPos,
    state: &BlockState,
    pose: PlayerPose,
    clicked_face: Direction,
) -> Option<PlannedBlockPlacement> {
    let mut facing = opposite(horizontal_direction(horizontal_facing_from_yaw(pose.yaw))?);
    let mut partner = None;
    let mut dependencies = Vec::with_capacity(2);
    let candidates = if pose.shifting {
        [
            super::is_horizontal(clicked_face).then(|| opposite(clicked_face)),
            None,
        ]
    } else {
        [Some(clockwise(facing)), Some(opposite(clockwise(facing)))]
    };
    for direction in candidates.into_iter().flatten() {
        let neighbor_pos = relative(pos, direction);
        let neighbor_id = snapshot.get_cached_block(neighbor_pos)?;
        dependencies.push(BlockEditPrecondition {
            pos: neighbor_pos,
            expected_state: neighbor_id,
            expected_token: snapshot.block_mutation_token(neighbor_pos)?,
        });
        let neighbor = blocks.by_id(neighbor_id)?;
        if neighbor.block.id != state.block.id || property(neighbor, "type") != Some("single") {
            continue;
        }
        let neighbor_facing = horizontal_direction(property(neighbor, "facing")?)?;
        if pose.shifting {
            if direction != clockwise(neighbor_facing)
                && direction != opposite(clockwise(neighbor_facing))
            {
                continue;
            }
            facing = neighbor_facing;
        } else if neighbor_facing != facing {
            continue;
        }
        let kind = if direction == clockwise(facing) {
            "left"
        } else {
            "right"
        };
        partner = Some((neighbor_pos, neighbor, kind));
        break;
    }
    let kind = partner.as_ref().map_or("single", |(_, _, kind)| *kind);
    let mut edits = Vec::with_capacity(1 + usize::from(partner.is_some()));
    edits.push(BlockEdit {
        pos,
        new_state: state_with(blocks, state, facing_name(facing), kind)?,
    });
    if let Some((pos, neighbor, kind)) = partner {
        edits.push(BlockEdit {
            pos,
            new_state: state_with(
                blocks,
                neighbor,
                facing_name(facing),
                if kind == "left" { "right" } else { "left" },
            )?,
        });
    }
    Some(PlannedBlockPlacement {
        edits,
        additional_preconditions: dependencies,
    })
}

pub(in crate::play) fn connected_position(pos: BlockPos, state: &BlockState) -> Option<BlockPos> {
    if !is_chest(state) {
        return None;
    }
    let facing = horizontal_direction(property(state, "facing")?)?;
    let direction = match property(state, "type")? {
        "left" => clockwise(facing),
        "right" => opposite(clockwise(facing)),
        _ => return None,
    };
    Some(relative(pos, direction))
}

pub(in crate::play) fn paired_position(
    blocks: &BlockRegistry,
    block_at: impl Fn(BlockPos) -> Option<BlockStateId>,
    pos: BlockPos,
    current: BlockStateId,
) -> Option<BlockPos> {
    let current = blocks.by_id(current)?;
    let partner_pos = connected_position(pos, current)?;
    let partner = blocks.by_id(block_at(partner_pos)?)?;
    if partner.block.id != current.block.id
        || connected_position(partner_pos, partner) != Some(pos)
        || property(partner, "facing") != property(current, "facing")
    {
        return None;
    }
    Some(partner_pos)
}

pub(in crate::play) fn reset_partner(
    blocks: &BlockRegistry,
    block_at: impl Fn(BlockPos) -> Option<BlockStateId>,
    pos: BlockPos,
    previous: BlockStateId,
) -> Option<BlockEdit> {
    let partner_pos = paired_position(blocks, &block_at, pos, previous)?;
    let partner = blocks.by_id(block_at(partner_pos)?)?;
    Some(BlockEdit {
        pos: partner_pos,
        new_state: state_with(blocks, partner, property(partner, "facing")?, "single")?,
    })
}
