//! Bounded aquatic steering, using 26.1.2's FishMoveControl and Squid.aiStep.
//! Navigation is a local water-only approximation, not vanilla schooling/A*.
use crate::{
    EntityId, GoalPathingRequest, PathingBudget, PathingDecision, PathingDecisionKind,
    PathingProbe, PathingProbeResult, RetainedPathState, Rotation, Vec3,
};

/// Requires the entire navigation body to occupy known water, never lava, air,
/// or a solid/waterlogged collision block. Both owner and legacy probes share
/// this conservative bounded-navigation contract.
pub fn water_occupancy(
    position: Vec3,
    aabb: mc_physics::Aabb,
    mut sample: impl FnMut(i32, i32, i32) -> Result<mc_physics::BlockMaterial, PathingProbeResult>,
) -> PathingProbeResult {
    if !position.is_finite()
        || !aabb.half_width.is_finite()
        || !aabb.height.is_finite()
        || aabb.half_width <= 0.0
        || aabb.height <= 0.0
    {
        return PathingProbeResult::Blocked;
    }
    let min_x = (position.x - aabb.half_width + 1.0e-6).floor() as i32;
    let max_x = (position.x + aabb.half_width - 1.0e-6).floor() as i32;
    let min_y = (position.y + 1.0e-6).floor() as i32;
    let max_y = (position.y + aabb.height - 1.0e-6).floor() as i32;
    let min_z = (position.z - aabb.half_width + 1.0e-6).floor() as i32;
    let max_z = (position.z + aabb.half_width - 1.0e-6).floor() as i32;
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            for y in min_y..=max_y {
                match sample(x, y, z) {
                    Ok(mc_physics::BlockMaterial::Water) => {}
                    Ok(_) => return PathingProbeResult::Blocked,
                    Err(result) => return result,
                }
            }
        }
    }
    PathingProbeResult::Walkable
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Swimmer {
    Fish,
    Squid,
    Other,
}

impl Swimmer {
    pub(crate) fn for_type(name: &str) -> Self {
        match name {
            "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish" => Self::Fish,
            "minecraft:squid" | "minecraft:glow_squid" => Self::Squid,
            _ => Self::Other,
        }
    }
}

pub(crate) fn wander_target(
    id: EntityId,
    position: Vec3,
    path: RetainedPathState,
    tick: u64,
    period_ticks: u32,
    vertical_speed: f64,
    swimmer: Swimmer,
) -> (Vec3, u64) {
    let delta = difference(path.target, position);
    if swimmer != Swimmer::Squid && path.has_target && delta.horizontal_len().hypot(delta.y) > 0.5 {
        return (path.target, path.target_epoch.unwrap_or(0));
    }
    // Preserve a destination across ticks rather than replacing a velocity at
    // global period boundaries. The existing deterministic stream survives owner transfers.
    let epoch = path
        .target_epoch
        .map_or(tick / u64::from(period_ticks.max(1)), |n| n.wrapping_add(1));
    if swimmer == Swimmer::Squid {
        // SquidRandomMovementGoal is evaluated every other tick with
        // reducedTickDelay(50)=25. Retain its vector, not a fish destination.
        let reroll = tick.is_multiple_of(2) && crate::deterministic_unit(id, tick) < 1.0 / 25.0;
        if path.has_target && !reroll {
            let direction = difference(path.target, path.last_position);
            return (
                Vec3::new(
                    position.x + direction.x,
                    position.y + direction.y,
                    position.z + direction.z,
                ),
                path.target_epoch.unwrap_or(0),
            );
        }
        let angle = crate::deterministic_angle(id, tick);
        return (
            Vec3::new(
                position.x + angle.cos() * crate::WANDER_MIN_DISTANCE,
                position.y + crate::deterministic_unit(id, tick.wrapping_add(0x41)) * 2.0 - 1.0,
                position.z + angle.sin() * crate::WANDER_MIN_DISTANCE,
            ),
            tick,
        );
    }
    let angle = crate::deterministic_angle(id, epoch);
    let distance = crate::WANDER_MIN_DISTANCE
        + crate::deterministic_unit(id, epoch.wrapping_add(0x2d)) * crate::WANDER_DISTANCE_SPREAD;
    // RandomSwimmingGoal.getPosition uses horizontal 10 / vertical 7. Keep
    // the engine's smaller bounded horizontal search and the vanilla vertical range.
    let y = (crate::deterministic_unit(id, epoch.wrapping_add(0x41)) * 2.0 - 1.0)
        * if vertical_speed > 0.0 { 7.0 } else { 0.0 };
    (
        Vec3::new(
            position.x + angle.cos() * distance,
            position.y + y,
            position.z + angle.sin() * distance,
        ),
        epoch,
    )
}

fn difference(target: Vec3, position: Vec3) -> Vec3 {
    Vec3::new(
        target.x - position.x,
        target.y - position.y,
        target.z - position.z,
    )
}
pub(crate) fn face_motion(
    swimmer: Swimmer,
    position: Vec3,
    target: Vec3,
    velocity: Vec3,
    rotation: &mut Rotation,
) {
    let direction = if swimmer == Swimmer::Fish {
        difference(target, position)
    } else {
        velocity
    };
    if direction.horizontal_len() > f64::EPSILON {
        let desired = crate::yaw_from_velocity(direction);
        // FishMoveControl.rotlerp(...,90); Squid.aiStep uses a 0.1 body-yaw lerp.
        let limit = if swimmer == Swimmer::Squid {
            ((desired - rotation.yaw + 180.0).rem_euclid(360.0) - 180.0).abs() * 0.1
        } else {
            90.0
        };
        rotation.yaw = crate::mob_control_26_1_2::rotate_towards(rotation.yaw, desired, limit);
        rotation.head_yaw = rotation.yaw;
    }
    // AbstractFish does not set XRot in its move control. Squid's xBodyRot is
    // client animation state, not the ordinary entity pitch sent on the wire.
    if swimmer != Swimmer::Other {
        rotation.pitch = 0.0;
    } else if velocity.horizontal_len() > f64::EPSILON || velocity.y != 0.0 {
        rotation.pitch =
            ((-velocity.y).atan2(velocity.horizontal_len()).to_degrees() as f32).clamp(-35.0, 35.0);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn steer(
    swimmer: Swimmer,
    id: EntityId,
    tick: u64,
    position: Vec3,
    target: Vec3,
    mut rotation: Rotation,
    mut velocity: Vec3,
    swim_speed: f32,
    speed: f64,
) -> (Vec3, f32) {
    let delta = difference(target, position);
    let length = delta.horizontal_len().hypot(delta.y);
    match swimmer {
        Swimmer::Fish => {
            // FishSwimGoal modifier=1; speed is the movement attribute here.
            // Vanilla uses float lerp(.125), .01 moveRelative and .1 vertical drive.
            let control_speed = swim_speed + 0.125 * (speed as f32 - swim_speed);
            face_motion(swimmer, position, target, velocity, &mut rotation);
            let yaw = f64::from(rotation.yaw).to_radians();
            velocity.x -= yaw.sin() * f64::from(control_speed) * 0.01 * 20.0;
            velocity.z += yaw.cos() * f64::from(control_speed) * 0.01 * 20.0;
            velocity.y += 0.005 * 20.0;
            if length > f64::EPSILON {
                velocity.y += f64::from(control_speed) * delta.y / length * 0.1 * 20.0;
            }
            (velocity, control_speed)
        }
        Swimmer::Squid => {
            // Squid seeds its RNG from entity id; its tentacle speed is
            // .2/(nextFloat()+1). Keep a deterministic individual cycle; the
            // vanilla 1/10 cycle-speed reroll and event 19 animation sync are not modeled.
            let phase_speed = 0.2 / (crate::deterministic_unit(id, 0x51) + 1.0);
            let phase = ((tick as f64 + 1.0) * phase_speed).rem_euclid(std::f64::consts::TAU);
            if phase < std::f64::consts::PI && phase / std::f64::consts::PI > 0.75 {
                let horizontal = delta.horizontal_normalized();
                velocity = Vec3::new(
                    horizontal.x * 0.2 * 20.0,
                    delta.y.clamp(-1.0, 1.0) * 0.1 * 20.0,
                    horizontal.z * 0.2 * 20.0,
                );
            } else if phase >= std::f64::consts::PI {
                velocity = Vec3::new(velocity.x * 0.9, velocity.y * 0.9, velocity.z * 0.9);
            }
            (velocity, 0.0)
        }
        Swimmer::Other => {
            // Do not substitute FishMoveControl for dolphin, guardian, turtle,
            // axolotl, tadpole or drowned. Their existing coarse wander remains.
            let horizontal = delta.horizontal_normalized();
            (
                Vec3::new(
                    horizontal.x * speed,
                    if length > f64::EPSILON {
                        delta.y / length * 0.18
                    } else {
                        0.0
                    },
                    horizontal.z * speed,
                ),
                0.0,
            )
        }
    }
}

fn candidate_target(request: &GoalPathingRequest, candidate: usize) -> Vec3 {
    if candidate == 0 {
        return request.target;
    }
    let delta = difference(request.target, request.expected_position);
    let angle = delta.z.atan2(delta.x)
        + match candidate {
            1 => std::f64::consts::FRAC_PI_2,
            2 => -std::f64::consts::FRAC_PI_2,
            _ => std::f64::consts::PI,
        };
    Vec3::new(
        request.expected_position.x + angle.cos() * crate::WANDER_MIN_DISTANCE,
        request.expected_position.y - delta.y,
        request.expected_position.z + angle.sin() * crate::WANDER_MIN_DISTANCE,
    )
}

fn candidate_motion(request: &GoalPathingRequest, tick: u64, target: Vec3) -> (Vec3, f32) {
    steer(
        request.aquatic.expect("aquatic request"),
        request.id,
        tick,
        request.expected_position,
        target,
        request.expected_rotation,
        request.expected_velocity,
        request.expected_path.swim_speed,
        request.speed,
    )
}

fn next_position(position: Vec3, velocity: Vec3) -> Vec3 {
    Vec3::new(
        position.x + velocity.x * PathingBudget::TICK_SECONDS,
        position.y + velocity.y * PathingBudget::TICK_SECONDS,
        position.z + velocity.z * PathingBudget::TICK_SECONDS,
    )
}

pub(crate) fn visit_probe_positions(
    request: &GoalPathingRequest,
    tick: u64,
    budget: PathingBudget,
    mut visit: impl FnMut(Vec3),
) {
    if budget.max_candidates_per_entity == 0 {
        return;
    }
    visit(request.expected_position);
    for candidate in 0..budget.max_candidates_per_entity.saturating_sub(1).min(4) {
        let (velocity, _) = candidate_motion(request, tick, candidate_target(request, candidate));
        visit(next_position(request.expected_position, velocity));
    }
}

pub(crate) fn resolve(
    request: &GoalPathingRequest,
    tick: u64,
    probe: &dyn PathingProbe,
    budget: PathingBudget,
) -> (PathingDecision, RetainedPathState) {
    let mut path = request.expected_path;
    path.last_position = request.expected_position;
    path.has_last_position = true;
    let swimmer = request.aquatic.expect("aquatic request");
    let mut kind = PathingDecisionKind::Blocked;
    if budget.max_candidates_per_entity == 0 {
        return (
            PathingDecision {
                velocity: Vec3::ZERO,
                kind,
                direct: false,
            },
            path,
        );
    }
    match probe.can_entity_swim_at(request.id, request.expected_position) {
        PathingProbeResult::Walkable => {}
        result => {
            path.has_target = false;
            path.swim_speed = 0.0;
            let mut velocity = request.expected_velocity;
            if result == PathingProbeResult::Unloaded {
                velocity = Vec3::ZERO;
                kind = PathingDecisionKind::Unloaded;
            } else if swimmer == Swimmer::Fish && request.expected_on_ground {
                // AbstractFish.aiStep's grounded flop: +/- .05 horizontal, +.4 vertical.
                velocity.x +=
                    (crate::deterministic_unit(request.id, tick) * 2.0 - 1.0) * 0.05 * 20.0;
                velocity.y += 0.400_000_005_960_464_5 * 20.0;
                velocity.z += (crate::deterministic_unit(request.id, tick.wrapping_add(1)) * 2.0
                    - 1.0)
                    * 0.05
                    * 20.0;
            } else if swimmer == Swimmer::Squid {
                velocity.x = 0.0;
                velocity.z = 0.0;
            }
            return (
                PathingDecision {
                    velocity,
                    kind,
                    direct: false,
                },
                path,
            );
        }
    }
    for candidate in 0..budget.max_candidates_per_entity.saturating_sub(1).min(4) {
        let target = candidate_target(request, candidate);
        let (velocity, swim_speed) = candidate_motion(request, tick, target);
        match probe.can_entity_swim_at(
            request.id,
            next_position(request.expected_position, velocity),
        ) {
            PathingProbeResult::Walkable => {
                path.target = target;
                path.target_epoch = request.target_epoch;
                path.has_target = true;
                path.swim_speed = swim_speed;
                return (
                    PathingDecision {
                        velocity,
                        kind: PathingDecisionKind::Move,
                        direct: candidate == 0,
                    },
                    path,
                );
            }
            PathingProbeResult::Unloaded => kind = PathingDecisionKind::Unloaded,
            PathingProbeResult::Blocked => {}
        }
    }
    // Stop only when all bounded alternatives are unsafe. Discard the blocked
    // destination so the next decision can escape rather than accelerate into a wall forever.
    path.has_target = false;
    path.target_epoch = request.target_epoch;
    path.swim_speed = 0.0;
    (
        PathingDecision {
            velocity: Vec3::ZERO,
            kind,
            direct: false,
        },
        path,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_goal(
    id: EntityId,
    type_name: &str,
    tick: u64,
    position: Vec3,
    rotation: &mut Rotation,
    velocity: Vec3,
    path: &mut RetainedPathState,
    goal: &crate::GoalState,
    resolved: Option<&crate::GoalPathingResult>,
) -> Vec3 {
    let swimmer = Swimmer::for_type(type_name);
    if let Some(result) = resolved {
        if path.has_target {
            face_motion(
                swimmer,
                position,
                path.target,
                result.decision.velocity,
                rotation,
            );
        }
        return result.decision.velocity;
    }
    let crate::GoalState::AquaticWander {
        speed,
        vertical_speed,
        period_ticks,
    } = goal
    else {
        unreachable!("aquatic goal dispatch");
    };
    let (target, epoch) = wander_target(
        id,
        position,
        *path,
        tick,
        *period_ticks,
        *vertical_speed,
        swimmer,
    );
    path.target = target;
    path.target_epoch = Some(epoch);
    path.has_target = true;
    path.last_position = position;
    path.has_last_position = true;
    let (velocity, swim_speed) = steer(
        swimmer,
        id,
        tick,
        position,
        target,
        *rotation,
        velocity,
        path.swim_speed,
        *speed,
    );
    path.swim_speed = swim_speed;
    face_motion(swimmer, position, target, velocity, rotation);
    velocity
}

#[cfg(test)]
#[path = "aquatic_motion_tests.rs"]
mod tests;
