use mc_script::{AdmittedScriptCommand, ScriptCommand, ScriptDtoError};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PlayerQueryAdapterError {
    WrongCommand,
    InvalidResult(ScriptDtoError),
    PublicationClosed,
}

pub(crate) struct PluginPlayerQueryAdapter {
    scripts: ScriptEventSink,
}

impl PluginPlayerQueryAdapter {
    pub(crate) fn new(scripts: ScriptEventSink) -> Self {
        Self { scripts }
    }

    pub(crate) async fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &SessionRegistry,
    ) -> Result<(), PlayerQueryAdapterError> {
        let ScriptCommand::ListOnlinePlayers { request } = admitted.request() else {
            return Err(PlayerQueryAdapterError::WrongCommand);
        };
        let (players, truncated) = sessions
            .script_online_players(request.limit())
            .map_err(PlayerQueryAdapterError::InvalidResult)?;
        let event = admitted
            .online_players_result(players, truncated)
            .map_err(PlayerQueryAdapterError::InvalidResult)?;
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => Ok(()),
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                Err(PlayerQueryAdapterError::PublicationClosed)
            }
        }
    }
}
