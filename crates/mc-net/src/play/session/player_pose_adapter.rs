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
    complete_accepted_player_pose_locked, plan_entities_from_player_candidate_geometry_locked,
    player_contact_geometry_from_projections, publish_player_body_pushes_locked,
};
use super::visibility::server_entity_snapshot_from;
use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    pub(in crate::play) fn commit_player_pose(
        &self,
        authority: &SimulationAuthority,
        id: SessionId,
        pose: PlayerPose,
        exhaustion: f32,
    ) -> Result<(Vec<VisibilityDispatch>, CommittedPlayerPose), SimulationRequestError> {
        self.commit_player_pose_batch(authority, vec![(id, pose, exhaustion)])
            .pop()
            .expect("single pose batch returns one result")
    }

    pub(in crate::play) fn commit_player_pose_batch(
        &self,
        _authority: &SimulationAuthority,
        requests: Vec<(SessionId, PlayerPose, f32)>,
    ) -> Vec<Result<(Vec<VisibilityDispatch>, CommittedPlayerPose), SimulationRequestError>> {
        struct AcceptedPose {
            index: usize,
            id: SessionId,
            pose: PlayerPose,
            accepted: AcceptedPlayerPose,
            committed: CommittedPlayerPose,
            candidates: HashSet<EntityId>,
            candidate_geometry: Vec<(EntityId, mc_physics::Aabb)>,
            pushed: Vec<mc_entity::EntitySnapshot>,
        }

        let mut results = std::iter::repeat_with(|| None)
            .take(requests.len())
            .collect::<Vec<_>>();
        let mut accepted_batch = Vec::with_capacity(requests.len());
        {
            let mut inner = self.lock_inner("accept player pose batch");
            for (index, (id, pose, exhaustion)) in requests.into_iter().enumerate() {
                if !inner.sessions.contains_key(&id) {
                    results[index] = Some(Err(SimulationRequestError::StaleSession));
                    continue;
                }
                let Some(player_state) = inner.player_persistence.get(&id).cloned() else {
                    results[index] = Some(Err(SimulationRequestError::InvalidCommand));
                    continue;
                };
                let wait_started = Instant::now();
                let guard = player_state.lock().unwrap_or_else(|poisoned| {
                    warn!(
                        session_id = id,
                        "player persistence mutex was poisoned during pose batch; recovering state"
                    );
                    poisoned.into_inner()
                });
                let mut player_state = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "commit player pose batch persistence",
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
                let Some(mut accepted) = accept_player_pose_locked(&mut inner, id, pose) else {
                    results[index] = Some(Err(SimulationRequestError::StaleSession));
                    continue;
                };
                self.update_prewarm_frontier_for_pose_locked(
                    &inner,
                    id,
                    accepted.old_prewarm_frontier(),
                );
                let candidates = accepted.take_body_candidate_ids();
                accepted_batch.push(AcceptedPose {
                    index,
                    id,
                    pose,
                    accepted,
                    committed,
                    candidates,
                    candidate_geometry: Vec::new(),
                    pushed: Vec::new(),
                });
            }
        }

        if accepted_batch.is_empty() {
            return results
                .into_iter()
                .map(|result| result.expect("every rejected pose has a result"))
                .collect();
        }

        let all_candidates = accepted_batch
            .iter()
            .flat_map(|entry| entry.candidates.iter().copied())
            .collect::<HashSet<_>>();
        {
            let mut entities = self.lock_entities("commit player pose batch body pushes");
            let projections = entities
                .simulation_projections_for_ids(&all_candidates)
                .into_iter()
                .map(|projection| (projection.id, projection))
                .collect::<HashMap<_, _>>();
            let mut contact_ids = HashSet::new();
            for entry in &mut accepted_batch {
                entry.candidate_geometry = player_contact_geometry_from_projections(
                    entry.pose,
                    entry
                        .candidates
                        .iter()
                        .filter_map(|entity_id| projections.get(entity_id)),
                );
                contact_ids.extend(
                    entry
                        .candidate_geometry
                        .iter()
                        .map(|(entity_id, _)| *entity_id),
                );
            }
            entities.prefetch(&contact_ids);
            for entry in &mut accepted_batch {
                let context = player_contact_context(
                    &projections,
                    &entry.candidate_geometry,
                    entry.id,
                    self.simulation_tick(),
                );
                let pushes = plan_entities_from_player_candidate_geometry_locked(
                    &entities,
                    entry.pose,
                    std::mem::take(&mut entry.candidate_geometry),
                    Some(&context),
                    entry.candidates.len() as u64,
                );
                #[cfg(test)]
                self.player_push_entity_visits
                    .fetch_add(pushes.visited_entities(), Ordering::Relaxed);
                entry.pushed = match commit_player_body_pushes_locked(&mut entities, pushes) {
                    PlayerBodyCommit::Committed(pushed) => pushed,
                    PlayerBodyCommit::Rejected => Vec::new(),
                    PlayerBodyCommit::FollowUp(requirements) => {
                        debug!(
                            requirements = requirements.len(),
                            "player contact batch deferred pending authoritative follow-up"
                        );
                        Vec::new()
                    }
                };
            }
        }

        let mut latest_push = HashMap::<EntityId, (usize, mc_entity::EntitySnapshot)>::new();
        for entry in &mut accepted_batch {
            for snapshot in std::mem::take(&mut entry.pushed) {
                latest_push.insert(snapshot.id, (entry.index, snapshot));
            }
        }
        let request_by_entity = latest_push
            .iter()
            .map(|(&entity, (index, _))| (entity, *index))
            .collect::<HashMap<_, _>>();
        let current_pushes = self.current_expected_entity_snapshots(
            latest_push.into_values().map(|(_, snapshot)| snapshot),
        );
        let mut pushes_by_request = HashMap::<usize, Vec<mc_entity::EntitySnapshot>>::new();
        for snapshot in current_pushes {
            if let Some(&index) = request_by_entity.get(&snapshot.id) {
                pushes_by_request.entry(index).or_default().push(snapshot);
            }
        }

        let mut session_to_index = HashMap::with_capacity(accepted_batch.len());
        {
            let mut inner = self.lock_inner("publish accepted player pose batch");
            for entry in accepted_batch {
                let body_push_dispatches = pushes_by_request
                    .remove(&entry.index)
                    .map(|snapshots| {
                        publish_player_body_pushes_locked(
                            &mut inner,
                            snapshots
                                .into_iter()
                                .map(server_entity_snapshot_from)
                                .collect(),
                        )
                    })
                    .unwrap_or_default();
                let dispatches = complete_accepted_player_pose_locked(
                    &mut inner,
                    entry.id,
                    entry.pose,
                    entry.accepted,
                    body_push_dispatches,
                );
                session_to_index.insert(entry.id, entry.index);
                results[entry.index] = Some(Ok((dispatches, entry.committed)));
            }
        }

        for pickup in self.pickup_candidate_dispatches(session_to_index.keys().copied().collect()) {
            if let Some(&index) = session_to_index.get(&pickup.recipient.id)
                && let Some(Ok((dispatches, _))) = results[index].as_mut()
            {
                dispatches.push(pickup);
            }
        }

        results
            .into_iter()
            .map(|result| result.expect("accepted pose batch publishes every result"))
            .collect()
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

    #[cfg(test)]
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

    #[cfg(test)]
    fn push_entities_from_player(
        &self,
        session_id: SessionId,
        pose: PlayerPose,
        candidate_ids: HashSet<EntityId>,
    ) -> Vec<VisibilityDispatch> {
        let committed = {
            let mut entities = self.lock_entities("commit player body push ECS");
            let projections = entities
                .simulation_projections_for_ids(&candidate_ids)
                .into_iter()
                .map(|projection| (projection.id, projection))
                .collect::<HashMap<_, _>>();
            let candidate_geometry = player_contact_geometry_from_projections(
                pose,
                candidate_ids
                    .iter()
                    .filter_map(|entity_id| projections.get(entity_id)),
            );
            let contact_ids = candidate_geometry
                .iter()
                .map(|(entity_id, _)| *entity_id)
                .collect::<HashSet<_>>();
            entities.prefetch(&contact_ids);
            let context = player_contact_context(
                &projections,
                &candidate_geometry,
                session_id,
                self.simulation_tick(),
            );
            let pushes = plan_entities_from_player_candidate_geometry_locked(
                &entities,
                pose,
                candidate_geometry,
                Some(&context),
                candidate_ids.len() as u64,
            );
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
    projections: &HashMap<EntityId, mc_entity::EntitySimulationProjection>,
    candidate_geometry: &[(EntityId, mc_physics::Aabb)],
    session_id: SessionId,
    simulation_tick: u64,
) -> PlayerContactContext {
    let entity_facts = candidate_geometry
        .iter()
        .filter_map(|(entity_id, _)| {
            let projection = projections.get(entity_id)?;
            Some((
                *entity_id,
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
                    contact_is_passenger: projection.has_vehicle,
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

#[cfg(test)]
fn complete_publication_batch(
    expected_count: usize,
    current: Vec<super::outbound::ServerEntitySnapshot>,
) -> Option<Vec<super::outbound::ServerEntitySnapshot>> {
    (current.len() == expected_count).then_some(current)
}

#[cfg(test)]
#[path = "player_pose_adapter_tests.rs"]
mod tests;
