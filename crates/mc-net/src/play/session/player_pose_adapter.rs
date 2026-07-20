use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::Instant;

use mc_entity::EntityId;
use tracing::{debug, warn};

use crate::play::PlayerPose;
use crate::play::simulation::{CommittedPlayerPose, SimulationAuthority, SimulationRequestError};

use super::outbound::VisibilityDispatch;
use super::player_pose_authority::{
    AcceptedPlayerPose, PlayerBodyMutation, PlayerBodyPushes, PlayerContactContext,
    PlayerContactRequirement, PlayerEntityContactFacts, accept_player_pose_locked,
    complete_accepted_player_pose_locked, plan_entities_from_player_locked,
    publish_player_body_pushes_locked,
};
use super::visibility::server_entity_snapshot_from;
use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    pub(in crate::play) fn commit_player_pose(
        &self,
        _authority: &SimulationAuthority,
        id: SessionId,
        pose: PlayerPose,
        exhaustion: f32,
    ) -> Result<(Vec<VisibilityDispatch>, CommittedPlayerPose), SimulationRequestError> {
        let (accepted, committed) = {
            let mut inner = self.lock_inner("accept player pose");
            if !inner.sessions.contains_key(&id) {
                return Err(SimulationRequestError::StaleSession);
            }
            let player_state = inner
                .player_persistence
                .get(&id)
                .cloned()
                .ok_or(SimulationRequestError::InvalidCommand)?;
            let wait_started = Instant::now();
            let guard = player_state.lock().unwrap_or_else(|poisoned| {
                warn!(
                    session_id = id,
                    "player persistence mutex was poisoned during pose commit; recovering state"
                );
                poisoned.into_inner()
            });
            let mut player_state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "commit player pose persistence",
                wait_started,
                guard,
            );
            player_state.pose = pose;
            let resources_changed = player_state.survival.add_exhaustion(exhaustion);
            let committed = CommittedPlayerPose {
                food: player_state.survival.food,
                saturation: player_state.survival.saturation,
                exhaustion: player_state.survival.exhaustion,
                resources_changed,
            };
            drop(player_state);
            let accepted = accept_player_pose_locked(&mut inner, id, pose)
                .expect("accepted session remains present under its session lock");
            self.update_prewarm_frontier_for_pose_locked(
                &inner,
                id,
                accepted.old_prewarm_frontier(),
            );
            (accepted, committed)
        };
        Ok((
            self.publish_accepted_player_pose(id, pose, accepted),
            committed,
        ))
    }

    #[cfg(test)]
    pub(in crate::play) fn update_pose(
        &self,
        id: SessionId,
        pose: PlayerPose,
    ) -> Vec<VisibilityDispatch> {
        let accepted = {
            let mut inner = self.lock_inner("accept test player pose");
            let accepted = accept_player_pose_locked(&mut inner, id, pose);
            if let Some(accepted) = accepted.as_ref() {
                self.update_prewarm_frontier_for_pose_locked(
                    &inner,
                    id,
                    accepted.old_prewarm_frontier(),
                );
            }
            accepted
        };
        accepted
            .map(|accepted| self.publish_accepted_player_pose(id, pose, accepted))
            .unwrap_or_default()
    }

    fn publish_accepted_player_pose(
        &self,
        id: SessionId,
        pose: PlayerPose,
        mut accepted: AcceptedPlayerPose,
    ) -> Vec<VisibilityDispatch> {
        let body_candidate_ids = accepted.take_body_candidate_ids();
        let body_push_dispatches = self.push_entities_from_player(id, pose, body_candidate_ids);
        let mut dispatches = {
            let mut inner = self.lock_inner("publish accepted player pose");
            complete_accepted_player_pose_locked(
                &mut inner,
                id,
                pose,
                accepted,
                body_push_dispatches,
            )
        };
        let pickup = self.pickup_candidate_dispatch(id);
        if let Some(pickup) = pickup {
            dispatches.push(pickup);
        }
        dispatches
    }

    fn push_entities_from_player(
        &self,
        session_id: SessionId,
        pose: PlayerPose,
        candidate_ids: HashSet<EntityId>,
    ) -> Vec<VisibilityDispatch> {
        let committed = {
            let mut entities = self.lock_entities("commit player body push ECS");
            let context = player_contact_context(
                &entities,
                &candidate_ids,
                session_id,
                self.simulation_tick(),
            );
            let pushes =
                plan_entities_from_player_locked(&entities, pose, &candidate_ids, Some(&context));
            #[cfg(test)]
            self.player_push_entity_visits
                .fetch_add(pushes.visited_entities(), Ordering::Relaxed);
            commit_player_body_pushes_locked(&mut entities, pushes)
        };
        let pushed_entities = match committed {
            PlayerBodyCommit::Committed(pushed_entities) => pushed_entities,
            PlayerBodyCommit::Rejected => return Vec::new(),
            PlayerBodyCommit::FollowUp(requirements) => {
                debug!(
                    requirements = requirements.len(),
                    "player contact deferred pending authoritative follow-up"
                );
                return Vec::new();
            }
        };
        let expected_count = pushed_entities.len();
        if expected_count == 0 {
            return Vec::new();
        }

        #[cfg(test)]
        self.pause_between_player_push_entity_and_session_commit_for_test();
        let pushed_entities = self
            .current_expected_entity_snapshots(pushed_entities)
            .into_iter();
        let Some(pushed_entities) = complete_publication_batch(
            expected_count,
            pushed_entities.map(server_entity_snapshot_from).collect(),
        ) else {
            return Vec::new();
        };
        let mut inner = self.lock_inner("publish player body push");
        publish_player_body_pushes_locked(&mut inner, pushed_entities)
    }
}

fn player_contact_context(
    entities: &super::EntityStoreGuard<'_>,
    candidate_ids: &HashSet<EntityId>,
    session_id: SessionId,
    simulation_tick: u64,
) -> PlayerContactContext {
    let entity_facts = candidate_ids
        .iter()
        .filter_map(|&entity_id| {
            let snapshot = entities.snapshot(entity_id)?;
            Some((
                entity_id,
                PlayerEntityContactFacts {
                    pusher_rule: mc_physics::entity_collision_26_1_2::TeamCollisionRule::Always,
                    contact_rule: mc_physics::entity_collision_26_1_2::TeamCollisionRule::Always,
                    team_relationship:
                        mc_physics::entity_collision_26_1_2::TeamRelationship::NotAllied,
                    player_physics_enabled: true,
                    contact_physics_enabled: true,
                    passenger_of_same_vehicle: false,
                    player_pushable: false,
                    player_is_vehicle: false,
                    contact_pushable: true,
                    contact_is_vehicle: false,
                    contact_is_passenger: snapshot.vehicle.is_some(),
                    contact_is_spectator: false,
                },
            ))
        })
        .collect::<HashMap<_, _>>();
    PlayerContactContext {
        entity_facts,
        max_entity_cramming: 24,
        session_id,
        simulation_tick,
    }
}

#[derive(Debug)]
enum PlayerBodyCommit {
    Committed(Vec<mc_entity::EntitySnapshot>),
    Rejected,
    FollowUp(Vec<PlayerContactRequirement>),
}

fn commit_player_body_pushes_locked(
    entities: &mut super::EntityStoreGuard<'_>,
    pushes: PlayerBodyPushes,
) -> PlayerBodyCommit {
    let mutations = match pushes.into_mutations() {
        Ok(mutations) => mutations,
        Err(requirements) => return PlayerBodyCommit::FollowUp(requirements),
    };
    if mutations.is_empty() {
        return PlayerBodyCommit::Committed(Vec::new());
    }

    let committed = mutations.iter().map(committed_snapshot).collect::<Vec<_>>();
    let conditional = mutations
        .into_iter()
        .map(|mutation| (mutation.expected, mutation.next));
    if !entities.apply_kinematics_states_if_current(conditional) {
        return PlayerBodyCommit::Rejected;
    }
    PlayerBodyCommit::Committed(committed)
}

fn committed_snapshot(mutation: &PlayerBodyMutation) -> mc_entity::EntitySnapshot {
    let mut snapshot = mutation.expected.clone();
    snapshot.position = mutation.next.position;
    snapshot.rotation = mutation.next.rotation;
    snapshot.velocity = mutation.next.velocity;
    snapshot.on_ground = mutation.next.on_ground;
    snapshot
}

fn complete_publication_batch(
    expected_count: usize,
    current: Vec<super::outbound::ServerEntitySnapshot>,
) -> Option<Vec<super::outbound::ServerEntitySnapshot>> {
    (current.len() == expected_count).then_some(current)
}

#[cfg(test)]
#[path = "player_pose_adapter_tests.rs"]
mod tests;
