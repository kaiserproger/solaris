use mc_data::block_facts::BlockFactsTable;
use mc_data::block_light::BlockLightTable;
use mc_domain::GameMode;
use mc_world::{BlockPos, BlockRegistry, ChunkPos, SECTION_DIM, WorldReadSnapshot, WorldReadView};

use super::campfire::is_campfire_block;
use super::chunk_stream::passable_block_name;
use super::movement::player_pose_collides_with_solid_in_snapshot;
use super::{
    BlockEdit, BlockEditPrecondition, DAY_LENGTH_TICKS, DAY_START_TICK, PlayerPose,
    block_state_property, sibling_state_with_property,
};

pub(super) fn plan_bed_occupied_edits(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    canonical_bed: BlockPos,
    occupied: bool,
) -> Option<(Vec<BlockEdit>, Vec<BlockEditPrecondition>)> {
    let head_chunk = chunk_pos(canonical_bed);
    let planning = world_read.snapshot_chunks(&[head_chunk]);
    let head = planning
        .get_cached_block(canonical_bed)
        .and_then(|state_id| blocks.by_id(state_id))?;
    if !is_bed_part(head, "head") {
        return None;
    }
    let facing = block_state_property(head, "facing")?;
    let (dx, dz) = horizontal_step(facing)?;
    let foot_pos = BlockPos {
        x: canonical_bed.x - dx,
        y: canonical_bed.y,
        z: canonical_bed.z - dz,
    };
    let snapshot = world_read.snapshot_chunks(&[head_chunk, chunk_pos(foot_pos)]);
    let head_id = snapshot.get_cached_block(canonical_bed)?;
    let head = blocks.by_id(head_id)?;
    if !is_bed_part(head, "head") || block_state_property(head, "facing") != Some(facing) {
        return None;
    }

    let foot_id = snapshot.get_cached_block(foot_pos)?;
    let foot = blocks.by_id(foot_id)?;
    if foot.block.id != head.block.id
        || !is_bed_part(foot, "foot")
        || block_state_property(foot, "facing") != Some(facing)
    {
        return None;
    }

    let desired = if occupied { "true" } else { "false" };
    let mut edits = Vec::with_capacity(2);
    let mut preconditions = Vec::with_capacity(2);
    for (position, expected_state) in [(canonical_bed, head_id), (foot_pos, foot_id)] {
        let current = blocks.by_id(expected_state)?;
        let new_state = sibling_state_with_property(blocks, current, "occupied", desired)?;
        preconditions.push(BlockEditPrecondition {
            pos: position,
            expected_state,
            expected_token: snapshot.block_mutation_token(position)?,
        });
        if new_state != expected_state {
            edits.push(BlockEdit {
                pos: position,
                new_state,
            });
        }
    }
    Some((edits, preconditions))
}

pub(super) fn bed_sleep_is_blocked_by_monster(game_mode: GameMode, hostile_nearby: bool) -> bool {
    game_mode != GameMode::Creative && hostile_nearby
}

pub(super) fn bed_sleep_is_obstructed(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    block_light: Option<&BlockLightTable>,
    canonical_bed: BlockPos,
) -> bool {
    let Some((head, foot)) = loaded_bed_halves(world_read, blocks, canonical_bed) else {
        return true;
    };
    let above_head = BlockPos {
        y: head.y + 1,
        ..head
    };
    let above_foot = BlockPos {
        y: foot.y + 1,
        ..foot
    };
    let overhead = snapshot_positions(world_read, &[above_head, above_foot]);

    [above_head, above_foot].into_iter().any(|pos| {
        let Some(state_id) = overhead.get_cached_block(pos) else {
            return true;
        };
        block_light
            .and_then(|facts| facts.suffocating(state_id.0))
            .unwrap_or(false)
    })
}

pub(super) fn plan_loaded_bed_interaction(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    pos: BlockPos,
) -> Option<(PlayerPose, BlockPos)> {
    let snapshot = snapshot_positions(world_read, &[pos]);
    let clicked = snapshot.get_cached_block(pos)?;
    let clicked_state = blocks.by_id(clicked)?;
    if !clicked_state.block.id.path().ends_with("_bed") {
        return None;
    }

    let canonical = canonical_bed_position(pos, clicked_state);
    if block_state_property(clicked_state, "part").is_some() {
        loaded_bed_halves(world_read, blocks, canonical)?;
    }
    let canonical_state = snapshot_positions(world_read, &[canonical])
        .get_cached_block(canonical)
        .and_then(|state_id| blocks.by_id(state_id))?;
    Some((bed_respawn_pose(canonical, canonical_state), canonical))
}

pub(in crate::play) fn next_morning_time(world_time: u64) -> u64 {
    let day = world_time / DAY_LENGTH_TICKS;
    day.saturating_add(1)
        .saturating_mul(DAY_LENGTH_TICKS)
        .saturating_add(DAY_START_TICK)
}

pub(super) fn bed_respawn_pose(pos: BlockPos, state: &mc_world::BlockState) -> PlayerPose {
    let mut pose = PlayerPose::new(
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 1.0,
        f64::from(pos.z) + 0.5,
    );
    pose.yaw = block_state_property(state, "facing")
        .map(yaw_for_horizontal_facing)
        .unwrap_or(0.0);
    pose
}

pub(super) fn canonical_bed_position(pos: BlockPos, state: &mc_world::BlockState) -> BlockPos {
    if block_state_property(state, "part") != Some("foot") {
        return pos;
    }
    let Some((dx, dz)) = block_state_property(state, "facing").and_then(horizontal_step) else {
        return pos;
    };
    BlockPos {
        x: pos.x + dx,
        y: pos.y,
        z: pos.z + dz,
    }
}

pub(super) fn safe_bed_wake_pose(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    block_facts: &BlockFactsTable,
    bed: BlockPos,
    sleeping_pose: PlayerPose,
) -> PlayerPose {
    let facing = snapshot_positions(world_read, &[bed])
        .get_cached_block(bed)
        .and_then(|state_id| blocks.by_id(state_id))
        .and_then(|bed_state| block_state_property(bed_state, "facing"))
        .and_then(horizontal_step)
        .unwrap_or((0, 1));
    let mut side = (-facing.1, facing.0);
    let yaw = sleeping_pose.yaw.to_radians();
    let view = (-yaw.sin(), yaw.cos());
    if f64::from(side.0) * f64::from(view.0) + f64::from(side.1) * f64::from(view.1) > 0.0 {
        side = (-side.0, -side.1);
    }

    for (dx, dz) in bed_wake_offsets(facing, side) {
        let above_bed = (dx, dz) == (0, 0) || (dx, dz) == (-facing.0, -facing.1);
        let Some(candidate_y) = (if above_bed {
            bed.y.checked_add(1)
        } else {
            Some(bed.y)
        }) else {
            continue;
        };
        let Some(support_y) = candidate_y.checked_sub(1) else {
            continue;
        };
        let support = BlockPos {
            x: bed.x + dx,
            y: support_y,
            z: bed.z + dz,
        };
        if !wake_position_has_support(world_read, blocks, block_facts, support) {
            continue;
        }
        let mut candidate = sleeping_pose;
        candidate.x = f64::from(support.x) + 0.5;
        candidate.y = f64::from(candidate_y);
        candidate.z = f64::from(support.z) + 0.5;
        candidate.flags.on_ground = true;
        candidate.fall_start_y = candidate.y;
        let body = player_body_snapshot(world_read, candidate);
        if !wake_body_contains_unsafe_cell(blocks, block_facts, &body, candidate)
            && !player_pose_collides_with_solid_in_snapshot(block_facts, blocks, &body, candidate)
        {
            candidate.yaw = yaw_toward_bed(candidate, bed);
            return candidate;
        }
    }

    let mut fallback = sleeping_pose;
    fallback.x = f64::from(bed.x) + 0.5;
    fallback.y = f64::from(bed.y) + 1.1;
    fallback.z = f64::from(bed.z) + 0.5;
    fallback.flags.on_ground = false;
    fallback.fall_start_y = fallback.y;
    fallback
}

fn loaded_bed_halves(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    canonical_bed: BlockPos,
) -> Option<(BlockPos, BlockPos)> {
    let head_snapshot = snapshot_positions(world_read, &[canonical_bed]);
    let head_id = head_snapshot.get_cached_block(canonical_bed)?;
    let head = blocks.by_id(head_id)?;
    if !is_bed_part(head, "head") {
        return None;
    }
    let facing = block_state_property(head, "facing")?;
    let (dx, dz) = horizontal_step(facing)?;
    let foot_pos = BlockPos {
        x: canonical_bed.x - dx,
        y: canonical_bed.y,
        z: canonical_bed.z - dz,
    };
    let snapshot = snapshot_positions(world_read, &[canonical_bed, foot_pos]);
    let current_head = snapshot
        .get_cached_block(canonical_bed)
        .and_then(|state_id| blocks.by_id(state_id))?;
    let foot = snapshot
        .get_cached_block(foot_pos)
        .and_then(|state_id| blocks.by_id(state_id))?;
    if current_head.block.id != head.block.id
        || !is_bed_part(current_head, "head")
        || block_state_property(current_head, "facing") != Some(facing)
        || foot.block.id != head.block.id
        || !is_bed_part(foot, "foot")
        || block_state_property(foot, "facing") != Some(facing)
    {
        return None;
    }
    Some((canonical_bed, foot_pos))
}

fn is_bed_part(state: &mc_world::BlockState, part: &str) -> bool {
    state.block.id.path().ends_with("_bed") && block_state_property(state, "part") == Some(part)
}

fn snapshot_positions(world_read: &WorldReadView, positions: &[BlockPos]) -> WorldReadSnapshot {
    let mut chunks = Vec::with_capacity(positions.len());
    for &position in positions {
        let chunk = chunk_pos(position);
        if !chunks.contains(&chunk) {
            chunks.push(chunk);
        }
    }
    world_read.snapshot_chunks(&chunks)
}

fn player_body_snapshot(world_read: &WorldReadView, pose: PlayerPose) -> WorldReadSnapshot {
    let half_width = 0.3;
    let min_cx = ((pose.x - half_width).floor() as i32).div_euclid(SECTION_DIM as i32);
    let max_cx = ((pose.x + half_width).floor() as i32).div_euclid(SECTION_DIM as i32);
    let min_cz = ((pose.z - half_width).floor() as i32).div_euclid(SECTION_DIM as i32);
    let max_cz = ((pose.z + half_width).floor() as i32).div_euclid(SECTION_DIM as i32);
    let mut chunks = Vec::with_capacity(4);
    for x in min_cx..=max_cx {
        for z in min_cz..=max_cz {
            chunks.push(ChunkPos { x, z });
        }
    }
    world_read.snapshot_chunks(&chunks)
}

fn wake_body_contains_unsafe_cell(
    blocks: &BlockRegistry,
    block_facts: &BlockFactsTable,
    snapshot: &WorldReadSnapshot,
    pose: PlayerPose,
) -> bool {
    let half_width = 0.3;
    let min_x = (pose.x - half_width).floor() as i32;
    let max_x = (pose.x + half_width).floor() as i32;
    let min_y = pose.y.floor() as i32;
    let max_y = (pose.y + 1.8 - 1.0e-6).floor() as i32;
    let min_z = (pose.z - half_width).floor() as i32;
    let max_z = (pose.z + half_width).floor() as i32;
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let Some(state_id) = snapshot.get_cached_block(BlockPos { x, y, z }) else {
                    return true;
                };
                if block_facts.fluid(state_id.0).is_some() || is_campfire_block(blocks, state_id) {
                    return true;
                }
            }
        }
    }
    false
}

fn chunk_pos(pos: BlockPos) -> ChunkPos {
    ChunkPos {
        x: pos.x.div_euclid(SECTION_DIM as i32),
        z: pos.z.div_euclid(SECTION_DIM as i32),
    }
}

fn horizontal_step(facing: &str) -> Option<(i32, i32)> {
    match facing {
        "north" => Some((0, -1)),
        "south" => Some((0, 1)),
        "west" => Some((-1, 0)),
        "east" => Some((1, 0)),
        _ => None,
    }
}

fn bed_wake_offsets(facing: (i32, i32), side: (i32, i32)) -> [(i32, i32); 12] {
    let (fx, fz) = facing;
    let (sx, sz) = side;
    [
        (sx, sz),
        (sx - fx, sz - fz),
        (sx - 2 * fx, sz - 2 * fz),
        (-2 * fx, -2 * fz),
        (-sx - 2 * fx, -sz - 2 * fz),
        (-sx - fx, -sz - fz),
        (-sx, -sz),
        (-sx + fx, -sz + fz),
        (fx, fz),
        (sx + fx, sz + fz),
        (0, 0),
        (-fx, -fz),
    ]
}

fn wake_position_has_support(
    world_read: &WorldReadView,
    blocks: &BlockRegistry,
    block_facts: &BlockFactsTable,
    support: BlockPos,
) -> bool {
    let snapshot = snapshot_positions(world_read, &[support]);
    let Some(state_id) = snapshot.get_cached_block(support) else {
        return false;
    };
    if block_facts.fluid(state_id.0).is_some() || is_campfire_block(blocks, state_id) {
        return false;
    }
    blocks
        .by_id(state_id)
        .is_some_and(|block| !passable_block_name(block.block.id.as_str()))
}

fn yaw_toward_bed(pose: PlayerPose, bed: BlockPos) -> f32 {
    let dx = f64::from(bed.x) + 0.5 - pose.x;
    let dz = f64::from(bed.z) + 0.5 - pose.z;
    (-dx).atan2(dz).to_degrees() as f32
}

fn yaw_for_horizontal_facing(facing: &str) -> f32 {
    match facing {
        "north" => 180.0,
        "south" => 0.0,
        "west" => 90.0,
        "east" => -90.0,
        _ => 0.0,
    }
}
