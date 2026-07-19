use std::collections::HashSet;
#[cfg(test)]
use std::sync::atomic::Ordering;
use std::time::Instant;

use mc_entity::EntityId;
use tracing::warn;

use crate::play::PlayerPose;
use crate::play::simulation::{SimulationAuthority, SimulationRequestError};

use super::outbound::VisibilityDispatch;
use super::player_pose_authority::{
    AcceptedPlayerPose, accept_player_pose_locked, complete_accepted_player_pose_locked,
    publish_player_body_pushes_locked, push_entities_from_player_locked,
};
use super::visibility::server_entity_snapshot_from;
use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    pub(in crate::play) fn commit_player_pose(
        &self,
        _authority: &SimulationAuthority,
        id: SessionId,
        pose: PlayerPose,
    ) -> Result<Vec<VisibilityDispatch>, SimulationRequestError> {
        let accepted = {
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
            drop(player_state);
            let accepted = accept_player_pose_locked(&mut inner, id, pose)
                .expect("accepted session remains present under its session lock");
            self.update_prewarm_frontier_for_pose_locked(
                &inner,
                id,
                accepted.old_prewarm_frontier(),
            );
            accepted
        };
        Ok(self.publish_accepted_player_pose(id, pose, accepted))
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
        let body_push_dispatches = self.push_entities_from_player(pose, body_candidate_ids);
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
        pose: PlayerPose,
        candidate_ids: HashSet<EntityId>,
    ) -> Vec<VisibilityDispatch> {
        let pushes = {
            let mut entities = self.lock_entities("commit player body push ECS");
            push_entities_from_player_locked(&mut entities, pose, &candidate_ids)
        };
        #[cfg(test)]
        self.player_push_entity_visits
            .fetch_add(pushes.visited_entities(), Ordering::Relaxed);
        let pushed_entities = pushes.into_expected_snapshots();

        #[cfg(test)]
        self.pause_between_player_push_entity_and_session_commit_for_test();
        let mut inner = self.lock_inner("publish player body push");
        let pushed_entities = self
            .current_expected_entity_snapshots(pushed_entities)
            .into_iter()
            .map(server_entity_snapshot_from)
            .collect();
        publish_player_body_pushes_locked(&mut inner, pushed_entities)
    }
}
