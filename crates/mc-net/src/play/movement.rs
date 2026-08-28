use std::collections::HashSet;
use std::sync::Arc;

use mc_data::block_facts::{BlockFactsTable, FluidKind};
use mc_data::collision_shapes::{CollisionShapeTable, vanilla_collision_shapes};
use mc_domain::GameMode;
use mc_protocol::packets::play::MovePlayerFlags;
use mc_world::{BlockPos, BlockRegistry, BlockStateId, ChunkPos, WorldReadSnapshot, WorldReadView};
use tracing::debug;

use crate::error::ConnectionError;

use super::PlayerPose;
use super::campfire::{is_campfire_block, is_lit_campfire_block};
use mc_data::block_semantics_26_1_2::passable_block_name;

const PLAYER_HORIZONTAL_COORDINATE_LIMIT: f64 = 30_000_000.0;
const PLAYER_VERTICAL_COORDINATE_LIMIT: f64 = 20_000_000.0;
const COLLISION_DEFLATION: f64 = 1.0e-5_f32 as f64;
const POWDER_SNOW_FALLING_TOP: f64 = 0.9_f32 as f64;
const POWDER_SNOW_FALL_DISTANCE: f64 = 2.5;
const PLAYER_BODY_HALF_WIDTH: f64 = 0.3;
const PLAYER_SURVIVAL_MOVEMENT_LIMIT: f64 = 10.0;
const PLAYER_FLYING_MOVEMENT_LIMIT: f64 = 16.0;
const PLAYER_SWEEP_SAMPLE_STEP: f64 = 1.0 / 32.0;
const PLAYER_EMBEDDED_ESCAPE_LIMIT: f64 = 0.5;
const PLAYER_MOVEMENT_MAX_CHUNKS: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlayerPoseCommitKind {
    Movement,
    Teleport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerMovementRejection {
    InvalidPose,
    Displacement,
    DestinationUnloaded,
    DestinationOutsideWorld,
    SweptCollision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlayerMovementAuthorityError {
    Rejected(PlayerMovementRejection),
    WorldUnavailable,
}

#[derive(Clone)]
pub(super) struct PlayerMovementAuthorityResources {
    world_read: WorldReadView,
    blocks: Arc<BlockRegistry>,
    block_facts: Arc<BlockFactsTable>,
}

impl std::fmt::Debug for PlayerMovementAuthorityResources {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlayerMovementAuthorityResources")
            .finish_non_exhaustive()
    }
}

impl PlayerMovementAuthorityResources {
    pub(super) fn new(
        world_read: WorldReadView,
        blocks: Arc<BlockRegistry>,
        block_facts: Arc<BlockFactsTable>,
    ) -> Self {
        Self {
            world_read,
            blocks,
            block_facts,
        }
    }

    pub(super) fn validate_movement(
        &self,
        loaded_chunks: &HashSet<(i32, i32)>,
        old_pose: PlayerPose,
        new_pose: PlayerPose,
        game_mode: GameMode,
        walks_on_powder_snow: bool,
    ) -> Result<(), PlayerMovementAuthorityError> {
        if !valid_authoritative_pose(new_pose) {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::InvalidPose,
            ));
        }

        let delta = player_pose_delta(old_pose, new_pose);
        let movement_limit = match game_mode {
            GameMode::Creative | GameMode::Spectator => PLAYER_FLYING_MOVEMENT_LIMIT,
            GameMode::Survival | GameMode::Adventure => PLAYER_SURVIVAL_MOVEMENT_LIMIT,
        };
        let displacement = mc_physics::Vec3::new(delta.0, delta.1, delta.2);
        if !mc_physics::displacement_within_limit(displacement, movement_limit) {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::Displacement,
            ));
        }

        // Rotation/flags-only updates do not need to re-prove world residency for an
        // already authoritative position. A shrinking stance transition is safe for
        // the same reason; expanding the collision volume still requires a snapshot.
        if displacement == mc_physics::Vec3::ZERO
            && new_pose.body_height() <= old_pose.body_height()
        {
            return Ok(());
        }

        let Some(destination_chunks) = player_body_chunks(new_pose) else {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::InvalidPose,
            ));
        };
        if destination_chunks
            .iter()
            .any(|chunk| !loaded_chunks.contains(&(chunk.x, chunk.z)))
        {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::DestinationUnloaded,
            ));
        }

        let Some(swept_chunks) = swept_player_chunks(old_pose, new_pose) else {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::Displacement,
            ));
        };
        let snapshot = self.world_read.snapshot_chunks(&swept_chunks);
        if swept_chunks
            .iter()
            .any(|chunk| !snapshot.contains_chunk(*chunk))
        {
            return Err(PlayerMovementAuthorityError::WorldUnavailable);
        }
        for chunk in &destination_chunks {
            let Some(chunk) = snapshot.chunk(*chunk) else {
                return Err(PlayerMovementAuthorityError::WorldUnavailable);
            };
            let geometry = chunk.geometry();
            if new_pose.y < f64::from(geometry.min_y())
                || new_pose.y + new_pose.body_height() > f64::from(geometry.max_y())
            {
                return Err(PlayerMovementAuthorityError::Rejected(
                    PlayerMovementRejection::DestinationOutsideWorld,
                ));
            }
        }

        let context = PlayerCollisionContext::from_pose(old_pose, walks_on_powder_snow);
        let old_collides = player_pose_collides_with_solid_in_snapshot_with_context(
            &self.block_facts,
            &self.blocks,
            &snapshot,
            old_pose,
            context,
        );
        if old_collides
            && mc_physics::displacement_within_limit(displacement, PLAYER_EMBEDDED_ESCAPE_LIMIT)
            && !player_pose_collides_with_solid_in_snapshot_with_context(
                &self.block_facts,
                &self.blocks,
                &snapshot,
                new_pose,
                context,
            )
        {
            return Ok(());
        }

        let Some(steps) = mc_physics::sweep_sample_count(displacement, PLAYER_SWEEP_SAMPLE_STEP)
        else {
            return Err(PlayerMovementAuthorityError::Rejected(
                PlayerMovementRejection::Displacement,
            ));
        };
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let mut sample = new_pose;
            sample.x = old_pose.x + delta.0 * t;
            sample.y = old_pose.y + delta.1 * t;
            sample.z = old_pose.z + delta.2 * t;
            if player_pose_collides_with_solid_in_snapshot_with_context(
                &self.block_facts,
                &self.blocks,
                &snapshot,
                sample,
                context,
            ) {
                return Err(PlayerMovementAuthorityError::Rejected(
                    PlayerMovementRejection::SweptCollision,
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn valid_authoritative_pose(pose: PlayerPose) -> bool {
    mc_physics::authoritative_pose_within_limits(
        mc_physics::Vec3::new(pose.x, pose.y, pose.z),
        pose.yaw,
        pose.pitch,
        PLAYER_HORIZONTAL_COORDINATE_LIMIT,
        PLAYER_VERTICAL_COORDINATE_LIMIT,
    )
}

fn player_pose_delta(old_pose: PlayerPose, new_pose: PlayerPose) -> (f64, f64, f64) {
    (
        new_pose.x - old_pose.x,
        new_pose.y - old_pose.y,
        new_pose.z - old_pose.z,
    )
}

fn player_body_chunks(pose: PlayerPose) -> Option<Vec<ChunkPos>> {
    mc_world::chunk_rectangle_for_world_bounds(
        pose.x - PLAYER_BODY_HALF_WIDTH,
        pose.x + PLAYER_BODY_HALF_WIDTH,
        pose.z - PLAYER_BODY_HALF_WIDTH,
        pose.z + PLAYER_BODY_HALF_WIDTH,
        PLAYER_MOVEMENT_MAX_CHUNKS,
    )
}

fn swept_player_chunks(old_pose: PlayerPose, new_pose: PlayerPose) -> Option<Vec<ChunkPos>> {
    mc_world::chunk_rectangle_for_world_bounds(
        old_pose.x.min(new_pose.x) - PLAYER_BODY_HALF_WIDTH,
        old_pose.x.max(new_pose.x) + PLAYER_BODY_HALF_WIDTH,
        old_pose.z.min(new_pose.z) - PLAYER_BODY_HALF_WIDTH,
        old_pose.z.max(new_pose.z) + PLAYER_BODY_HALF_WIDTH,
        PLAYER_MOVEMENT_MAX_CHUNKS,
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PlayerCollisionContext {
    entity_bottom: f64,
    fall_distance: f64,
    descending: bool,
    walks_on_powder_snow: bool,
}

impl PlayerCollisionContext {
    pub(super) fn from_pose(pose: PlayerPose, walks_on_powder_snow: bool) -> Self {
        Self {
            entity_bottom: pose.y,
            fall_distance: (pose.fall_start_y - pose.y).max(0.0),
            descending: pose.shifting,
            walks_on_powder_snow,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AcceptedAbsoluteMovement {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) z: f64,
    pub(super) yaw_pitch: Option<(f32, f32)>,
    pub(super) flags: MovePlayerFlags,
}

pub(super) fn validate_player_rotation(yaw: f32, pitch: f32) -> Result<(), ConnectionError> {
    mc_physics::rotation_is_finite(yaw, pitch)
        .then_some(())
        .ok_or(ConnectionError::InvalidPlayerMovement)
}

pub(super) fn clamp_player_coordinates(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let input = mc_physics::Vec3::new(x, y, z);
    let position = mc_physics::clamp_world_position(
        input,
        PLAYER_HORIZONTAL_COORDINATE_LIMIT,
        PLAYER_VERTICAL_COORDINATE_LIMIT,
    )
    .unwrap_or(input);
    (position.x, position.y, position.z)
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
    let max_y = (pose.y + pose.body_height()).floor() as i32;
    let eye_pos = BlockPos {
        x: pose.x.floor() as i32,
        y: (pose.y + pose.eye_height()).floor() as i32,
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
    if let Some(fall_start_y) = mc_physics::next_fall_start_y(
        old_pose.y,
        old_pose.fall_start_y,
        old_pose.flags.on_ground,
        old_pose.in_water,
        new_pose.y,
        new_pose.flags.on_ground,
        new_pose.in_water,
    ) {
        new_pose.fall_start_y = fall_start_y;
    }
}

pub(super) fn player_pose_collides_with_solid_in_snapshot(
    facts: &BlockFactsTable,
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pose: PlayerPose,
) -> bool {
    player_pose_collides_with_solid_in_snapshot_with_context(
        facts,
        blocks,
        snapshot,
        pose,
        PlayerCollisionContext::from_pose(pose, false),
    )
}

pub(super) fn player_pose_collides_with_solid_in_snapshot_with_context(
    facts: &BlockFactsTable,
    blocks: &BlockRegistry,
    snapshot: &WorldReadSnapshot,
    pose: PlayerPose,
    context: PlayerCollisionContext,
) -> bool {
    let collision_shapes = vanilla_collision_shapes();
    let half_width = 0.3;
    let min_x = (pose.x - half_width).floor() as i32;
    let max_x = (pose.x + half_width).floor() as i32;
    let max_collision_box_y = collision_shapes.max_box_y_blocks();
    let min_y = (pose.y - max_collision_box_y + COLLISION_DEFLATION).floor() as i32 + 1;
    let max_y = (pose.y + pose.body_height() - 1.0e-6).floor() as i32;
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
                            facts,
                            blocks,
                            collision_shapes,
                            state_id,
                            block_pos,
                            pose,
                            context,
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
    collision_shapes: &CollisionShapeTable,
    state_id: BlockStateId,
    block_pos: BlockPos,
    pose: PlayerPose,
    context: PlayerCollisionContext,
) -> bool {
    if facts.fluid(state_id.0).is_some() {
        return false;
    }
    let Some(block_state) = blocks.by_id(state_id) else {
        return false;
    };
    let block_name = block_state.block.id.as_str();

    let block_min_x = f64::from(block_pos.x);
    let block_min_y = f64::from(block_pos.y);
    let block_min_z = f64::from(block_pos.z);
    let player_half_width = 0.3;
    let body = [
        pose.x - player_half_width,
        pose.y,
        pose.z - player_half_width,
        pose.x + player_half_width,
        pose.y + pose.body_height(),
        pose.z + player_half_width,
    ];
    let intersects = |[min_x, min_y, min_z, max_x, max_y, max_z]: [f64; 6]| {
        mc_physics::aabb_intersects_deflated_obstacle(
            body,
            [
                block_min_x + min_x,
                block_min_y + min_y,
                block_min_z + min_z,
                block_min_x + max_x,
                block_min_y + max_y,
                block_min_z + max_z,
            ],
            COLLISION_DEFLATION,
        )
    };

    let exact_shape = collision_shapes.get_for_state(
        state_id.0,
        &block_state.block.id,
        block_state.properties.as_slice(),
    );
    if block_name == "minecraft:powder_snow" && exact_shape.is_some() {
        if context.fall_distance > POWDER_SNOW_FALL_DISTANCE {
            return intersects([0.0, 0.0, 0.0, 1.0, POWDER_SNOW_FALLING_TOP, 1.0]);
        }
        let is_above = context.entity_bottom > block_min_y + 1.0 - COLLISION_DEFLATION;
        return context.walks_on_powder_snow
            && is_above
            && !context.descending
            && intersects([0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
    }

    if let Some(boxes) = exact_shape {
        return boxes
            .iter()
            .any(|collision_box| intersects(collision_box.as_blocks()));
    }

    // Custom or reduced registries have no vanilla table identity. Preserve the
    // known semantics for those fixtures instead of turning plants into cubes.
    if is_campfire_block(blocks, state_id)
        || (block_name != "minecraft:powder_snow" && passable_block_name(block_name))
    {
        return false;
    }

    let fallback_box = if collision_shapes
        .is_exact_farmland_state(&block_state.block.id, block_state.properties.as_slice())
    {
        [0.0, 0.0, 0.0, 1.0, 15.0 / 16.0, 1.0]
    } else {
        [0.0, 0.0, 0.0, 1.0, 1.0, 1.0]
    };
    intersects(fallback_box)
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
        exhaustion += horizontal_distance as f32
            * mc_entity::player_survival_26_1_2::SPRINT_EXHAUSTION_PER_METER;
    }
    if new_pose.input.jump
        && old_pose.flags.on_ground
        && !new_pose.flags.on_ground
        && new_pose.y > old_pose.y
    {
        exhaustion += if new_pose.sprinting {
            mc_entity::player_survival_26_1_2::SPRINT_JUMP_EXHAUSTION
        } else {
            mc_entity::player_survival_26_1_2::JUMP_EXHAUSTION
        };
    }
    exhaustion
}
