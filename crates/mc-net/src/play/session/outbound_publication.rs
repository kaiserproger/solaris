use std::sync::Arc;

use mc_domain::GameMode;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{PlayerInfoEntry, PlayerInfoRemove, PlayerInfoUpdate};

use super::entity_lifecycle::NaturalMobDespawnOutcome;
use super::outbound::{
    OutboundCommand, SessionRecipient, VisibilityDispatch, dispatch_visibility_commands,
};
use super::visibility::{session_recipients, session_snapshot, visibility_dispatches};
use super::{SessionId, SessionRegistry};
use crate::play::wire_entities::player_info_entry;

impl SessionRegistry {
    pub(crate) fn publish_natural_mob_despawn(&self, outcome: NaturalMobDespawnOutcome) {
        dispatch_visibility_commands(outcome.dispatches);
    }

    pub(crate) fn disconnect_player(&self, player_id: u64, reason: String) -> bool {
        let dispatch = {
            let inner = self.lock_inner("extension disconnect player");
            inner
                .sessions
                .get(&player_id)
                .map(|session| VisibilityDispatch {
                    recipient: SessionRecipient::unordered(
                        player_id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    ),
                    command: OutboundCommand::DisconnectPlayer { reason },
                })
        };
        let Some(dispatch) = dispatch else {
            return false;
        };
        dispatch_visibility_commands(vec![dispatch]);
        true
    }

    pub(crate) fn send_custom_payload(
        &self,
        player_id: u64,
        channel: Identifier,
        payload: Vec<u8>,
    ) -> bool {
        let dispatch = {
            let inner = self.lock_inner("extension custom payload");
            inner
                .sessions
                .get(&player_id)
                .map(|session| VisibilityDispatch {
                    recipient: SessionRecipient::unordered(
                        player_id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    ),
                    command: OutboundCommand::CustomPayload { channel, payload },
                })
        };
        let Some(dispatch) = dispatch else {
            return false;
        };
        dispatch_visibility_commands(vec![dispatch]);
        true
    }

    pub(in crate::play) fn broadcast_system_chat(
        &self,
        message: String,
    ) -> Vec<VisibilityDispatch> {
        let recipients = {
            let inner = self.lock_inner("broadcast system chat");
            session_recipients(&inner, inner.sessions.keys().copied().collect::<Vec<_>>())
        };
        visibility_dispatches(recipients, || OutboundCommand::SystemChat {
            message: message.clone(),
        })
    }

    pub(crate) fn send_script_system_chat(&self, player_id: u64, message: String) -> bool {
        let dispatch = {
            let inner = self.lock_inner("script system chat");
            inner
                .sessions
                .get(&player_id)
                .map(|session| VisibilityDispatch {
                    recipient: SessionRecipient::unordered(
                        player_id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    ),
                    command: OutboundCommand::SystemChat { message },
                })
        };
        let Some(dispatch) = dispatch else {
            return false;
        };
        dispatch_visibility_commands(vec![dispatch]);
        true
    }

    pub(crate) fn broadcast_script_system_chat(&self, message: String) {
        dispatch_visibility_commands(self.broadcast_system_chat(message));
    }

    /// Snapshot the current online roster as tab-list entries, sorted by
    /// name so login bursts are deterministic.
    pub(in crate::play) fn tab_list_roster(&self) -> Vec<PlayerInfoEntry> {
        let inner = self.lock_inner("snapshot tab list roster");
        let mut entries: Vec<PlayerInfoEntry> = inner
            .sessions
            .iter()
            .map(|(id, session)| player_info_entry(&session_snapshot(*id, session)))
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name).then(a.profile_id.cmp(&b.profile_id)));
        entries
    }

    /// Snapshot one session's tab-list entry, if it is still online.
    pub(in crate::play) fn tab_list_entry(&self, id: SessionId) -> Option<PlayerInfoEntry> {
        let inner = self.lock_inner("snapshot tab list entry");
        inner
            .sessions
            .get(&id)
            .map(|session| player_info_entry(&session_snapshot(id, session)))
    }

    /// Broadcast a player-info update to every online session.
    pub(in crate::play) fn broadcast_player_info_update(
        &self,
        update: PlayerInfoUpdate,
    ) -> Vec<VisibilityDispatch> {
        let recipients = {
            let inner = self.lock_inner("broadcast player info update");
            session_recipients(&inner, inner.sessions.keys().copied().collect::<Vec<_>>())
        };
        visibility_dispatches(recipients, || OutboundCommand::PlayerInfo(update.clone()))
    }

    /// Build the removal notice for a session that is still registered;
    /// call before unregistering so the UUID is still available.
    pub(in crate::play) fn player_info_remove_for(
        &self,
        id: SessionId,
    ) -> Option<PlayerInfoRemove> {
        let inner = self.lock_inner("snapshot player info remove");
        inner.sessions.get(&id).map(|session| PlayerInfoRemove {
            profile_ids: vec![session.uuid],
        })
    }

    /// Broadcast a player-info removal to every online session.
    pub(in crate::play) fn broadcast_player_info_remove(
        &self,
        remove: PlayerInfoRemove,
    ) -> Vec<VisibilityDispatch> {
        let recipients = {
            let inner = self.lock_inner("broadcast player info remove");
            session_recipients(&inner, inner.sessions.keys().copied().collect::<Vec<_>>())
        };
        visibility_dispatches(recipients, || {
            OutboundCommand::PlayerInfoRemove(remove.clone())
        })
    }

    /// Record a player's current game mode for tab-list entries.
    pub(in crate::play) fn update_player_game_mode(&self, id: SessionId, game_mode: GameMode) {
        let mut inner = self.lock_inner("update player game mode");
        if let Some(session) = inner.sessions.get_mut(&id) {
            session.game_mode = game_mode;
        }
    }

    pub(in crate::play) fn debug_outbound_pressure_dispatches(
        &self,
        id: SessionId,
        count: usize,
    ) -> Vec<VisibilityDispatch> {
        let recipient = {
            let inner = self.lock_inner("debug outbound pressure dispatches");
            inner.sessions.get(&id).map(|session| {
                (
                    session.entity_id,
                    SessionRecipient::unordered(
                        id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    ),
                )
            })
        };
        let Some((entity_id, recipient)) = recipient else {
            return Vec::new();
        };
        (0..count)
            .map(|_| VisibilityDispatch {
                recipient: recipient.clone(),
                command: OutboundCommand::AnimatePlayer { entity_id },
            })
            .collect()
    }
}
