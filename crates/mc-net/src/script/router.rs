use mc_script::{AdmittedScriptCommand, ScriptCommand, ScriptPluginStorageFailure};
use tracing::debug;

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use super::storage::PluginStorageHandle;
use crate::RuntimeControlHandle;
use crate::chunk_pipeline::ChunkPipelineResources;
use crate::play;
use crate::server::{
    ScriptEventSink, ServerConfig, ShutdownHandle, execute_console_command,
    resolve_script_entity_type,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptRouterExit {
    Continue,
    Stop,
}

#[derive(Clone, Copy)]
pub(crate) struct ScriptRouterContext<'a> {
    pub(crate) config: &'a ServerConfig,
    pub(crate) sessions: &'a play::SessionRegistry,
    pub(crate) runtime_control: Option<&'a RuntimeControlHandle>,
    pub(crate) simulation: &'a play::SimulationHandle,
    pub(crate) chunk_pipeline_resources: &'a ChunkPipelineResources,
    pub(crate) shutdown: &'a ShutdownHandle,
}

pub(crate) struct ScriptRouter {
    scripts: ScriptEventSink,
    storage: Option<PluginStorageHandle>,
}

impl ScriptRouter {
    pub(crate) fn new(scripts: ScriptEventSink, storage: Option<PluginStorageHandle>) -> Self {
        Self { scripts, storage }
    }

    pub(crate) fn context<'a>(
        config: &'a ServerConfig,
        sessions: &'a play::SessionRegistry,
        runtime_control: Option<&'a RuntimeControlHandle>,
        simulation: &'a play::SimulationHandle,
        chunk_pipeline_resources: &'a ChunkPipelineResources,
        shutdown: &'a ShutdownHandle,
    ) -> ScriptRouterContext<'a> {
        ScriptRouterContext {
            config,
            sessions,
            runtime_control,
            simulation,
            chunk_pipeline_resources,
            shutdown,
        }
    }

    pub(crate) async fn wait_for_storage_stop(&self) {
        match self.storage.as_ref() {
            Some(storage) => storage.wait_stopped().await,
            None => std::future::pending::<()>().await,
        }
    }

    pub(crate) async fn route(
        &self,
        command: ScriptCommand,
        context: ScriptRouterContext<'_>,
    ) -> ScriptRouterExit {
        match command {
            ScriptCommand::HostAttached { .. } => match self.scripts.accept_host_command(command) {
                Ok(admitted) => self.route_admitted(admitted, context).await,
                Err(error) => {
                    debug!(?error, "script host command admission rejected");
                    ScriptRouterExit::Continue
                }
            },
            ScriptCommand::SendChatMessage { player_id, message } => {
                send_chat(context.sessions, player_id.value(), message);
                ScriptRouterExit::Continue
            }
            ScriptCommand::BroadcastChatMessage { message } => {
                context.sessions.broadcast_script_system_chat(message);
                ScriptRouterExit::Continue
            }
            ScriptCommand::DisconnectPlayer { player_id, reason } => {
                disconnect(context.sessions, player_id.value(), reason);
                ScriptRouterExit::Continue
            }
            ScriptCommand::RunConsoleCommand { .. }
            | ScriptCommand::SpawnEntity { .. }
            | ScriptCommand::PluginStorageGet { .. }
            | ScriptCommand::PluginStorageCompareAndSwap { .. }
            | ScriptCommand::PluginStorageDelete { .. }
            | ScriptCommand::OpenInventoryMenu { .. }
            | ScriptCommand::CloseInventoryMenu { .. }
            | ScriptCommand::InventoryStorageTransaction { .. }
            | ScriptCommand::UpsertZone { .. }
            | ScriptCommand::RemoveZone { .. }
            | ScriptCommand::UpsertColony { .. }
            | ScriptCommand::RequestVillagerBinding { .. } => {
                debug!("unattested privileged script command rejected");
                ScriptRouterExit::Continue
            }
            _ => {
                debug!("unknown direct script command rejected");
                ScriptRouterExit::Continue
            }
        }
    }

    async fn route_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        context: ScriptRouterContext<'_>,
    ) -> ScriptRouterExit {
        if matches!(
            admitted.request(),
            ScriptCommand::PluginStorageGet { .. }
                | ScriptCommand::PluginStorageCompareAndSwap { .. }
                | ScriptCommand::PluginStorageDelete { .. }
        ) {
            return self
                .route_storage_admitted(admitted, context.shutdown)
                .await;
        }

        match admitted.request() {
            ScriptCommand::SendChatMessage { player_id, message } => {
                send_chat(context.sessions, player_id.value(), message.clone());
                ScriptRouterExit::Continue
            }
            ScriptCommand::BroadcastChatMessage { message } => {
                context
                    .sessions
                    .broadcast_script_system_chat(message.clone());
                ScriptRouterExit::Continue
            }
            ScriptCommand::DisconnectPlayer { player_id, reason } => {
                disconnect(context.sessions, player_id.value(), reason.clone());
                ScriptRouterExit::Continue
            }
            ScriptCommand::RunConsoleCommand { command } => {
                execute_console_command(
                    command,
                    "script save-all",
                    "script stop",
                    context.config,
                    context.sessions,
                    context.runtime_control,
                    context.simulation,
                    context.chunk_pipeline_resources,
                )
                .await;
                ScriptRouterExit::Continue
            }
            ScriptCommand::SpawnEntity {
                actor,
                entity_type,
                position,
            } => {
                let Some(entity_type_id) = resolve_script_entity_type(context.config, entity_type)
                else {
                    debug!(%entity_type, "script entity spawn requested an unknown entity type");
                    return ScriptRouterExit::Continue;
                };
                if let Err(error) = context
                    .simulation
                    .spawn_script_entity(
                        actor.value(),
                        entity_type_id,
                        entity_type.clone(),
                        mc_entity::Vec3::new(position.x(), position.y(), position.z()),
                    )
                    .await
                {
                    debug!(
                        ?error,
                        player_id = actor.value(),
                        "script entity spawn rejected"
                    );
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::OpenInventoryMenu { .. } | ScriptCommand::CloseInventoryMenu { .. } => {
                if let Err(error) = context.sessions.route_script_menu_command(admitted) {
                    debug!(?error, "admitted script menu command rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::InventoryStorageTransaction { .. }
            | ScriptCommand::UpsertZone { .. }
            | ScriptCommand::RemoveZone { .. }
            | ScriptCommand::UpsertColony { .. }
            | ScriptCommand::RequestVillagerBinding { .. } => {
                debug!("admitted script command has no production adapter in slice A");
                ScriptRouterExit::Continue
            }
            ScriptCommand::HostAttached { .. }
            | ScriptCommand::PluginStorageGet { .. }
            | ScriptCommand::PluginStorageCompareAndSwap { .. }
            | ScriptCommand::PluginStorageDelete { .. } => {
                debug!("invalid admitted script command rejected");
                ScriptRouterExit::Continue
            }
            _ => {
                debug!("unknown admitted script command rejected");
                ScriptRouterExit::Continue
            }
        }
    }

    pub(super) async fn route_storage_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        shutdown: &ShutdownHandle,
    ) -> ScriptRouterExit {
        let Some(storage) = self.storage.as_ref() else {
            debug!(
                plugin = admitted.plugin_id(),
                "plugin storage unavailable; publishing explicit failure"
            );
            return self
                .publish_storage_failure(admitted, ScriptPluginStorageFailure::Unavailable)
                .await;
        };
        match storage.enqueue(admitted, shutdown).await {
            Ok(()) => ScriptRouterExit::Continue,
            Err(admitted) if storage.failed() => {
                storage.wait_stopped().await;
                self.publish_storage_failure(admitted, ScriptPluginStorageFailure::DurabilityFailed)
                    .await
            }
            Err(_) => ScriptRouterExit::Stop,
        }
    }

    async fn publish_storage_failure(
        &self,
        admitted: AdmittedScriptCommand,
        failure: ScriptPluginStorageFailure,
    ) -> ScriptRouterExit {
        let event = match admitted.plugin_storage_failure_result(failure) {
            Ok(event) => event,
            Err(error) => {
                debug!(
                    ?error,
                    "admitted storage failure result construction rejected"
                );
                return ScriptRouterExit::Stop;
            }
        };
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => ScriptRouterExit::Continue,
            TargetedEventDelivery::Closed | TargetedEventDelivery::Shutdown => {
                ScriptRouterExit::Stop
            }
        }
    }
}

fn send_chat(sessions: &play::SessionRegistry, player_id: u64, message: String) {
    if !sessions.send_script_system_chat(player_id, message) {
        debug!(player_id, "script chat command targeted unknown player");
    }
}

fn disconnect(sessions: &play::SessionRegistry, player_id: u64, reason: String) {
    if !sessions.disconnect_player(player_id, reason) {
        debug!(
            player_id,
            "script disconnect command targeted unknown player"
        );
    }
}
