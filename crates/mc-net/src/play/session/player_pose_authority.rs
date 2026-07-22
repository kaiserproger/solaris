use std::collections::{HashMap, HashSet};

use mc_entity::{EntityId, EntityKinematics, EntityLifecycle, EntitySnapshot, Vec3};
use mc_physics::entity_collision_26_1_2::{
    CrammingContact, EntityPushPairInput, TeamCollisionRule, TeamRelationship, apply_cramming_roll,
    vanilla_cramming_roll_request, vanilla_push_impulses, vanilla_pushable_by,
};

use crate::play::PlayerPose;
use crate::play::splitmix64;
use crate::play::wire_entities::ServerEntityWireMove;

use super::entity_lifecycle::{
    move_entity_chunk_locked, nearby_entity_candidate_ids_locked, track_entity_chunk_locked,
};
use super::interaction_geometry::{canonical_entity_facts, entity_geometry};
use super::outbound::{
    OutboundCommand, ServerEntityMove, ServerEntitySnapshot, VisibilityDispatch,
};
use super::visibility::{
    entity_velocity_changed, finish_player_pose_locked, ordered_session_recipient,
    publish_entity_movement_locked, publish_player_body_snapshot_locked,
    refresh_entity_target_visibility_locked, spawn_entity_visibility_from_snapshot_locked,
    visible_entity_observers_locked, visible_observers_locked,
};
use super::{
    EntityStoreGuard, SessionId, SessionRegistryInner, chunk_pos_from_coords,
    record_entity_dispatches_locked,
};

const PLAYER_BODY_HALF_WIDTH: f64 = 0.3;
const PLAYER_BODY_HEIGHT: f64 = 1.8;

pub(super) struct AcceptedPlayerPose {
    old_observers: HashSet<SessionId>,
    old_chunk: (i32, i32),
    old_prewarm_frontier: ((i32, i32), i32, f32),
    body_candidate_ids: HashSet<EntityId>,
}

impl AcceptedPlayerPose {
    pub(super) fn old_prewarm_frontier(&self) -> ((i32, i32), i32, f32) {
        self.old_prewarm_frontier
    }

    pub(super) fn take_body_candidate_ids(&mut self) -> HashSet<EntityId> {
        std::mem::take(&mut self.body_candidate_ids)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PlayerBodyMutation {
    pub(super) expected: EntitySnapshot,
    pub(super) next: EntityKinematics,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum PlayerContactRequirement {
    CollisionFactsUnavailable { entity_ids: Vec<EntityId> },
    PlayerVelocity { impulse: Vec3 },
    CrammingDamage { amount: f32 },
    InvalidContact { entity_id: EntityId },
    InvalidCrammingRoll { roll: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PlayerEntityContactFacts {
    pub(super) pusher_rule: TeamCollisionRule,
    pub(super) contact_rule: TeamCollisionRule,
    pub(super) team_relationship: TeamRelationship,
    pub(super) player_physics_enabled: bool,
    pub(super) contact_physics_enabled: bool,
    pub(super) passenger_of_same_vehicle: bool,
    pub(super) player_pushable: bool,
    pub(super) player_is_vehicle: bool,
    pub(super) contact_pushable: bool,
    pub(super) contact_is_vehicle: bool,
    pub(super) contact_is_passenger: bool,
    pub(super) contact_is_spectator: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PlayerContactContext {
    pub(super) entity_facts: HashMap<EntityId, PlayerEntityContactFacts>,
    pub(super) max_entity_cramming: u32,
    pub(super) session_id: SessionId,
    pub(super) simulation_tick: u64,
}

#[derive(Debug, Clone)]
pub(super) struct PlayerContactCandidate {
    pub(super) snapshot: EntitySnapshot,
    pub(super) aabb: mc_physics::Aabb,
}

pub(super) struct PlayerBodyPushes {
    mutations: Vec<PlayerBodyMutation>,
    requirements: Vec<PlayerContactRequirement>,
    #[cfg(test)]
    visited_entities: u64,
}

impl PlayerBodyPushes {
    pub(super) fn into_mutations(
        self,
    ) -> Result<Vec<PlayerBodyMutation>, Vec<PlayerContactRequirement>> {
        if self.requirements.is_empty() {
            Ok(self.mutations)
        } else {
            Err(self.requirements)
        }
    }

    #[cfg(test)]
    pub(super) fn mutations(&self) -> &[PlayerBodyMutation] {
        &self.mutations
    }

    #[cfg(test)]
    pub(super) fn requirements(&self) -> &[PlayerContactRequirement] {
        &self.requirements
    }

    #[cfg(test)]
    pub(super) fn visited_entities(&self) -> u64 {
        self.visited_entities
    }
}

pub(super) fn accept_player_pose_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    pose: PlayerPose,
) -> Option<AcceptedPlayerPose> {
    let old_observers = visible_observers_locked(inner, id);
    let old_session = inner.sessions.get(&id)?;
    let old_chunk = old_session.pose.chunk_pos();
    let old_prewarm_frontier = (
        old_session.center,
        old_session.view_distance,
        old_session.pose.yaw,
    );
    inner.sessions.get_mut(&id)?.pose = pose;
    inner.publish_combat_target(id);
    let max_entity_half_width = inner
        .entity_type_aabbs
        .values()
        .map(|aabb| aabb.half_width)
        .fold(mc_physics::Aabb::COW.half_width, f64::max);
    let player = Vec3::new(pose.x, pose.y, pose.z);
    let body_candidate_ids = nearby_entity_candidate_ids_locked(
        inner,
        player,
        PLAYER_BODY_HALF_WIDTH + max_entity_half_width,
    )
    .into_iter()
    .collect();
    Some(AcceptedPlayerPose {
        old_observers,
        old_chunk,
        old_prewarm_frontier,
        body_candidate_ids,
    })
}

pub(super) fn plan_entities_from_player_locked(
    entities: &EntityStoreGuard<'_>,
    pose: PlayerPose,
    candidate_ids: &HashSet<EntityId>,
    context: Option<&PlayerContactContext>,
) -> PlayerBodyPushes {
    let mut candidate_geometry = Vec::new();
    #[cfg(test)]
    let mut visited_entities = 0_u64;
    entities.visit_simulation_entities_for_ids(candidate_ids, |entity| {
        #[cfg(test)]
        {
            visited_entities += 1;
        }
        if entity.lifecycle == EntityLifecycle::Alive
            && canonical_entity_facts(entity.type_name)
                .is_some_and(|facts| facts.category.is_living())
        {
            let aabb = entity_geometry(entity.type_name, entity.animal).aabb;
            candidate_geometry.push((entity.id, aabb));
        }
    });
    let candidates = candidate_geometry
        .into_iter()
        .filter_map(|(entity_id, aabb)| {
            entities
                .snapshot(entity_id)
                .filter(|snapshot| snapshot.lifecycle == EntityLifecycle::Alive)
                .map(|snapshot| PlayerContactCandidate { snapshot, aabb })
        })
        .collect();

    let mut plan = plan_player_contacts(pose, candidates, context);
    PlayerBodyPushes {
        mutations: std::mem::take(&mut plan.mutations),
        requirements: std::mem::take(&mut plan.requirements),
        #[cfg(test)]
        visited_entities,
    }
}

fn player_aabb_intersects(
    pose: PlayerPose,
    entity_position: Vec3,
    entity_aabb: mc_physics::Aabb,
) -> bool {
    (pose.x - entity_position.x).abs() < PLAYER_BODY_HALF_WIDTH + entity_aabb.half_width
        && pose.y < entity_position.y + entity_aabb.height
        && pose.y + PLAYER_BODY_HEIGHT > entity_position.y
        && (pose.z - entity_position.z).abs() < PLAYER_BODY_HALF_WIDTH + entity_aabb.half_width
}

pub(super) fn plan_player_contacts(
    pose: PlayerPose,
    mut candidates: Vec<PlayerContactCandidate>,
    context: Option<&PlayerContactContext>,
) -> PlayerBodyPushes {
    candidates.sort_unstable_by_key(|candidate| candidate.snapshot.id);
    let candidates = candidates
        .into_iter()
        .filter(|candidate| {
            candidate.snapshot.lifecycle == EntityLifecycle::Alive
                && player_aabb_intersects(pose, candidate.snapshot.position, candidate.aabb)
        })
        .collect::<Vec<_>>();

    let Some(context) = context else {
        let entity_ids = candidates
            .into_iter()
            .map(|candidate| candidate.snapshot.id)
            .collect::<Vec<_>>();
        let requirements = (!entity_ids.is_empty())
            .then_some(PlayerContactRequirement::CollisionFactsUnavailable { entity_ids })
            .into_iter()
            .collect();
        return PlayerBodyPushes {
            mutations: Vec::new(),
            requirements,
            #[cfg(test)]
            visited_entities: 0,
        };
    };

    let mut missing_facts = Vec::new();
    let mut mutations = Vec::new();
    let mut contacts = Vec::new();
    let mut contact_ids = Vec::new();
    let mut player_impulse = Vec3::ZERO;
    let mut requirements = Vec::new();

    for candidate in candidates {
        let entity_id = candidate.snapshot.id;
        let Some(facts) = context.entity_facts.get(&entity_id).copied() else {
            missing_facts.push(entity_id);
            continue;
        };
        if !vanilla_pushable_by(
            facts.pusher_rule,
            facts.contact_rule,
            facts.team_relationship,
            facts.contact_pushable,
            facts.contact_is_spectator,
        ) {
            continue;
        }

        contacts.push(CrammingContact {
            is_passenger: facts.contact_is_passenger,
        });
        contact_ids.push(entity_id);
        let impulses = match vanilla_push_impulses(EntityPushPairInput {
            caller_to_other_x: pose.x - candidate.snapshot.position.x,
            caller_to_other_z: pose.z - candidate.snapshot.position.z,
            caller_physics_enabled: facts.contact_physics_enabled,
            other_physics_enabled: facts.player_physics_enabled,
            passenger_of_same_vehicle: facts.passenger_of_same_vehicle,
            caller_pushable: facts.contact_pushable,
            caller_is_vehicle: facts.contact_is_vehicle,
            other_pushable: facts.player_pushable,
            other_is_vehicle: facts.player_is_vehicle,
        }) {
            Ok(impulses) => impulses,
            Err(_) => {
                requirements.push(PlayerContactRequirement::InvalidContact { entity_id });
                continue;
            }
        };

        let entity_impulse = Vec3::new(impulses.caller.x, impulses.caller.y, impulses.caller.z);
        if entity_impulse != Vec3::ZERO {
            let next_velocity = Vec3::new(
                candidate.snapshot.velocity.x + entity_impulse.x,
                candidate.snapshot.velocity.y + entity_impulse.y,
                candidate.snapshot.velocity.z + entity_impulse.z,
            );
            if next_velocity.is_finite() {
                mutations.push(PlayerBodyMutation {
                    expected: candidate.snapshot.clone(),
                    next: EntityKinematics {
                        id: entity_id,
                        position: candidate.snapshot.position,
                        rotation: candidate.snapshot.rotation,
                        velocity: next_velocity,
                        on_ground: candidate.snapshot.on_ground,
                    },
                });
            } else {
                requirements.push(PlayerContactRequirement::InvalidContact { entity_id });
            }
        }
        player_impulse.x += impulses.other.x;
        player_impulse.y += impulses.other.y;
        player_impulse.z += impulses.other.z;
    }

    if !missing_facts.is_empty() {
        requirements.push(PlayerContactRequirement::CollisionFactsUnavailable {
            entity_ids: missing_facts,
        });
    }
    if let Some(request) = vanilla_cramming_roll_request(&contacts, context.max_entity_cramming) {
        let roll =
            deterministic_cramming_roll(context.session_id, context.simulation_tick, &contact_ids);
        match apply_cramming_roll(request, roll) {
            Ok(Some(amount)) => {
                requirements.push(PlayerContactRequirement::CrammingDamage { amount });
            }
            Ok(None) => {}
            Err(_) => requirements.push(PlayerContactRequirement::InvalidCrammingRoll { roll }),
        }
    }
    if player_impulse != Vec3::ZERO {
        requirements.push(PlayerContactRequirement::PlayerVelocity {
            impulse: player_impulse,
        });
    }
    if !requirements.is_empty() {
        mutations.clear();
    }

    PlayerBodyPushes {
        mutations,
        requirements,
        #[cfg(test)]
        visited_entities: 0,
    }
}

pub(super) fn deterministic_cramming_roll(
    session_id: SessionId,
    simulation_tick: u64,
    entity_ids: &[EntityId],
) -> u8 {
    let mut sorted_ids = entity_ids.to_vec();
    sorted_ids.sort_unstable();
    let mut seed = splitmix64(session_id ^ simulation_tick.rotate_left(29));
    for entity_id in sorted_ids {
        seed = splitmix64(seed ^ entity_id.0 as u32 as u64);
    }
    (seed % u64::from(mc_physics::entity_collision_26_1_2::CRAMMING_ROLL_DENOMINATOR)) as u8
}

pub(super) fn filter_current_expected_entity_snapshots(
    expected: Vec<EntitySnapshot>,
    current: Vec<EntitySnapshot>,
) -> Vec<EntitySnapshot> {
    let current = current
        .into_iter()
        .map(|snapshot| (snapshot.id, snapshot))
        .collect::<HashMap<_, _>>();
    expected
        .into_iter()
        .filter(|snapshot| current.get(&snapshot.id) == Some(snapshot))
        .collect()
}

pub(super) fn publish_player_body_pushes_locked(
    inner: &mut SessionRegistryInner,
    pushed_entities: Vec<ServerEntitySnapshot>,
) -> Vec<VisibilityDispatch> {
    let mut dispatches = Vec::new();
    for pushed in pushed_entities {
        let entity_id = pushed.id;
        let position = pushed.position;
        let velocity = pushed.velocity;
        let old_observers = visible_entity_observers_locked(inner, entity_id);
        let mut snapshot = publish_player_body_snapshot_locked(inner, pushed);
        snapshot.velocity = velocity;
        if let Some(published) = inner.published_entity_snapshots.get_mut(&entity_id) {
            published.velocity = velocity;
        }
        let new_chunk = chunk_pos_from_coords(position.x, position.z);
        match inner.simulation_inputs.entity_chunk(entity_id) {
            Some(old_chunk) if old_chunk != new_chunk => {
                move_entity_chunk_locked(inner, entity_id, old_chunk, new_chunk);
                dispatches.extend(refresh_entity_target_visibility_locked(
                    inner, entity_id, old_chunk, new_chunk,
                ));
            }
            None => {
                track_entity_chunk_locked(inner, entity_id, position);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    inner,
                    snapshot.clone(),
                ));
            }
            Some(_) => {}
        }

        let new_observers = visible_entity_observers_locked(inner, entity_id);
        let send_velocity = inner
            .entity_movement_trackers
            .get(entity_id)
            .is_some_and(|last_sent| entity_velocity_changed(last_sent.velocity, velocity));
        let mut movement_dispatches =
            publish_entity_movement_locked(inner, &snapshot, &old_observers, &new_observers);
        if send_velocity {
            let mut covered_observers = HashSet::new();
            for dispatch in &mut movement_dispatches {
                if let OutboundCommand::MoveEntityRelative(movement) = &mut dispatch.command
                    && movement.id == entity_id
                {
                    movement.velocity = velocity;
                    movement.send_velocity = true;
                    covered_observers.insert(dispatch.recipient.id);
                }
            }
            let velocity_dispatches = old_observers
                .intersection(&new_observers)
                .filter(|observer_id| !covered_observers.contains(observer_id))
                .filter_map(|observer_id| {
                    let observer = inner.sessions.get(observer_id)?;
                    Some(VisibilityDispatch {
                        recipient: ordered_session_recipient(*observer_id, observer),
                        command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                            id: entity_id,
                            position: snapshot.position,
                            wire_move: Option::<ServerEntityWireMove>::None,
                            velocity,
                            rotation: snapshot.rotation,
                            on_ground: snapshot.on_ground,
                            send_velocity: true,
                            send_head_rotation: false,
                        }),
                    })
                })
                .collect::<Vec<_>>();
            inner
                .entity_movement_trackers
                .update(entity_id, |last_sent| last_sent.velocity = velocity);
            record_entity_dispatches_locked(inner, &velocity_dispatches);
            movement_dispatches.extend(velocity_dispatches);
        }
        dispatches.extend(movement_dispatches);
    }
    dispatches
}

pub(super) fn complete_accepted_player_pose_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    pose: PlayerPose,
    accepted: AcceptedPlayerPose,
    mut body_push_dispatches: Vec<VisibilityDispatch>,
) -> Vec<VisibilityDispatch> {
    body_push_dispatches.extend(finish_player_pose_locked(
        inner,
        id,
        pose,
        accepted.old_chunk,
        &accepted.old_observers,
    ));
    body_push_dispatches
}

#[cfg(test)]
#[path = "player_pose_authority_tests.rs"]
mod tests;
