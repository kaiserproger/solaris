use mc_script::{AdmittedScriptCommand, ScriptDtoError};

use crate::loader::{encode_loader_ui, loader_ui_channel};
use crate::{LoaderContentKind, LoaderManifest, LoaderPermission};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptClientUiRouteError {
    InvalidCommand(ScriptDtoError),
    PluginHasNoEligibleUiBundle,
    UiNotOwned,
    PlayerUnavailable,
}

pub(super) fn plugin_has_ui_bundle(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.bundles.iter().any(|bundle| {
        bundle.owner == plugin_id
            && bundle.content.contains(&LoaderContentKind::Ui)
            && bundle.permissions.contains(&LoaderPermission::PresentUi)
    })
}

pub(super) fn ui_is_owned(plugin_id: &str, ui_id: &str) -> bool {
    ui_id
        .strip_prefix(plugin_id)
        .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
}

impl SessionRegistry {
    pub(crate) fn route_script_client_ui_command(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientUiRouteError> {
        let (plugin, player_id, presentation) = admitted
            .into_present_client_ui()
            .map_err(ScriptClientUiRouteError::InvalidCommand)?;
        let Some(manifest) = manifest else {
            return Err(ScriptClientUiRouteError::PluginHasNoEligibleUiBundle);
        };
        if !plugin_has_ui_bundle(manifest, plugin.plugin_id()) {
            return Err(ScriptClientUiRouteError::PluginHasNoEligibleUiBundle);
        }
        if !ui_is_owned(plugin.plugin_id(), presentation.ui_id()) {
            return Err(ScriptClientUiRouteError::UiNotOwned);
        }
        let recipient = {
            let inner = self.lock_inner("route script client UI");
            let session = inner
                .sessions
                .get(&player_id.value())
                .filter(|session| session.loader_session.is_some() && !session.tx.is_closed())
                .ok_or(ScriptClientUiRouteError::PlayerUnavailable)?;
            ordered_session_recipient(player_id.value(), session)
        };
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::CustomPayload {
                channel: loader_ui_channel().clone(),
                payload: encode_loader_ui(&presentation),
            },
        );
        Ok(())
    }
}
