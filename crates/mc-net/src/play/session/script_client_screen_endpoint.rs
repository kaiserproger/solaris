use mc_script::{AdmittedScriptCommand, ScriptDtoError};

use crate::loader::{encode_loader_open_screen, loader_open_screen_channel};
use crate::{LoaderContentKind, LoaderManifest, LoaderPermission};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptClientScreenRouteError {
    InvalidCommand(ScriptDtoError),
    PluginHasNoEligibleScreenBundle,
    ScreenNotOwned,
    PlayerUnavailable,
}

pub(super) fn plugin_has_screen_bundle(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.bundles.iter().any(|bundle| {
        bundle.owner == plugin_id
            && bundle.content.contains(&LoaderContentKind::Screens)
            && bundle.permissions.contains(&LoaderPermission::OpenScreens)
    })
}

pub(super) fn screen_is_owned(plugin_id: &str, screen_id: &str) -> bool {
    screen_id
        .strip_prefix(plugin_id)
        .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
}

impl SessionRegistry {
    pub(crate) fn route_script_client_screen_command(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientScreenRouteError> {
        let plugin_id = admitted.plugin_id().to_owned();
        let Some(manifest) = manifest else {
            return Err(ScriptClientScreenRouteError::PluginHasNoEligibleScreenBundle);
        };
        if !plugin_has_screen_bundle(manifest, &plugin_id) {
            return Err(ScriptClientScreenRouteError::PluginHasNoEligibleScreenBundle);
        }
        let (_, player_id, screen_id) = admitted
            .into_open_client_screen()
            .map_err(ScriptClientScreenRouteError::InvalidCommand)?;
        if !screen_is_owned(&plugin_id, &screen_id) {
            return Err(ScriptClientScreenRouteError::ScreenNotOwned);
        }
        let payload = encode_loader_open_screen(&screen_id)
            .ok_or(ScriptClientScreenRouteError::ScreenNotOwned)?;
        let recipient = {
            let inner = self.lock_inner("route script client screen");
            let session = inner
                .sessions
                .get(&player_id.value())
                .filter(|session| session.loader_session.is_some() && !session.tx.is_closed())
                .ok_or(ScriptClientScreenRouteError::PlayerUnavailable)?;
            ordered_session_recipient(player_id.value(), session)
        };
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::CustomPayload {
                channel: loader_open_screen_channel().clone(),
                payload,
            },
        );
        Ok(())
    }
}
