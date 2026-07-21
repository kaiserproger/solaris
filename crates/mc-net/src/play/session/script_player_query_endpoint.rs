use mc_script::{ScriptDtoError, ScriptOnlinePlayerSnapshot, ScriptPlayerContext};

use super::SessionRegistry;

impl SessionRegistry {
    /// Return one bounded point-in-time view of sessions whose outbound owner is live.
    pub(crate) fn script_online_players(
        &self,
        limit: usize,
    ) -> Result<(Vec<ScriptOnlinePlayerSnapshot>, bool), ScriptDtoError> {
        let inner = self.lock_inner("snapshot online players for script");
        let mut sessions = inner
            .sessions
            .iter()
            .filter(|(_, session)| !session.tx.is_closed())
            .collect::<Vec<_>>();
        sessions.sort_unstable_by_key(|(session_id, _)| **session_id);
        let truncated = sessions.len() > limit;
        let players = sessions
            .into_iter()
            .take(limit)
            .map(|(&session_id, session)| {
                let context = ScriptPlayerContext::try_new(
                    session.uuid.to_string(),
                    &session.name,
                    session.script_operator,
                    session.pose.x,
                    session.pose.y,
                    session.pose.z,
                )?;
                ScriptOnlinePlayerSnapshot::try_new(
                    mc_script::ScriptPlayerId::new(session_id),
                    context,
                    &session.dimension,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((players, truncated))
    }
}
