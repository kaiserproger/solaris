//! Routing of the schema-2 view lifecycle from plugins and of wire-3 client
//! view requests back to the owning plugin.
//!
//! Every path re-reads the plugin's granted permission pair and the live
//! registry: a revoked permission, a stale revision, a closed instance, a
//! substituted field or a foreign instance never authorizes an action.

use mc_script::{
    AdmittedScriptCommand, ScriptDtoError, ScriptEvent, ScriptPlayerId, ScriptQueueError,
};
use tracing::debug;

use crate::loader::{
    LoaderViewClientRequest, LoaderViewWireMessage, decode_loader_view_client_request,
    encode_loader_view_message, loader_view_channel,
};
use crate::{LoaderContentKind, LoaderManifest, LoaderPermission};

use super::SessionRegistry;
use super::loader_views::LoaderViewError;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptClientViewRouteError {
    InvalidCommand(ScriptDtoError),
    PluginHasNoEligibleViewBundle,
    PlayerUnavailable,
    Registry(LoaderViewError),
}

impl From<LoaderViewError> for ScriptClientViewRouteError {
    fn from(error: LoaderViewError) -> Self {
        Self::Registry(error)
    }
}

/// One targeted result event the router must enqueue for the calling plugin.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptClientViewEvent {
    pub(crate) event: ScriptEvent,
}

/// One decoded wire-3 client request with its resolved owner.
#[derive(Debug, PartialEq)]
pub(crate) enum ClientLoaderViewRoute {
    /// A targeted action event for the owning plugin.
    Action(ScriptEvent),
    /// The view request was refused or resolved to no owner; nothing is sent.
    Refused(LoaderViewRouteError),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoaderViewRouteError {
    LoaderNotAcknowledged,
    RuntimeUnavailable,
    Malformed,
    NoOwner,
    Registry(LoaderViewError),
    InvalidEvent(ScriptDtoError),
    Queue(ScriptQueueError),
}

fn plugin_grants_views(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.plugin_grants(
        plugin_id,
        LoaderContentKind::Views,
        LoaderPermission::PresentViews,
    )
}

fn plugin_grants_view_actions(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.plugin_grants(
        plugin_id,
        LoaderContentKind::ViewActions,
        LoaderPermission::SendViewActions,
    )
}

fn plugin_grants_world_selection(manifest: &LoaderManifest, plugin_id: &str) -> bool {
    manifest.plugin_grants(
        plugin_id,
        LoaderContentKind::WorldSelection,
        LoaderPermission::SendWorldSelection,
    )
}

/// True when `view_id` is namespaced to `plugin_id`.
fn view_is_owned(plugin_id: &str, view_id: &str) -> bool {
    view_id
        .strip_prefix(plugin_id)
        .is_some_and(|suffix| suffix.starts_with(':') && suffix.len() > 1)
}

impl SessionRegistry {
    /// Declare the view kinds one plugin may be asked to open. Production
    /// population comes from a loaded plugin's verified bundle; no shipped
    /// package declares `[client]` yet, so this stays empty by default.
    #[allow(dead_code)]
    pub(crate) fn declare_loader_view_kinds(
        &self,
        owner: &str,
        kinds: impl IntoIterator<Item = mc_script::ScriptClientViewRequestKind>,
    ) {
        let mut inner = self.lock_inner("declare loader view kinds");
        for kind in kinds {
            inner.loader_views.declare_owner_kind(owner, kind);
        }
    }

    /// Drop every view and selection context of one plugin (permission
    /// revocation). No shipped package declares `[client]` yet, so the
    /// production revocation hook has nothing to revoke; the contract path is
    /// implemented and unit-tested through the registry.
    #[allow(dead_code)]
    pub(crate) fn revoke_loader_views(&self, owner: &str) {
        let mut inner = self.lock_inner("revoke loader views");
        inner.loader_views.revoke_owner(owner);
    }

    /// Resolve which plugin (if any) must be told about one key-driven request.
    /// No declared view of that kind, an ambiguous owner, or a missing
    /// present_views grant all resolve to `None`, so nothing opens and no event
    /// is published.
    pub(crate) fn resolve_view_request_owner(
        &self,
        request_kind: mc_script::ScriptClientViewRequestKind,
        manifest: Option<&LoaderManifest>,
    ) -> Option<String> {
        let owner = {
            let inner = self.lock_inner("resolve loader view request owner");
            inner.loader_views.owner_for_kind(request_kind)
        }?;
        manifest
            .filter(|manifest| plugin_grants_views(manifest, &owner))
            .map(|_| owner)
    }

    fn deliver_view_payload(
        &self,
        player_id: u64,
        payload: Vec<u8>,
    ) -> Result<(), ScriptClientViewRouteError> {
        let recipient = {
            let inner = self.lock_inner("route script client view");
            let session = inner
                .sessions
                .get(&player_id)
                .filter(|session| session.loader_session.is_some() && !session.tx.is_closed())
                .ok_or(ScriptClientViewRouteError::PlayerUnavailable)?;
            ordered_session_recipient(player_id, session)
        };
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::CustomPayload {
                channel: loader_view_channel().clone(),
                payload,
            },
        );
        Ok(())
    }

    pub(crate) fn route_script_open_client_view(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<ScriptClientViewEvent, ScriptClientViewRouteError> {
        let (plugin, request) = admitted
            .into_open_client_view()
            .map_err(ScriptClientViewRouteError::InvalidCommand)?;
        let plugin_id = plugin.plugin_id().to_owned();
        let player_id = request.player_id().value();
        let request_id = request.request_id().to_owned();
        let refuse = |failure| {
            Ok(ScriptClientViewEvent {
                event: ScriptEvent::client_view_opened(
                    &plugin_id,
                    &request_id,
                    request.player_id(),
                    None,
                    None,
                    Some(failure),
                )
                .expect("validated view result event"),
            })
        };
        if !manifest.is_some_and(|manifest| plugin_grants_views(manifest, &plugin_id)) {
            return refuse(mc_script::ScriptClientViewFailure::Refused);
        }
        if !view_is_owned(&plugin_id, request.owned_view_id()) {
            return refuse(mc_script::ScriptClientViewFailure::Refused);
        }
        let opened = {
            let mut inner = self.lock_inner("route script open client view");
            inner.loader_views.open(&plugin_id, &request)
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(LoaderViewError::TooLarge) => {
                return refuse(mc_script::ScriptClientViewFailure::TooLarge);
            }
            Err(_) => return refuse(mc_script::ScriptClientViewFailure::Refused),
        };
        let payload = encode_loader_view_message(&LoaderViewWireMessage::Open {
            view_instance_id: &opened.instance_id,
            revision: opened.revision,
            view_id: request.owned_view_id(),
            title: request.owned_view_id(),
            model: request.model(),
        })
        .map_err(|_| ScriptClientViewRouteError::PlayerUnavailable)?;
        if let Err(error) = self.deliver_view_payload(player_id, payload) {
            let mut inner = self.lock_inner("rollback script open client view");
            inner
                .loader_views
                .close(None, player_id, &opened.instance_id)
                .ok();
            return Err(error);
        }
        Ok(ScriptClientViewEvent {
            event: ScriptEvent::client_view_opened(
                &plugin_id,
                &request_id,
                request.player_id(),
                Some(opened.instance_id),
                Some(opened.revision),
                None,
            )
            .expect("validated view result event"),
        })
    }

    pub(crate) fn route_script_present_client_view(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientViewRouteError> {
        let (plugin, request) = admitted
            .into_present_client_view()
            .map_err(ScriptClientViewRouteError::InvalidCommand)?;
        let plugin_id = plugin.plugin_id().to_owned();
        if !manifest.is_some_and(|manifest| plugin_grants_views(manifest, &plugin_id)) {
            return Err(ScriptClientViewRouteError::PluginHasNoEligibleViewBundle);
        }
        let player_id = request.player_id().value();
        let current_tick = self.simulation_tick();
        let (instance_id, revision, model) = {
            let mut inner = self.lock_inner("route script present client view");
            let revision = inner
                .loader_views
                .present(&plugin_id, &request, current_tick)?;
            let instance = inner
                .loader_views
                .instance(request.view_instance_id())
                .expect("presented instance exists");
            (
                instance.instance_id().to_owned(),
                revision,
                instance.model().clone(),
            )
        };
        let payload = encode_loader_view_message(&LoaderViewWireMessage::Present {
            view_instance_id: &instance_id,
            revision,
            model: &model,
        })
        .map_err(|_| ScriptClientViewRouteError::PlayerUnavailable)?;
        self.deliver_view_payload(player_id, payload)
    }

    pub(crate) fn route_script_close_client_view(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientViewRouteError> {
        let (plugin, player_id, instance_id) = admitted
            .into_close_client_view()
            .map_err(ScriptClientViewRouteError::InvalidCommand)?;
        let plugin_id = plugin.plugin_id().to_owned();
        if !manifest.is_some_and(|manifest| plugin_grants_views(manifest, &plugin_id)) {
            return Err(ScriptClientViewRouteError::PluginHasNoEligibleViewBundle);
        }
        {
            let mut inner = self.lock_inner("route script close client view");
            inner
                .loader_views
                .close(Some(&plugin_id), player_id.value(), &instance_id)?;
        }
        let payload = encode_loader_view_message(&LoaderViewWireMessage::Close {
            view_instance_id: &instance_id,
        })
        .map_err(|_| ScriptClientViewRouteError::PlayerUnavailable)?;
        self.deliver_view_payload(player_id.value(), payload)
    }

    pub(crate) fn route_script_begin_client_selection(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<ScriptClientViewEvent, ScriptClientViewRouteError> {
        let (plugin, selection) = admitted
            .into_begin_client_selection()
            .map_err(ScriptClientViewRouteError::InvalidCommand)?;
        let plugin_id = plugin.plugin_id().to_owned();
        let request_id = selection.request_id().to_owned();
        let player_id = selection.player_id();
        let view_instance_id = selection.view_instance_id().to_owned();
        let view_revision = selection.view_revision();
        let action_id = selection.action_id().to_owned();
        let constraints = selection.constraints().clone();
        let refuse = |failure| {
            Ok(ScriptClientViewEvent {
                event: ScriptEvent::client_selection_started(
                    &plugin_id,
                    &request_id,
                    player_id,
                    None,
                    None,
                    Some(failure),
                )
                .expect("validated selection result event"),
            })
        };
        let grants = manifest.is_some_and(|manifest| {
            plugin_grants_views(manifest, &plugin_id)
                && plugin_grants_world_selection(manifest, &plugin_id)
        });
        if !grants {
            return refuse(mc_script::ScriptClientViewFailure::Refused);
        }
        let current_tick = self.simulation_tick();
        let started = {
            let mut inner = self.lock_inner("route script begin client selection");
            inner.loader_views.begin_selection(
                &plugin_id,
                player_id.value(),
                &view_instance_id,
                view_revision,
                &action_id,
                &constraints,
                current_tick,
            )
        };
        let started = match started {
            Ok(started) => started,
            Err(LoaderViewError::StaleRevision) => {
                return refuse(mc_script::ScriptClientViewFailure::StaleRevision);
            }
            Err(LoaderViewError::UnknownInstance | LoaderViewError::ClosedInstance) => {
                return refuse(mc_script::ScriptClientViewFailure::UnknownInstance);
            }
            Err(_) => return refuse(mc_script::ScriptClientViewFailure::Refused),
        };
        Ok(ScriptClientViewEvent {
            event: ScriptEvent::client_selection_started(
                &plugin_id,
                &request_id,
                player_id,
                Some(started.context_id),
                Some(started.expires_at_tick),
                None,
            )
            .expect("validated selection result event"),
        })
    }

    pub(crate) fn route_script_cancel_client_selection(
        &self,
        admitted: AdmittedScriptCommand,
        manifest: Option<&LoaderManifest>,
    ) -> Result<(), ScriptClientViewRouteError> {
        let (plugin, player_id, context_id) = admitted
            .into_cancel_client_selection()
            .map_err(ScriptClientViewRouteError::InvalidCommand)?;
        let plugin_id = plugin.plugin_id().to_owned();
        if !manifest.is_some_and(|manifest| {
            plugin_grants_views(manifest, &plugin_id)
                && plugin_grants_world_selection(manifest, &plugin_id)
        }) {
            return Err(ScriptClientViewRouteError::PluginHasNoEligibleViewBundle);
        }
        let mut inner = self.lock_inner("route script cancel client selection");
        inner
            .loader_views
            .cancel_selection(player_id.value(), &context_id)?;
        Ok(())
    }
}

/// Route one client `view_request` / `view_action` / `cancel_selection` payload.
/// Refusals are silent: the client only learns the outcome from the owner.
pub(in crate::play) async fn route_client_loader_view_request(
    scripts: Option<&crate::server::ScriptEventSink>,
    sessions: &SessionRegistry,
    player_id: u64,
    loader_eligible: bool,
    manifest: Option<&LoaderManifest>,
    payload: &[u8],
) -> Result<ClientLoaderViewRoute, LoaderViewRouteError> {
    if !loader_eligible {
        return Err(LoaderViewRouteError::LoaderNotAcknowledged);
    }
    let scripts = scripts.ok_or(LoaderViewRouteError::RuntimeUnavailable)?;
    let manifest = manifest.ok_or(LoaderViewRouteError::NoOwner)?;
    let request =
        decode_loader_view_client_request(payload).map_err(|_| LoaderViewRouteError::Malformed)?;
    match request {
        LoaderViewClientRequest::ViewRequest { request_kind } => {
            let Some(owner) = sessions.resolve_view_request_owner(request_kind, Some(manifest))
            else {
                return Ok(ClientLoaderViewRoute::Refused(
                    LoaderViewRouteError::NoOwner,
                ));
            };
            let event = ScriptEvent::loader_view_request(
                &owner,
                ScriptPlayerId::new(player_id),
                request_kind,
            )
            .map_err(LoaderViewRouteError::InvalidEvent)?;
            scripts
                .enqueue_targeted_event(event.clone())
                .await
                .map_err(LoaderViewRouteError::Queue)?;
            Ok(ClientLoaderViewRoute::Action(event))
        }
        LoaderViewClientRequest::Action(action) => {
            let (owner, outcome) = {
                let mut inner = sessions.lock_inner("route client view action");
                let Some(owner) = inner
                    .loader_views
                    .instance(&action.view_instance_id)
                    .map(|instance| instance.owner().to_owned())
                else {
                    return Ok(ClientLoaderViewRoute::Refused(
                        LoaderViewRouteError::Registry(LoaderViewError::UnknownInstance),
                    ));
                };
                let grants = plugin_grants_view_actions(manifest, &owner)
                    && (action.selection_token.is_none()
                        || plugin_grants_world_selection(manifest, &owner));
                let current_tick = sessions.simulation_tick();
                let ingress = super::loader_views::IngressAction {
                    view_instance_id: action.view_instance_id.clone(),
                    view_revision: action.view_revision,
                    action_id: action.action_id.clone(),
                    action_sequence: action.action_sequence,
                    fields: action.fields.clone(),
                    selection_token: action.selection_token.clone(),
                };
                let outcome =
                    inner
                        .loader_views
                        .handle_action(player_id, &ingress, grants, current_tick);
                (owner, outcome)
            };
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    debug!(?error, player_id, "Loader view action rejected");
                    return Ok(ClientLoaderViewRoute::Refused(
                        LoaderViewRouteError::Registry(error),
                    ));
                }
            };
            if !outcome.deliver {
                // A replayed point or sequence produces no second effect.
                return Ok(ClientLoaderViewRoute::Refused(
                    LoaderViewRouteError::NoOwner,
                ));
            }
            let event = ScriptEvent::loader_view_action(
                &owner,
                ScriptPlayerId::new(player_id),
                &action.view_instance_id,
                action.view_revision,
                &action.action_id,
                action.action_sequence,
                action.fields.clone(),
                action.selection_token.clone(),
            )
            .map_err(LoaderViewRouteError::InvalidEvent)?;
            scripts
                .enqueue_targeted_event(event.clone())
                .await
                .map_err(LoaderViewRouteError::Queue)?;
            Ok(ClientLoaderViewRoute::Action(event))
        }
        LoaderViewClientRequest::CancelSelection {
            selection_context_id,
        } => {
            let mut inner = sessions.lock_inner("route client cancel selection");
            match inner
                .loader_views
                .cancel_selection(player_id, &selection_context_id)
            {
                Ok(()) => Ok(ClientLoaderViewRoute::Refused(
                    LoaderViewRouteError::NoOwner,
                )),
                Err(error) => Ok(ClientLoaderViewRoute::Refused(
                    LoaderViewRouteError::Registry(error),
                )),
            }
        }
    }
}
