use mc_script::{AdmittedScriptCommand, ScriptCommand, ScriptPluginStorageFailure};
use tracing::{debug, warn};

use super::colony::{ColonyAdapterError, PluginColonyAdapter};
use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use super::inventory::{InventoryAdapterError, PluginInventoryAdapter};
use super::player_query::{PlayerQueryAdapterError, PluginPlayerQueryAdapter};
use super::storage::{PluginStorageHandle, storage_failure_event};
use super::teleport::{PluginTeleportAdapter, TeleportAdapterError};
use super::zone::PluginZoneAdapter;
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
    inventories: PluginInventoryAdapter,
    storage: Option<PluginStorageHandle>,
    zones: PluginZoneAdapter,
    colonies: PluginColonyAdapter,
    teleports: PluginTeleportAdapter,
    player_queries: PluginPlayerQueryAdapter,
}

impl ScriptRouter {
    #[cfg(test)]
    pub(crate) fn new(scripts: ScriptEventSink, storage: Option<PluginStorageHandle>) -> Self {
        let zones = PluginZoneAdapter::new(scripts.clone());
        Self::new_with_zones(scripts, storage, zones)
    }

    pub(crate) fn new_with_zones(
        scripts: ScriptEventSink,
        storage: Option<PluginStorageHandle>,
        zones: PluginZoneAdapter,
    ) -> Self {
        let inventories = PluginInventoryAdapter::new(scripts.clone());
        let colonies = PluginColonyAdapter::new(scripts.clone());
        let teleports = PluginTeleportAdapter::new(scripts.clone());
        let player_queries = PluginPlayerQueryAdapter::new(scripts.clone());
        Self {
            scripts,
            inventories,
            storage,
            zones,
            colonies,
            teleports,
            player_queries,
        }
    }

    pub(crate) fn zones(&self) -> PluginZoneAdapter {
        self.zones.clone()
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
            | ScriptCommand::PlayerInventoryTransaction { .. }
            | ScriptCommand::PlaceLoaderBlock { .. }
            | ScriptCommand::GrantLoaderBlockItem { .. }
            | ScriptCommand::UpsertZone { .. }
            | ScriptCommand::RemoveZone { .. }
            | ScriptCommand::UpsertColony { .. }
            | ScriptCommand::RequestVillagerBinding { .. }
            | ScriptCommand::SetVillagerOrder { .. }
            | ScriptCommand::TeleportPlayer { .. }
            | ScriptCommand::ListOnlinePlayers { .. } => {
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
                | ScriptCommand::InventoryStorageTransaction { .. }
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
            ScriptCommand::OpenClientScreen { .. } => {
                if let Err(error) = context.sessions.route_script_client_screen_command(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    debug!(?error, "admitted client screen command rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::PlaceLoaderBlock { block_id, x, y, z } => {
                let Some(manifest) = context.config.loader_manifest.as_deref() else {
                    debug!("Loader block placement rejected without an active manifest");
                    return ScriptRouterExit::Continue;
                };
                let Some(state) = manifest.world_block_state(
                    admitted.plugin_id(),
                    block_id,
                    &context.config.blocks,
                ) else {
                    debug!(
                        plugin = admitted.plugin_id(),
                        %block_id,
                        "Loader block placement rejected outside plugin ownership"
                    );
                    return ScriptRouterExit::Continue;
                };
                if !(mc_world::MIN_Y..mc_world::MAX_Y).contains(y) {
                    debug!(%block_id, y, "Loader block placement rejected outside world height");
                    return ScriptRouterExit::Continue;
                }
                if let Err(error) = context
                    .simulation
                    .place_loader_block_server_owned(
                        mc_world::BlockPos {
                            x: *x,
                            y: *y,
                            z: *z,
                        },
                        state,
                    )
                    .await
                {
                    debug!(?error, %block_id, "Loader block placement rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::GrantLoaderBlockItem {
                player_id,
                block_id,
                count,
            } => {
                let Some(manifest) = context.config.loader_manifest.as_deref() else {
                    debug!("Loader item grant rejected without an active manifest");
                    return ScriptRouterExit::Continue;
                };
                let Some(stack) = manifest.world_block_item(
                    admitted.plugin_id(),
                    block_id,
                    *count,
                    &context.config.items,
                ) else {
                    debug!(
                        plugin = admitted.plugin_id(),
                        %block_id,
                        "Loader item grant rejected outside plugin ownership"
                    );
                    return ScriptRouterExit::Continue;
                };
                if let Err(error) = context
                    .sessions
                    .route_loader_item_grant(player_id.value(), block_id, stack)
                    .await
                {
                    debug!(
                        ?error,
                        player_id = player_id.value(),
                        %block_id,
                        "Loader item grant rejected"
                    );
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::PlayerInventoryTransaction { .. } => {
                match self
                    .inventories
                    .route_admitted(admitted, context.sessions, context.config.world.is_some())
                    .await
                {
                    Ok(()) => ScriptRouterExit::Continue,
                    Err(InventoryAdapterError::PublicationClosed) => ScriptRouterExit::Stop,
                    Err(error) => {
                        warn!(?error, "admitted player inventory transaction rejected");
                        ScriptRouterExit::Continue
                    }
                }
            }
            ScriptCommand::UpsertZone { .. } | ScriptCommand::RemoveZone { .. } => {
                match self.zones.route_admitted_with_result(admitted).await {
                    Ok(_) => {}
                    Err(super::zone::ZoneAdapterError::PublicationClosed) => {
                        return ScriptRouterExit::Stop;
                    }
                    Err(error) => warn!(?error, "admitted script zone command rejected"),
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::UpsertColony { .. }
            | ScriptCommand::RequestVillagerBinding { .. }
            | ScriptCommand::SetVillagerOrder { .. } => {
                self.route_colony_admitted(admitted, context.sessions).await
            }
            ScriptCommand::TeleportPlayer { .. } => {
                match self
                    .teleports
                    .route_admitted(admitted, context.sessions)
                    .await
                {
                    Ok(()) => ScriptRouterExit::Continue,
                    Err(TeleportAdapterError::PublicationClosed) => ScriptRouterExit::Stop,
                    Err(error) => {
                        warn!(?error, "admitted player teleport rejected");
                        ScriptRouterExit::Continue
                    }
                }
            }
            ScriptCommand::ListOnlinePlayers { .. } => {
                match self
                    .player_queries
                    .route_admitted(admitted, context.sessions)
                    .await
                {
                    Ok(()) => ScriptRouterExit::Continue,
                    Err(PlayerQueryAdapterError::PublicationClosed) => ScriptRouterExit::Stop,
                    Err(error) => {
                        warn!(?error, "admitted player query rejected");
                        ScriptRouterExit::Continue
                    }
                }
            }
            ScriptCommand::HostAttached { .. }
            | ScriptCommand::PluginStorageGet { .. }
            | ScriptCommand::PluginStorageCompareAndSwap { .. }
            | ScriptCommand::PluginStorageDelete { .. }
            | ScriptCommand::InventoryStorageTransaction { .. } => {
                debug!("invalid admitted script command rejected");
                ScriptRouterExit::Continue
            }
            _ => {
                debug!("unknown admitted script command rejected");
                ScriptRouterExit::Continue
            }
        }
    }

    pub(super) async fn route_colony_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &play::SessionRegistry,
    ) -> ScriptRouterExit {
        let result = match admitted.request() {
            ScriptCommand::UpsertColony { .. } => self.colonies.route_admitted(admitted).await,
            ScriptCommand::RequestVillagerBinding { .. } => {
                self.colonies
                    .route_binding_admitted(admitted, sessions)
                    .await
            }
            ScriptCommand::SetVillagerOrder { .. } => {
                self.colonies.route_order_admitted(admitted, sessions).await
            }
            _ => Err(ColonyAdapterError::WrongCommand),
        };
        match result {
            Ok(_) => ScriptRouterExit::Continue,
            Err(
                ColonyAdapterError::PublicationClosed
                | ColonyAdapterError::BindingOwner(_)
                | ColonyAdapterError::TokenUnavailable
                | ColonyAdapterError::InvalidResult(_),
            ) => ScriptRouterExit::Stop,
            Err(error) => {
                warn!(?error, "admitted colony command rejected");
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
        let event = match storage_failure_event(admitted, failure) {
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
