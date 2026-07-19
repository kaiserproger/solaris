use mc_data::block_facts::{BlockFactsTable, FluidKind};
use mc_protocol::packets::play::MovePlayerFlags;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, WorldReadSnapshot};
use tracing::debug;

use crate::error::ConnectionError;

use super::PlayerPose;
use super::campfire::{is_campfire_block, is_lit_campfire_block};
use super::chunk_stream::passable_block_name;
use super::survival::SurvivalState;

const PLAYER_HORIZONTAL_COORDINATE_LIMIT: f64 = 30_000_000.0;
const PLAYER_VERTICAL_COORDINATE_LIMIT: f64 = 20_000_000.0;

#[derive(Debug, Clone, Copy)]
pub(super) struct AcceptedAbsoluteMovement {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) z: f64,
    pub(super) yaw_pitch: Option<(f32, f32)>,
    pub(super) flags: MovePlayerFlags,
}

pub(super) fn validate_player_rotation(yaw: f32, pitch: f32) -> Result<(), ConnectionError> {
    if yaw.is_finite() && pitch.is_finite() {
        Ok(())
    } else {
        Err(ConnectionError::InvalidPlayerMovement)
    }
}

pub(super) fn clamp_player_coordinates(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    (
        x.clamp(
            -PLAYER_HORIZONTAL_COORDINATE_LIMIT,
            PLAYER_HORIZONTAL_COORDINATE_LIMIT,
        ),
        y.clamp(
            -PLAYER_VERTICAL_COORDINATE_LIMIT,
            PLAYER_VERTICAL_COORDINATE_LIMIT,
        ),
        z.clamp(
            -PLAYER_HORIZONTAL_COORDINATE_LIMIT,
            PLAYER_HORIZONTAL_COORDINATE_LIMIT,
        ),
    )
}

pub(super) fn clamp_player_pose(mut pose: PlayerPose) -> PlayerPose {
    (pose.x, pose.y, pose.z) = clamp_player_coordinates(pose.x, pose.y, pose.z);
    pose
}

pub(super) fn normalize_absolute_player_movement(
    mut movement: AcceptedAbsoluteMovement,
) -> Result<AcceptedAbsoluteMovement, ConnectionError> {
    if !movement.x.is_finite() || !movement.y.is_finite() || !movement.z.is_finite() {
        return Err(ConnectionError::InvalidPlayerMovement);
    }
    if let Some((yaw, pitch)) = movement.yaw_pitch {
        validate_player_rotation(yaw, pitch)?;
    }
    (movement.x, movement.y, movement.z) =
        clamp_player_coordinates(movement.x, movement.y, movement.z);
    Ok(movement)
}

pub(super) fn player_water_overlap_in_snapshot(
    facts: &BlockFactsTable,
    snapshot: &WorldReadSnapshot,
    pose: PlayerPose,
) -> (bool, bool) {
    let half_width = 0.3;
    let min_x = (pose.x - half_width).floor() as i32;
    let max_x = (pose.x + half_width).floor() as i32;
    let min_z = (pose.z - half_width).floor() as i32;
    let max_z = (pose.z + half_width).floor() as i32;
    let min_y = pose.y.floor() as i32;
    let max_y = (pose.y + 1.8).floor() as i32;
    let eye_pos = BlockPos {
        x: pose.x.floor() as i32,
        y: (pose.y + 1.62).floor() as i32,
        z: pose.z.floor() as i32,
    };
    let mut in_water = false;
    let mut eye_in_water = false;
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                let pos = BlockPos { x, y, z };
                let water = snapshot
                    .get_cached_block(pos)
                    .is_some_and(|state_id| state_is_water(facts, state_id));
                in_water |= water;
                eye_in_water |= water && pos == eye_pos;
            }
        }
    }
    (in_water, eye_in_water)
}

pub(super) fn refresh_player_fall_state(old_pose: PlayerPose, new_pose: &mut PlayerPose) {
    if new_pose.in_water || new_pose.flags.on_ground {
        new_pose.fall_start_y = new_pose.y;
    } else if old_pose.flags.on_ground || old_pose.in_water {
        new_pose.fall_start_y = old_pose.y.max(new_pose.y);
    } else {
        new_pose.fall_start_y = old_pose.fall_start_y.max(new_pose.y);
    }
}

pub(super) fn player_pose_collides_with_solid_in_snapshot(
    facts: &BlockFactsTable,
    blocks: &BlockRegistry,
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
                let block_pos = BlockPos { x, y, z };
                let collides = snapshot
                    .get_cached_block(block_pos)
                    .is_some_and(|state_id| {
                        player_collision_state_intersects(
                            facts, blocks, state_id, block_pos, pose, half_width,
                        )
                    });
                if collides {
                    return true;
                }
            }
        }
    }
    false
}

fn player_collision_state_intersects(
    facts: &BlockFactsTable,
    blocks: &BlockRegistry,
    state_id: BlockStateId,
    block_pos: BlockPos,
    pose: PlayerPose,
    player_half_width: f64,
) -> bool {
    if facts.fluid(state_id.0).is_some() {
        return false;
    }
    if is_campfire_block(blocks, state_id) {
        return false;
    }
    let Some(block_state) = blocks.by_id(state_id) else {
        return false;
    };
    let block_name = block_state.block.id.as_str();
    if passable_block_name(block_name) {
        return false;
    }

    let collision_height = if block_name == "minecraft:farmland" {
        15.0 / 16.0
    } else {
        1.0
    };
    let block_min_x = f64::from(block_pos.x);
    let block_min_y = f64::from(block_pos.y);
    let block_min_z = f64::from(block_pos.z);
    let block_max_x = block_min_x + 1.0;
    let block_max_y = block_min_y + collision_height;
    let block_max_z = block_min_z + 1.0;
    let epsilon = 1.0e-7;

    pose.x - player_half_width < block_max_x - epsilon
        && pose.x + player_half_width > block_min_x + epsilon
        && pose.y < block_max_y - epsilon
        && pose.y + 1.8 > block_min_y + epsilon
        && pose.z - player_half_width < block_max_z - epsilon
        && pose.z + player_half_width > block_min_z + epsilon
}

pub(super) fn player_touches_lit_campfire_in_snapshot(
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    player_pose: PlayerPose,
) -> bool {
    let half_width = 0.301;
    let min_x = (player_pose.x - half_width).floor() as i32;
    let max_x = (player_pose.x + half_width).floor() as i32;
    let foot_y = player_pose.y.floor() as i32;
    let min_z = (player_pose.z - half_width).floor() as i32;
    let max_z = (player_pose.z + half_width).floor() as i32;
    for x in min_x..=max_x {
        for y in [foot_y, foot_y - 1] {
            for z in min_z..=max_z {
                let touching = snapshot
                    .get_cached_block(BlockPos { x, y, z })
                    .is_some_and(|block_state| is_lit_campfire_block(blocks, block_state));
                if touching {
                    return true;
                }
            }
        }
    }
    false
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PendingTeleport {
    pub(super) id: i32,
    pub(super) sent_tick: u64,
}

impl PendingTeleport {
    pub(super) fn new(id: i32, sent_tick: u64) -> Self {
        Self { id, sent_tick }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TeleportConfirmResult {
    Confirmed,
    Mismatched { expected: i32 },
    Unexpected,
}

pub(super) fn confirm_pending_teleport(
    pending: &mut Option<PendingTeleport>,
    teleport_id: i32,
) -> TeleportConfirmResult {
    let Some(current) = *pending else {
        return TeleportConfirmResult::Unexpected;
    };
    if current.id != teleport_id {
        return TeleportConfirmResult::Mismatched {
            expected: current.id,
        };
    }
    *pending = None;
    TeleportConfirmResult::Confirmed
}

pub(super) fn guard_pending_teleport_movement(
    pending: &Option<PendingTeleport>,
    packet: &'static str,
) -> bool {
    let Some(current) = pending else {
        return false;
    };
    debug!(
        teleport_id = current.id,
        packet, "movement ignored until matching teleport confirmation"
    );
    true
}

pub(super) fn next_player_teleport_id(next_teleport_id: &mut i32) -> i32 {
    let teleport_id = (*next_teleport_id).max(1);
    *next_teleport_id = if teleport_id == i32::MAX {
        1
    } else {
        teleport_id + 1
    };
    teleport_id
}

fn state_is_water(facts: &BlockFactsTable, state_id: BlockStateId) -> bool {
    facts
        .fluid(state_id.0)
        .is_some_and(|fluid| fluid.kind == FluidKind::Water)
}

pub(super) fn farmland_trample_pos(old_pose: PlayerPose, new_pose: PlayerPose) -> Option<BlockPos> {
    if old_pose.in_water || new_pose.in_water {
        return None;
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground || old_pose.y - new_pose.y <= 0.5 {
        return None;
    }
    Some(BlockPos {
        x: new_pose.x.floor() as i32,
        y: (new_pose.y - 0.01).floor() as i32,
        z: new_pose.z.floor() as i32,
    })
}

pub(super) fn fall_damage_amount(old_pose: PlayerPose, new_pose: PlayerPose) -> f32 {
    if old_pose.in_water || new_pose.in_water {
        return 0.0;
    }
    if old_pose.flags.on_ground || !new_pose.flags.on_ground {
        return 0.0;
    }
    ((old_pose.fall_start_y - new_pose.y).max(0.0) - 3.0)
        .floor()
        .max(0.0) as f32
}

pub(super) fn movement_exhaustion(old_pose: PlayerPose, new_pose: PlayerPose) -> f32 {
    let dx = new_pose.x - old_pose.x;
    let dz = new_pose.z - old_pose.z;
    let horizontal_distance = dx.hypot(dz);
    let mut exhaustion = 0.0;
    if new_pose.sprinting && horizontal_distance > 0.0 {
        exhaustion += horizontal_distance as f32 * SurvivalState::SPRINT_EXHAUSTION_PER_METER;
    }
    if new_pose.input.jump
        && old_pose.flags.on_ground
        && !new_pose.flags.on_ground
        && new_pose.y > old_pose.y
    {
        exhaustion += if new_pose.sprinting {
            SurvivalState::SPRINT_JUMP_EXHAUSTION
        } else {
            SurvivalState::JUMP_EXHAUSTION
        };
    }
    exhaustion
}
