use mc_script::{ScriptPlayerTeleportFailure, ScriptPlayerTeleportRequest, ScriptPosition};
use tokio::sync::oneshot;

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug)]
pub(in crate::play) struct ScriptPlayerTeleportCommand {
    pub(in crate::play) position: ScriptPosition,
    completion: Option<oneshot::Sender<Result<(), ScriptPlayerTeleportFailure>>>,
}

impl ScriptPlayerTeleportCommand {
    pub(in crate::play) fn complete(mut self, result: Result<(), ScriptPlayerTeleportFailure>) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(result);
        }
    }

    pub(in crate::play) fn into_owner_completion(
        mut self,
    ) -> (ScriptPosition, ScriptPlayerTeleportCompletion) {
        (
            self.position,
            ScriptPlayerTeleportCompletion {
                completion: self.completion.take(),
            },
        )
    }
}

#[derive(Debug)]
#[must_use = "the simulation owner must publish the teleport commit outcome"]
pub(in crate::play) struct ScriptPlayerTeleportCompletion {
    completion: Option<oneshot::Sender<Result<(), ScriptPlayerTeleportFailure>>>,
}

impl ScriptPlayerTeleportCompletion {
    pub(in crate::play) fn complete(mut self, result: Result<(), ScriptPlayerTeleportFailure>) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(result);
        }
    }
}

impl Drop for ScriptPlayerTeleportCompletion {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(Err(ScriptPlayerTeleportFailure::PlayerUnavailable));
        }
    }
}

impl SessionRegistry {
    /// Push one teleport request to the exact session owner and await its commit result.
    pub(crate) async fn route_script_player_teleport(
        &self,
        request: ScriptPlayerTeleportRequest,
    ) -> Result<(), ScriptPlayerTeleportFailure> {
        let recipient = {
            let inner = self.lock_inner("route script player teleport");
            let Some(session) = inner.sessions.get(&request.player_id().value()) else {
                return Err(ScriptPlayerTeleportFailure::PlayerUnavailable);
            };
            if session.tx.is_closed() {
                return Err(ScriptPlayerTeleportFailure::PlayerUnavailable);
            }
            ordered_session_recipient(request.player_id().value(), session)
        };
        let (completion, result) = oneshot::channel();
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::ScriptPlayerTeleport(ScriptPlayerTeleportCommand {
                position: request.position(),
                completion: Some(completion),
            }),
        );
        result
            .await
            .unwrap_or(Err(ScriptPlayerTeleportFailure::PlayerUnavailable))
    }
}
