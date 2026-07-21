use super::{SessionEntityGuards, SessionId};
use crate::play::persistence::PlayerPersistedState;
use mc_entity::Vec3;
use mc_protocol::packets::play::GameMode;
use mc_script::{ScriptEvent, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId};
use tracing::warn;

pub(super) fn push_player_death_event_locked(
    inner: &SessionEntityGuards<'_>,
    actor_session: SessionId,
    player_state: &PlayerPersistedState,
    position: Vec3,
) {
    let Some(sender) = inner.player_death_events.as_ref() else {
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
            "committed player death event worker is unavailable"
        );
    }
}
