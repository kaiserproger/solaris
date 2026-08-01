use std::time::Instant;

use mc_protocol::packets::play::{EntityDataValue, GameMode};

use crate::play::persistence::SpawnState;
use crate::play::simulation::{PlayerStateEvent, SimulationAuthority, SimulationRequestError};

use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::sleep::SleepWakeReason;
use super::visibility::{session_recipients, visibility_dispatches, visible_observers_locked};
use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    pub(in crate::play) fn commit_player_state_event(
        &self,
        _authority: &SimulationAuthority,
        id: SessionId,
        event: PlayerStateEvent,
    ) -> Result<Vec<VisibilityDispatch>, SimulationRequestError> {
        let mut inner = self.lock_inner("commit player state event");
        if !inner.sessions.contains_key(&id) {
            return Err(SimulationRequestError::StaleSession);
        }
        let Some(player_state) = inner.player_persistence.get(&id).cloned() else {
            return Err(SimulationRequestError::InvalidCommand);
        };
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player state event persistence",
            wait_started,
            guard,
        );
        let mut staged_spectator_wake = None;
        match event {
            PlayerStateEvent::SelectedHotbarSlot(slot) if slot <= 8 => {
                player_state.selected_hotbar_slot = slot;
            }
            PlayerStateEvent::SelectedHotbarSlot(_) => {
                return Err(SimulationRequestError::InvalidCommand);
            }
            PlayerStateEvent::RespawnPose(pose)
                if pose.x.is_finite()
                    && pose.y.is_finite()
                    && pose.z.is_finite()
                    && pose.yaw.is_finite() =>
            {
                player_state.spawn = SpawnState::from_pose(pose);
            }
            PlayerStateEvent::RespawnPose(_) => {
                return Err(SimulationRequestError::InvalidCommand);
            }
            PlayerStateEvent::GameMode(game_mode) => {
                let previous = player_state.game_mode;
                player_state.game_mode = game_mode;
                if game_mode == GameMode::Spectator {
                    staged_spectator_wake = self.stage_sleep_wake_locked(
                        &mut inner,
                        id,
                        SleepWakeReason::Spectator { previous },
                    );
                    if staged_spectator_wake.is_none() {
                        inner.spectator_sessions.insert(id);
                    }
                } else {
                    inner.spectator_sessions.remove(&id);
                }
            }
        }
        if matches!(event, PlayerStateEvent::GameMode(_)) {
            inner.publish_combat_target(id);
        }
        drop(player_state);
        let transition = (matches!(event, PlayerStateEvent::GameMode(_))
            && staged_spectator_wake.is_none())
        .then(|| self.resolve_sleep_transition_locked(&mut inner))
        .flatten();
        drop(inner);

        let mut dispatches =
            self.completed_sleep_dispatches(staged_spectator_wake.into_iter().collect(), None);
        dispatches.extend(self.sleep_transition_dispatches(transition));
        Ok(dispatches)
    }

    pub(in crate::play) fn broadcast_player_animation(
        &self,
        id: SessionId,
    ) -> Vec<VisibilityDispatch> {
        let (entity_id, recipients) = {
            let inner = self.lock_inner("broadcast player animation");
            let Some(session) = inner.sessions.get(&id) else {
                return Vec::new();
            };
            let entity_id = session.entity_id;
            let recipients = session_recipients(&inner, visible_observers_locked(&inner, id));
            (entity_id, recipients)
        };
        visibility_dispatches(recipients, || OutboundCommand::AnimatePlayer { entity_id })
    }

    pub(in crate::play) fn broadcast_player_entity_data(
        &self,
        id: SessionId,
        values: Vec<EntityDataValue>,
    ) -> Vec<VisibilityDispatch> {
        let (entity_id, recipients) = {
            let inner = self.lock_inner("broadcast player entity data");
            let Some(session) = inner.sessions.get(&id) else {
                return Vec::new();
            };
            let entity_id = session.entity_id;
            let recipients = session_recipients(&inner, visible_observers_locked(&inner, id));
            (entity_id, recipients)
        };
        visibility_dispatches(recipients, || OutboundCommand::PlayerEntityData {
            entity_id,
            values: values.clone(),
        })
    }

    pub(in crate::play) fn broadcast_player_entity_data_including_self(
        &self,
        id: SessionId,
        values: Vec<EntityDataValue>,
    ) -> Vec<VisibilityDispatch> {
        let (entity_id, recipients) = {
            let inner = self.lock_inner("broadcast player entity data including self");
            let Some(session) = inner.sessions.get(&id) else {
                return Vec::new();
            };
            let entity_id = session.entity_id;
            let mut recipient_ids = visible_observers_locked(&inner, id);
            recipient_ids.insert(id);
            let recipients = session_recipients(&inner, recipient_ids);
            (entity_id, recipients)
        };
        visibility_dispatches(recipients, || OutboundCommand::PlayerEntityData {
            entity_id,
            values: values.clone(),
        })
    }
}
