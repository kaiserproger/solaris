use std::time::Instant;

use mc_domain::GameMode;
use mc_entity::{EntityId, EntityLifecycle, Vec3};

use super::interaction_geometry::{entity_geometry, within_entity_reach};
use super::{SessionId, SessionRegistry, server_entity_snapshot_from};
use crate::lock_policy::lock_authoritative_mutex;
use crate::play::PlayerPose;

#[derive(Debug, Clone)]
pub(in crate::play) struct AcceptedScriptEntityInteraction {
    pub(in crate::play) player_pose: PlayerPose,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) entity_type: String,
    pub(in crate::play) entity_position: Vec3,
}

impl SessionRegistry {
    pub(in crate::play) fn accept_script_entity_interaction(
        &self,
        actor_session: SessionId,
        entity_id: EntityId,
    ) -> Option<AcceptedScriptEntityInteraction> {
        let inner = self.lock_session_entities("accept script entity interaction");
        let player_pose = inner.sessions.get(&actor_session)?.pose;
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let player_state = lock_authoritative_mutex(&player_state, "play.player_persistence");
        let player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "accept script entity interaction",
            wait_started,
            player_state,
        );
        if player_state.game_mode == GameMode::Spectator || player_state.survival.is_dead() {
            return None;
        }
        let game_mode = player_state.game_mode;
        drop(player_state);

        let entity = inner.entities.snapshot(entity_id)?;
        if entity.lifecycle != EntityLifecycle::Alive
            || !entity.health.is_finite()
            || entity.health <= 0.0
        {
            return None;
        }
        let entity = server_entity_snapshot_from(entity);
        if entity.health.is_none()
            || !within_entity_reach(
                player_pose,
                entity.position,
                entity_geometry(&entity.type_name, entity.animal).aabb,
                game_mode,
            )
        {
            return None;
        }

        Some(AcceptedScriptEntityInteraction {
            player_pose,
            game_mode,
            entity_id,
            entity_type: entity.type_name,
            entity_position: entity.position,
        })
    }
}
