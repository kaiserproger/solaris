use mc_script::{AdmittedScriptCommand, ScriptCommand, ScriptDtoError};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TeleportAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    PublicationClosed,
}

pub(crate) struct PluginTeleportAdapter {
    scripts: ScriptEventSink,
}

impl PluginTeleportAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self { scripts }
    }

    pub(crate) async fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<(), TeleportAdapterError> {
        let ScriptCommand::TeleportPlayer { request } = admitted.request() else {
            return Err(TeleportAdapterError::WrongCommand);
        };
        let failure = sessions
            .route_script_player_teleport(request.clone())
            .await
            .err();
        let event = admitted
            .player_teleport_result(failure)
            .map_err(TeleportAdapterError::InvalidResult)?;
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(()),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(TeleportAdapterError::PublicationClosed)
            }
        }
    }
}
