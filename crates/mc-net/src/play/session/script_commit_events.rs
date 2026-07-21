use super::{ServerEntitySnapshot, SessionEntityGuards, SessionId};
use crate::play::persistence::PlayerPersistedState;
use mc_entity::Vec3;
use mc_protocol::packets::play::GameMode;
use mc_script::{
    ScriptEntityId, ScriptEntityKillSource, ScriptEvent, ScriptGameMode, ScriptPlayerContext,
    ScriptPlayerId,
};
use tracing::warn;

pub(super) fn push_player_death_event_locked(
    inner: &SessionEntityGuards<'_>,
    actor_session: SessionId,
    player_state: &PlayerPersistedState,
    position: Vec3,
) {
    let Some(sender) = inner.script_commit_events.as_ref() else {
        return;
    };
    let Some(session) = inner.sessions.get(&actor_session) else {
        warn!(
            session_id = actor_session,
            "committed player death lost its session snapshot"
        );
        return;
    };
    let game_mode = match player_state.game_mode {
        GameMode::Survival => ScriptGameMode::Survival,
        GameMode::Adventure => ScriptGameMode::Adventure,
        game_mode => {
            warn!(
                session_id = actor_session,
                ?game_mode,
                "committed player death has unsupported script game mode"
            );
            return;
        }
    };
    let context = match ScriptPlayerContext::try_new(
        session.uuid.to_string(),
        &session.name,
        session.script_operator,
        position.x,
        position.y,
        position.z,
    ) {
        Ok(context) => context,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed player death context is invalid"
            );
            return;
        }
    };
    let event = match ScriptEvent::try_player_died_with_context(
        ScriptPlayerId::new(actor_session),
        context,
        &session.dimension,
        game_mode,
    ) {
        Ok(event) => event,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed player death script event is invalid"
            );
            return;
        }
    };
    if sender.send(event).is_err() {
        warn!(
            session_id = actor_session,
            "committed script event worker is unavailable after player death"
        );
    }
}

pub(super) fn push_player_entity_killed_event_locked(
    inner: &SessionEntityGuards<'_>,
    actor_session: SessionId,
    game_mode: GameMode,
    player_pose: crate::play::PlayerPose,
    entity: &ServerEntitySnapshot,
) {
    let Some(sender) = inner.script_commit_events.as_ref() else {
        return;
    };
    let Some(session) = inner.sessions.get(&actor_session) else {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            "committed entity kill lost its player session snapshot"
        );
        return;
    };
    let game_mode = match game_mode {
        GameMode::Survival => ScriptGameMode::Survival,
        GameMode::Creative => ScriptGameMode::Creative,
        GameMode::Adventure => ScriptGameMode::Adventure,
        game_mode => {
            warn!(
                session_id = actor_session,
                ?game_mode,
                "committed entity kill has unsupported script game mode"
            );
            return;
        }
    };
    let Ok(entity_id) = u64::try_from(entity.id.0) else {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            "committed entity kill has invalid script entity id"
        );
        return;
    };
    let context = match ScriptPlayerContext::try_new(
        session.uuid.to_string(),
        &session.name,
        session.script_operator,
        player_pose.x,
        player_pose.y,
        player_pose.z,
    ) {
        Ok(context) => context,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed entity kill player context is invalid"
            );
            return;
        }
    };
    let event = match ScriptEvent::try_player_entity_killed_with_context(
        ScriptPlayerId::new(actor_session),
        context,
        &session.dimension,
        ScriptEntityId::new(entity_id),
        &entity.type_name,
        ScriptEntityKillSource::Melee,
        game_mode,
    ) {
        Ok(event) => event,
        Err(error) => {
            warn!(
                session_id = actor_session,
                entity_id = entity.id.0,
                ?error,
                "committed entity kill script event is invalid"
            );
            return;
        }
    };
    if sender.send(event).is_err() {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            "committed script event worker is unavailable after entity kill"
        );
    }
}
