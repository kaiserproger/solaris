//! The two directions of the script-facing player query.
//!
//! `list-online-players` advertises a stable identity per connected session;
//! resolving that identity back to the session it holds right now is the same
//! question asked the other way round, so both are answered here from the one
//! registry and cannot disagree: a session the snapshot omits resolves to
//! nobody, and a session it advertises resolves to exactly the id it carries.

use mc_script::{ScriptDtoError, ScriptOnlinePlayerSnapshot, ScriptPlayerContext};

use super::{SessionId, SessionRegistry};

impl SessionRegistry {
    /// The session that currently holds `identity`, when that player is
    /// connected.
    ///
    /// `identity` is the stable player identity the online-player snapshot
    /// advertises and every player context carries; any spelling of that uuid
    /// `Uuid::parse_str` accepts resolves, and the server advertises the
    /// hyphenated lowercase form. A username, a session id, or anything else
    /// that is not a uuid resolves nobody.
    ///
    /// A player who is offline is answered as absent rather than with the
    /// session they used to hold: like [`Self::script_online_players`], this
    /// counts only sessions whose outbound owner is still live, so a
    /// connection that has already ended resolves to nobody even before the
    /// registry is told to tear its session down.
    pub(crate) fn script_session_of_identity(&self, identity: &str) -> Option<SessionId> {
        let wanted = uuid::Uuid::parse_str(identity).ok()?;
        let inner = self.lock_inner("resolve scripted player identity");
        inner
            .sessions
            .iter()
            .find(|(_, session)| session.uuid == wanted && !session.tx.is_closed())
            .map(|(&session_id, _)| session_id)
    }

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
