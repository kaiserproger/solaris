use mc_script::{
    AdmittedScriptCommand, ScriptCommand, ScriptDtoError, ScriptEvent, ScriptInventoryMenu,
    ScriptPlayerId, ScriptPluginTarget,
};

use crate::server::ScriptEventSink;

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug)]
pub(in crate::play) struct ScriptMenuOpenRequest {
    pub(in crate::play) owner: ScriptPluginTarget,
    pub(in crate::play) player_id: ScriptPlayerId,
    pub(in crate::play) menu: ScriptInventoryMenu,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct ScriptMenuCloseRequest {
    pub(in crate::play) plugin_id: String,
    pub(in crate::play) player_id: ScriptPlayerId,
    pub(in crate::play) menu_id: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptMenuRouteError {
    WrongCommand,
    InvalidOpenCommand(ScriptDtoError),
    PlayerDisconnected,
}

pub(in crate::play) async fn publish_script_menu_click(
    scripts: Option<&ScriptEventSink>,
    event: ScriptEvent,
) -> bool {
    match scripts {
        Some(scripts) => scripts.enqueue_targeted_event(event).await.is_ok(),
        None => false,
    }
}

impl SessionRegistry {
    /// Routes one admitted open/close command to the target player's ordered,
    /// reliable session lane. The script router is the only remaining caller.
    #[allow(dead_code)]
    pub(crate) fn route_script_menu_command(
        &self,
        admitted: AdmittedScriptCommand,
    ) -> Result<(), ScriptMenuRouteError> {
        match admitted.request() {
            ScriptCommand::OpenInventoryMenu { .. } => {
                let (owner, player_id, menu) = admitted
                    .into_open_inventory_menu()
                    .map_err(ScriptMenuRouteError::InvalidOpenCommand)?;
                self.dispatch_script_menu_command(
                    player_id,
                    OutboundCommand::OpenScriptMenu(ScriptMenuOpenRequest {
                        owner,
                        player_id,
                        menu,
                    }),
                )
            }
            ScriptCommand::CloseInventoryMenu { player_id, menu_id } => {
                let request = ScriptMenuCloseRequest {
                    plugin_id: admitted.plugin_id().to_owned(),
                    player_id: *player_id,
                    menu_id: menu_id.clone(),
                };
                self.dispatch_script_menu_command(
                    request.player_id,
                    OutboundCommand::CloseScriptMenu(request),
                )
            }
            _ => Err(ScriptMenuRouteError::WrongCommand),
        }
    }

    fn dispatch_script_menu_command(
        &self,
        player_id: ScriptPlayerId,
        command: OutboundCommand,
    ) -> Result<(), ScriptMenuRouteError> {
        let recipient = {
            let inner = self.lock_inner("route script menu command");
            let session = inner
                .sessions
                .get(&player_id.value())
                .ok_or(ScriptMenuRouteError::PlayerDisconnected)?;
            if session.tx.is_closed() {
                return Err(ScriptMenuRouteError::PlayerDisconnected);
            }
            ordered_session_recipient(player_id.value(), session)
        };
        dispatch_visibility_command(&recipient, command);
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn dispatch_script_menu_close_for_test(
        &self,
        request: ScriptMenuCloseRequest,
    ) -> Result<(), ScriptMenuRouteError> {
        self.dispatch_script_menu_command(
            request.player_id,
            OutboundCommand::CloseScriptMenu(request),
        )
    }
}
