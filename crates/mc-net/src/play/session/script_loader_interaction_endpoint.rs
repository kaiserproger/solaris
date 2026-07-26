use mc_script::{ScriptDtoError, ScriptEvent, ScriptPlayerId, ScriptQueueError};

use crate::loader::LoaderInteractionAction;
use crate::server::ScriptEventSink;
use crate::{LoaderContentKind, LoaderManifest, LoaderPermission};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoaderInteractionRouteError {
    LoaderNotAcknowledged,
    RuntimeUnavailable,
    Malformed,
    PluginHasNoEligibleInteractionBundle,
    InteractionNotOwned,
    InvalidEvent(ScriptDtoError),
    Queue(ScriptQueueError),
}

pub(super) fn plugin_has_interaction_bundle(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.bundles.iter().any(|bundle| {
        bundle.owner == plugin_id
            && bundle.content.contains(&LoaderContentKind::Interactions)
            && bundle
                .permissions
                .contains(&LoaderPermission::SendInteractions)
    })
}

pub(in crate::play) async fn route_client_loader_interaction(
    scripts: Option<&ScriptEventSink>,
    player_id: u64,
    loader_eligible: bool,
    manifest: Option<&LoaderManifest>,
    payload: &[u8],
) -> Result<(), LoaderInteractionRouteError> {
    if !loader_eligible {
        return Err(LoaderInteractionRouteError::LoaderNotAcknowledged);
    }
    let scripts = scripts.ok_or(LoaderInteractionRouteError::RuntimeUnavailable)?;
    let manifest =
        manifest.ok_or(LoaderInteractionRouteError::PluginHasNoEligibleInteractionBundle)?;
    let action = LoaderInteractionAction::decode(payload)
        .map_err(|_| LoaderInteractionRouteError::Malformed)?;
    let (owner, local_id) = action
        .interaction_id
        .split_once(':')
        .ok_or(LoaderInteractionRouteError::InteractionNotOwned)?;
    if owner.is_empty() || local_id.is_empty() {
        return Err(LoaderInteractionRouteError::InteractionNotOwned);
    }
    if !plugin_has_interaction_bundle(manifest, owner) {
        return Err(LoaderInteractionRouteError::PluginHasNoEligibleInteractionBundle);
    }
    let event = ScriptEvent::loader_interaction(
        owner,
        ScriptPlayerId::new(player_id),
        &action.interaction_id,
        &action.payload,
    )
    .map_err(LoaderInteractionRouteError::InvalidEvent)?;
    scripts
        .enqueue_targeted_event(event)
        .await
        .map_err(LoaderInteractionRouteError::Queue)
}
