use std::sync::Arc;

use mc_protocol::codec::Identifier;

use super::entity_lifecycle::NaturalMobDespawnOutcome;
use super::outbound::{
    OutboundCommand, SessionRecipient, VisibilityDispatch, dispatch_visibility_commands,
};
use super::visibility::{session_recipients, visibility_dispatches};
use super::{SessionId, SessionRegistry};

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
