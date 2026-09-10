use std::collections::{BTreeMap, HashSet};

use crate::{EntityStore, EntityTrackingMotion};

use super::super::{RegionKey, RegionLease};
use super::{
    FencedKinematicsCandidate, LocalPhysicsInput, LocalPhysicsRegionBatch, OwnerLanePhysicsOutput,
    OwnerLanePhysicsTickInput, RegionOwnerLaneError, apply_local_physics_if_current,
};

pub(super) fn tick_owned_region_physics(
    lane: usize,
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    input: OwnerLanePhysicsTickInput,
) -> Result<OwnerLanePhysicsOutput, RegionOwnerLaneError> {
    let worker_receipt = std::time::Instant::now();
    let OwnerLanePhysicsTickInput {
        regions: physics_regions,
        mut goal_motion,
        simulation_chunks,
        world,
    } = input;
    for lease in &physics_regions {
        if lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let Some((current_lease, _)) = regions.get(&lease.key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        if current_lease != lease {
            return Err(RegionOwnerLaneError::StaleLease);
        }
    }

    let candidate_capacity = physics_regions
        .iter()
        .filter_map(|lease| regions.get(&lease.key))
        .map(|(_, store)| store.len())
        .sum();
    let mut fallback_candidates = Vec::new();
    let mut committed_motion =
        Vec::<(RegionLease, EntityTrackingMotion)>::with_capacity(candidate_capacity);
    let mut terrain_pathing_additions = HashSet::new();
    for lease in physics_regions {
        let store = &mut regions
            .get_mut(&lease.key)
            .expect("validated owner physics region")
            .1;
        let mut candidate_results = Vec::with_capacity(store.len());
        store.visit_simulation_fence_results(|state, result| {
            let chunk = (
                (state.motion.position.x.floor() as i32).div_euclid(16),
                (state.motion.position.z.floor() as i32).div_euclid(16),
            );
            if state.lifecycle != crate::EntityLifecycle::Alive
                || !simulation_chunks.contains(&chunk)
            {
                return;
            }
            let candidate = FencedKinematicsCandidate {
                lease,
                id: state.motion.id,
                uuid: state.uuid,
                lifecycle: state.lifecycle,
                pickup_claimed: state.pickup_claimed,
                vehicle_attached: state.vehicle_attached,
                result,
            };
            let Some(result) = result else {
                return;
            };
            if candidate.eligible()
                && matches!(
                    result.physics.kind,
                    crate::EntityPhysicsKind::Living
                        | crate::EntityPhysicsKind::PowderSnowWalkableLiving
                        | crate::EntityPhysicsKind::FishLiving
                        | crate::EntityPhysicsKind::SquidLiving
                        | crate::EntityPhysicsKind::AquaticLiving
                )
            {
                candidate_results.push(candidate);
            }
        });
        let physics_world = world.physics_world(
            candidate_results
                .iter()
                .filter_map(|candidate| candidate.result.map(|result| result.physics)),
        );
        let mut direct_candidates = Vec::with_capacity(candidate_results.len());
        let mut physics_inputs = Vec::with_capacity(candidate_results.len());
        for candidate in candidate_results {
            let result = candidate
                .result
                .expect("owner-selected living entity has simulation facts");
            let step = physics_world
                .step_local(result.physics)
                .expect("owner-selected living entity has local physics");
            let source_chunk = (
                (result.physics.position.x.floor() as i32).div_euclid(16),
                (result.physics.position.z.floor() as i32).div_euclid(16),
            );
            let target_chunk = (
                (step.position.x.floor() as i32).div_euclid(16),
                (step.position.z.floor() as i32).div_euclid(16),
            );
            if source_chunk != target_chunk {
                fallback_candidates.push(candidate);
                continue;
            }
            let previous = candidate
                .expected()
                .expect("owner-selected living entity retains its compact fence");
            direct_candidates.push((
                candidate,
                step.horizontal_collision && step.velocity.y <= 0.0,
            ));
            physics_inputs.push(LocalPhysicsInput {
                previous: previous.into(),
                expected: result.physics,
                step,
                publish: true,
            });
        }
        direct_candidates.sort_unstable_by_key(|(candidate, _)| candidate.id);
        let (rejected, accepted, committed) = apply_local_physics_if_current(
            lane,
            regions,
            vec![LocalPhysicsRegionBatch {
                lease,
                inputs: physics_inputs,
                world_fence: Some(physics_world),
            }],
            true,
        )?;
        let mut accepted = accepted.into_iter().peekable();
        let mut committed = committed.into_iter().peekable();
        for (candidate, horizontal_collision) in direct_candidates {
            if rejected.binary_search(&candidate.id).is_ok() {
                fallback_candidates.push(candidate);
                continue;
            }
            if horizontal_collision {
                terrain_pathing_additions.insert(candidate.id);
            }
            let Some((lease, previous)) = accepted.next() else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            if previous.id != candidate.id {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if let Some(state) = committed.next_if(|state| state.id == candidate.id) {
                let mut motion = previous;
                motion.position = state.position;
                motion.rotation = state.rotation;
                motion.velocity = state.velocity;
                motion.on_ground = state.on_ground;
                committed_motion.push((lease, motion.into()));
            }
        }
        if accepted.next().is_some() || committed.next().is_some() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
    }
    committed_motion.sort_unstable_by_key(|(_, motion)| motion.id);
    let mut wrong_lane = false;
    goal_motion.retain_mut(|(lease, motion)| {
        if lease.lane != lane {
            wrong_lane = true;
            return false;
        }
        // Physics publication replaces this goal entry; do not refresh a discarded value.
        if committed_motion
            .binary_search_by_key(&motion.id, |(_, current)| current.id)
            .is_ok()
        {
            return false;
        }
        let Some((current_lease, store)) = regions.get(&lease.key) else {
            return false;
        };
        if current_lease != lease {
            return false;
        }
        let Some(current) = store.kinematics_fence_state(motion.id) else {
            return false;
        };
        if current.lifecycle != crate::EntityLifecycle::Alive {
            return false;
        }
        motion.position = current.motion.position;
        motion.rotation = current.motion.rotation;
        motion.velocity = current.motion.velocity;
        motion.on_ground = current.motion.on_ground;
        true
    });
    if wrong_lane {
        return Err(RegionOwnerLaneError::WrongLane);
    }
    let physics_mutated = !committed_motion.is_empty();
    let committed_motion = merge_committed_motion(goal_motion, committed_motion);
    Ok(OwnerLanePhysicsOutput {
        fallback_candidates,
        committed_motion,
        terrain_pathing_additions,
        physics_mutated,
        state_version: 0,
        worker_exec_us: worker_receipt
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64,
    })
}

fn merge_committed_motion(
    goal_motion: Vec<(RegionLease, EntityTrackingMotion)>,
    physics_motion: Vec<(RegionLease, EntityTrackingMotion)>,
) -> Vec<(RegionLease, EntityTrackingMotion)> {
    let mut goal = goal_motion.into_iter().peekable();
    let mut physics = physics_motion.into_iter().peekable();
    let mut merged = Vec::with_capacity(goal.len().max(physics.len()));
    while goal.peek().is_some() || physics.peek().is_some() {
        match (
            goal.peek().map(|(_, motion)| motion.id),
            physics.peek().map(|(_, motion)| motion.id),
        ) {
            (Some(goal_id), Some(physics_id)) if goal_id < physics_id => {
                merged.push(goal.next().expect("peeked goal motion"));
            }
            (Some(goal_id), Some(physics_id)) if physics_id < goal_id => {
                merged.push(physics.next().expect("peeked physics motion"));
            }
            (Some(_), Some(_)) => {
                goal.next();
                merged.push(physics.next().expect("peeked physics motion"));
            }
            (Some(_), None) => merged.push(goal.next().expect("peeked goal motion")),
            (None, Some(_)) => merged.push(physics.next().expect("peeked physics motion")),
            (None, None) => break,
        }
    }
    merged
}
