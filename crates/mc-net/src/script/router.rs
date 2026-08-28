use mc_script::{
    AdmittedScriptCommand, ScriptCommand, ScriptEntityDamageFailure, ScriptEntitySpawnFailure,
    ScriptLoaderBlockPlacementFailure, ScriptLoaderItemGrantFailure, ScriptPlayerInventoryFailure,
    ScriptPluginStorageFailure, ScriptWorldBlockSetFailure, ScriptWorldBlockSetRequest,
    ScriptWorldTimeSetFailure,
};
use tracing::{debug, warn};

use super::events::{TargetedEventDelivery, deliver_required_targeted_event};
use super::inventory::{InventoryAdapterError, PluginInventoryAdapter};
use super::player_query::{PlayerQueryAdapterError, PluginPlayerQueryAdapter};
use super::storage::{PluginStorageHandle, storage_failure_event};
use super::teleport::{PluginTeleportAdapter, TeleportAdapterError};
use super::villager::{PluginVillagerAdapter, VillagerAdapterError};
use super::zone::PluginZoneAdapter;
use crate::play;
use crate::server::{ScriptEventSink, ServerConfig, ShutdownHandle, resolve_script_entity_type};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptRouterExit {
    Continue,
    Stop,
}

#[derive(Clone, Copy)]
pub(crate) struct ScriptRouterContext<'a> {
    pub(crate) config: &'a ServerConfig,
    pub(crate) sessions: &'a play::SessionRegistry,
    pub(crate) simulation: &'a play::SimulationHandle,
    pub(crate) shutdown: &'a ShutdownHandle,
}

pub(crate) struct ScriptRouter {
    scripts: ScriptEventSink,
    inventories: PluginInventoryAdapter,
    storage: Option<PluginStorageHandle>,
    zones: PluginZoneAdapter,
    villagers: PluginVillagerAdapter,
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
        let villagers = PluginVillagerAdapter::new(scripts.clone());
        let teleports = PluginTeleportAdapter::new(scripts.clone());
        let player_queries = PluginPlayerQueryAdapter::new(scripts.clone());
        Self {
            scripts,
            inventories,
            storage,
            zones,
            villagers,
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
        simulation: &'a play::SimulationHandle,
        shutdown: &'a ShutdownHandle,
    ) -> ScriptRouterContext<'a> {
        ScriptRouterContext {
            config,
            sessions,
            simulation,
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
            ScriptCommand::SpawnEntity { .. }
            | ScriptCommand::DamageEntity { .. }
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
            | ScriptCommand::RequestVillagerBinding { .. }
            | ScriptCommand::SetVillagerGoal { .. }
            | ScriptCommand::TeleportPlayer { .. }
            | ScriptCommand::SetWorldTime { .. }
            | ScriptCommand::SetWorldBlock { .. }
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
            ScriptCommand::SpawnEntity { .. } => {
                self.route_entity_spawn_admitted(admitted, context.config, context.simulation)
                    .await
            }
            ScriptCommand::DamageEntity { .. } => {
                self.route_entity_damage_admitted(admitted, context.simulation)
                    .await
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
            ScriptCommand::PlaceLoaderBlock { .. } => {
                self.route_loader_block_admitted(admitted, context.config, context.simulation)
                    .await
            }
            ScriptCommand::GrantLoaderBlockItem { .. } => {
                self.route_loader_item_grant_admitted(admitted, context.config, context.sessions)
                    .await
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
            ScriptCommand::RequestVillagerBinding { .. }
            | ScriptCommand::SetVillagerGoal { .. } => {
                self.route_villager_admitted(admitted, context.sessions)
                    .await
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
            ScriptCommand::SetWorldTime { .. } => {
                self.route_world_time_admitted(admitted, context.simulation)
                    .await
            }
            ScriptCommand::SetWorldBlock { .. } => {
                self.route_world_block_admitted(
                    admitted,
                    context.config.blocks.as_ref(),
                    context.simulation,
                )
                .await
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

    pub(super) async fn route_entity_spawn_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        config: &ServerConfig,
        simulation: &play::SimulationHandle,
    ) -> ScriptRouterExit {
        let ScriptCommand::SpawnEntity {
            actor,
            entity_type,
            position,
            ..
        } = admitted.request()
        else {
            debug!("invalid admitted entity-spawn command rejected");
            return ScriptRouterExit::Continue;
        };
        let failure = match resolve_script_entity_type(config, entity_type) {
            None => Some(ScriptEntitySpawnFailure::UnknownEntityType),
            Some(entity_type_id) => simulation
                .spawn_script_entity(
                    actor.value(),
                    entity_type_id,
                    entity_type.clone(),
                    mc_entity::Vec3::new(position.x(), position.y(), position.z()),
                )
                .await
                .err()
                .map(script_entity_spawn_failure),
        };
        let event = match admitted.entity_spawn_result(failure) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "entity-spawn result construction failed");
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

    pub(super) async fn route_loader_block_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        config: &ServerConfig,
        simulation: &play::SimulationHandle,
    ) -> ScriptRouterExit {
        let ScriptCommand::PlaceLoaderBlock { request } = admitted.request() else {
            debug!("invalid admitted Loader block command rejected");
            return ScriptRouterExit::Continue;
        };
        let failure = if config.loader_manifest.is_none() {
            Some(ScriptLoaderBlockPlacementFailure::LoaderUnavailable)
        } else if !(mc_world::MIN_Y..mc_world::MAX_Y).contains(&request.y())
            || f64::from(request.x()).abs() > mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT
            || f64::from(request.z()).abs() > mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT
        {
            Some(ScriptLoaderBlockPlacementFailure::OutOfWorld)
        } else {
            let manifest = config
                .loader_manifest
                .as_deref()
                .expect("checked Loader manifest");
            match manifest.world_block_state(
                admitted.plugin_id(),
                request.block_id(),
                &config.blocks,
            ) {
                None => Some(ScriptLoaderBlockPlacementFailure::NotOwned),
                Some(state) => match simulation
                    .place_loader_block_server_owned(
                        mc_world::BlockPos {
                            x: request.x(),
                            y: request.y(),
                            z: request.z(),
                        },
                        state,
                    )
                    .await
                {
                    Ok(true) => None,
                    Ok(false) => Some(ScriptLoaderBlockPlacementFailure::Rejected),
                    Err(error) => Some(script_loader_block_failure(error)),
                },
            }
        };
        let event = match admitted.loader_block_placement_result(failure) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "Loader block placement result construction failed");
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

    pub(super) async fn route_loader_item_grant_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        config: &ServerConfig,
        sessions: &play::SessionRegistry,
    ) -> ScriptRouterExit {
        let ScriptCommand::GrantLoaderBlockItem { request } = admitted.request() else {
            debug!("invalid admitted Loader item grant rejected");
            return ScriptRouterExit::Continue;
        };
        let failure = match config.loader_manifest.as_deref() {
            None => Some(ScriptLoaderItemGrantFailure::LoaderUnavailable),
            Some(manifest) => match manifest.world_block_item(
                admitted.plugin_id(),
                request.block_id(),
                request.count(),
                &config.items,
            ) {
                None => Some(ScriptLoaderItemGrantFailure::NotOwned),
                Some(stack) => sessions
                    .route_loader_item_grant(request.player_id().value(), request.block_id(), stack)
                    .await
                    .err()
                    .map(script_loader_item_grant_failure),
            },
        };
        let event = match admitted.loader_item_grant_result(failure) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "Loader item grant result construction failed");
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

    pub(super) async fn route_entity_damage_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        simulation: &play::SimulationHandle,
    ) -> ScriptRouterExit {
        let ScriptCommand::DamageEntity { request } = admitted.request() else {
            debug!("invalid admitted entity-damage command rejected");
            return ScriptRouterExit::Continue;
        };
        let entity_id = i32::try_from(request.entity_id().value())
            .expect("validated script entity id fits the simulation wire id");
        let (health, killed, failure) = match simulation
            .damage_script_entity(mc_entity::EntityId(entity_id), request.amount())
            .await
        {
            Ok(Some(committed)) => (Some(committed.health), committed.killed, None),
            Ok(None) => (None, false, Some(ScriptEntityDamageFailure::Rejected)),
            Err(error) => (None, false, Some(script_entity_damage_failure(error))),
        };
        let event = match admitted.entity_damage_result(health, killed, failure) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "entity-damage result construction failed");
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

    pub(super) async fn route_world_block_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        blocks: &mc_world::BlockRegistry,
        simulation: &play::SimulationHandle,
    ) -> ScriptRouterExit {
        let ScriptCommand::SetWorldBlock { request } = admitted.request() else {
            debug!("invalid admitted world-block command rejected");
            return ScriptRouterExit::Continue;
        };
        let result = match resolve_world_block_request(request, blocks) {
            Err(failure) => (false, Some(failure)),
            Ok((position, state)) => match simulation
                .place_loader_block_server_owned(position, state)
                .await
            {
                Ok(true) => (true, None),
                Ok(false) => (false, Some(ScriptWorldBlockSetFailure::Rejected)),
                Err(error) => (false, Some(script_world_block_failure(error))),
            },
        };
        let event = match admitted.world_block_set_result(result.0, result.1) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "world-block result construction failed");
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

    pub(super) async fn route_world_time_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        simulation: &play::SimulationHandle,
    ) -> ScriptRouterExit {
        let ScriptCommand::SetWorldTime { request } = admitted.request() else {
            debug!("invalid admitted world-time command rejected");
            return ScriptRouterExit::Continue;
        };
        let world_time = request.world_time();
        let failure = simulation
            .set_world_time_server_owned(world_time)
            .await
            .err()
            .map(script_world_time_failure);
        let event = match admitted.world_time_set_result(failure) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, world_time, "world-time result construction failed");
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

    pub(super) async fn route_villager_admitted(
        &self,
        admitted: AdmittedScriptCommand,
        sessions: &play::SessionRegistry,
    ) -> ScriptRouterExit {
        let result = match admitted.request() {
            ScriptCommand::RequestVillagerBinding { .. } => {
                self.villagers
                    .route_binding_admitted(admitted, sessions)
                    .await
            }
            ScriptCommand::SetVillagerGoal { .. } => {
                self.villagers.route_goal_admitted(admitted, sessions).await
            }
            _ => Err(VillagerAdapterError::WrongCommand),
        };
        match result {
            Ok(_) => ScriptRouterExit::Continue,
            Err(
                VillagerAdapterError::PublicationClosed
                | VillagerAdapterError::BindingOwner(_)
                | VillagerAdapterError::TokenUnavailable
                | VillagerAdapterError::InvalidResult(_),
            ) => ScriptRouterExit::Stop,
            Err(error) => {
                warn!(?error, "admitted villager command rejected");
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

fn resolve_world_block_request(
    request: &ScriptWorldBlockSetRequest,
    blocks: &mc_world::BlockRegistry,
) -> Result<(mc_world::BlockPos, mc_world::BlockStateId), ScriptWorldBlockSetFailure> {
    if request.dimension() != "minecraft:overworld" {
        return Err(ScriptWorldBlockSetFailure::UnsupportedDimension);
    }
    if !(mc_world::MIN_Y..mc_world::MAX_Y).contains(&request.y())
        || f64::from(request.x()).abs() > mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT
        || f64::from(request.z()).abs() > mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT
    {
        return Err(ScriptWorldBlockSetFailure::OutOfWorld);
    }
    let state = mc_data::Identifier::parse(request.block_id())
        .ok()
        .and_then(|id| blocks.block(&id))
        .map(|block| block.default)
        .ok_or(ScriptWorldBlockSetFailure::UnknownBlock)?;
    Ok((
        mc_world::BlockPos {
            x: request.x(),
            y: request.y(),
            z: request.z(),
        },
        state,
    ))
}

fn script_loader_block_failure(
    error: play::SimulationRequestError,
) -> ScriptLoaderBlockPlacementFailure {
    match error {
        play::SimulationRequestError::Full
        | play::SimulationRequestError::QueueAdmissionTimeout => {
            ScriptLoaderBlockPlacementFailure::Busy
        }
        play::SimulationRequestError::Closed
        | play::SimulationRequestError::OwnerStopped
        | play::SimulationRequestError::ResponseTimeout
        | play::SimulationRequestError::ShuttingDown
        | play::SimulationRequestError::WorldUnavailable => {
            ScriptLoaderBlockPlacementFailure::RuntimeUnavailable
        }
        _ => ScriptLoaderBlockPlacementFailure::Rejected,
    }
}

fn script_loader_item_grant_failure(
    failure: ScriptPlayerInventoryFailure,
) -> ScriptLoaderItemGrantFailure {
    match failure {
        ScriptPlayerInventoryFailure::PlayerUnavailable => {
            ScriptLoaderItemGrantFailure::PlayerUnavailable
        }
        ScriptPlayerInventoryFailure::InventoryFull => ScriptLoaderItemGrantFailure::InventoryFull,
        ScriptPlayerInventoryFailure::RuntimeUnavailable => {
            ScriptLoaderItemGrantFailure::RuntimeUnavailable
        }
        _ => ScriptLoaderItemGrantFailure::Rejected,
    }
}

fn script_entity_damage_failure(error: play::SimulationRequestError) -> ScriptEntityDamageFailure {
    match error {
        play::SimulationRequestError::Full
        | play::SimulationRequestError::QueueAdmissionTimeout => ScriptEntityDamageFailure::Busy,
        play::SimulationRequestError::Closed
        | play::SimulationRequestError::OwnerStopped
        | play::SimulationRequestError::ResponseTimeout
        | play::SimulationRequestError::ShuttingDown
        | play::SimulationRequestError::WorldUnavailable => {
            ScriptEntityDamageFailure::RuntimeUnavailable
        }
        _ => ScriptEntityDamageFailure::Rejected,
    }
}

fn script_entity_spawn_failure(error: play::SimulationRequestError) -> ScriptEntitySpawnFailure {
    match error {
        play::SimulationRequestError::StaleSession => ScriptEntitySpawnFailure::ActorUnavailable,
        play::SimulationRequestError::Full
        | play::SimulationRequestError::QueueAdmissionTimeout => ScriptEntitySpawnFailure::Busy,
        play::SimulationRequestError::Closed
        | play::SimulationRequestError::OwnerStopped
        | play::SimulationRequestError::ResponseTimeout
        | play::SimulationRequestError::ShuttingDown
        | play::SimulationRequestError::WorldUnavailable => {
            ScriptEntitySpawnFailure::RuntimeUnavailable
        }
        _ => ScriptEntitySpawnFailure::Rejected,
    }
}

fn script_world_block_failure(error: play::SimulationRequestError) -> ScriptWorldBlockSetFailure {
    match error {
        play::SimulationRequestError::Full
        | play::SimulationRequestError::QueueAdmissionTimeout => ScriptWorldBlockSetFailure::Busy,
        play::SimulationRequestError::Closed
        | play::SimulationRequestError::OwnerStopped
        | play::SimulationRequestError::ResponseTimeout
        | play::SimulationRequestError::ShuttingDown
        | play::SimulationRequestError::WorldUnavailable => {
            ScriptWorldBlockSetFailure::RuntimeUnavailable
        }
        _ => ScriptWorldBlockSetFailure::Rejected,
    }
}

fn script_world_time_failure(error: play::SimulationRequestError) -> ScriptWorldTimeSetFailure {
    match error {
        play::SimulationRequestError::Full
        | play::SimulationRequestError::QueueAdmissionTimeout => ScriptWorldTimeSetFailure::Busy,
        play::SimulationRequestError::Closed
        | play::SimulationRequestError::OwnerStopped
        | play::SimulationRequestError::ResponseTimeout
        | play::SimulationRequestError::ShuttingDown
        | play::SimulationRequestError::WorldUnavailable => {
            ScriptWorldTimeSetFailure::RuntimeUnavailable
        }
        _ => ScriptWorldTimeSetFailure::Rejected,
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

#[cfg(test)]
mod loader_mutation_tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use mc_data::Identifier;
    use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};
    use mc_world::{BlockPos, Chunk, ChunkPos, WorldStorage};

    use super::*;
    use crate::loader::{
        LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderContentKind, LoaderManifest, LoaderPermission,
        LoaderPlatform,
    };

    fn loader_manifest(owner: &str) -> LoaderManifest {
        LoaderManifest {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles: vec![LoaderBundle {
                owner: owner.to_owned(),
                id: "ruby".to_owned(),
                version: "1".to_owned(),
                artifact: "client/ruby.zip".to_owned(),
                sha256: "a".repeat(64),
                size_bytes: 1,
                loaders: vec![LoaderPlatform::Fabric],
                content: vec![LoaderContentKind::Blocks],
                permissions: vec![LoaderPermission::RegisterBlocks],
                cache_key: format!("{owner}:ruby/1/{}", "a".repeat(64)),
                source_path: None,
                artifact_bytes: None,
                block_id: Some(format!("{owner}:ruby_block")),
                block_name: Some("Ruby Block".to_owned()),
            }],
        }
    }

    fn loader_blocks(manifest: &LoaderManifest) -> Arc<mc_world::BlockRegistry> {
        let mut report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }];
        manifest.append_world_block_report(&mut report).unwrap();
        Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap())
    }

    fn loader_config(
        manifest: LoaderManifest,
        blocks: Arc<mc_world::BlockRegistry>,
    ) -> ServerConfig {
        ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "loader mutation plugin test".to_owned(),
            max_players: 4,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: None,
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::solaris_required_items()),
            item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: crate::ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: crate::server::CommandPermissionConfig::new(
                Vec::<String>::new(),
                true,
            ),
            loader_manifest: Some(Arc::new(manifest)),
            shutdown: ShutdownHandle::default(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn loader_block_placement_returns_targeted_result_after_owner_commit() {
        let owner_id = "loader-plugin";
        let manifest = loader_manifest(owner_id);
        let blocks = loader_blocks(&manifest);
        let config = loader_config(manifest, Arc::clone(&blocks));
        let custom = blocks
            .block(&Identifier::parse("loader-plugin:ruby_block").unwrap())
            .unwrap()
            .default;
        let air = blocks
            .block(&Identifier::parse("minecraft:air").unwrap())
            .unwrap()
            .default;
        let chunk = ChunkPos { x: 0, z: 0 };
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(chunk, air, Identifier::parse("minecraft:plains").unwrap()),
            )
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));

        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join(owner_id);
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "loader-plugin"
name = "Loader Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_server_started(_event)
                    solaris.place_loader_block("place-ruby", "loader-plugin:ruby_block", 1, 64, 1)
                end

                function on_loader_block_placement_result(event)
                    solaris.broadcast(tostring(event.placed) .. ":" .. tostring(event.failure))
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        let sessions = play::SessionRegistry::new();
        let (simulation, mut simulation_owner) = play::simulation_channel();
        let (route_exit, ()) = tokio::join!(
            router.route_loader_block_admitted(admitted, &config, &simulation),
            async {
                assert!(simulation_owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                simulation_owner
                    .process_commands_with_world(&sessions, Some(&world), None, 1)
                    .await;
            }
        );
        assert_eq!(route_exit, ScriptRouterExit::Continue);
        assert_eq!(
            world
                .lock()
                .await
                .get_cached_block(BlockPos { x: 1, y: 64, z: 1 }),
            Some(custom)
        );

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message } if message == "true:nil"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn loader_item_grant_returns_targeted_player_unavailable_result() {
        let owner_id = "loader-plugin";
        let manifest = loader_manifest(owner_id);
        let blocks = loader_blocks(&manifest);
        let config = loader_config(manifest, blocks);
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join(owner_id);
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "loader-plugin"
name = "Loader Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_server_started(_event)
                    solaris.grant_loader_block_item("grant-ruby", 7, "loader-plugin:ruby_block", 1)
                end

                function on_loader_item_grant_result(event)
                    solaris.broadcast(tostring(event.granted) .. ":" .. tostring(event.failure))
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        let sessions = play::SessionRegistry::new();
        assert_eq!(
            router
                .route_loader_item_grant_admitted(admitted, &config, &sessions)
                .await,
            ScriptRouterExit::Continue
        );

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message }
                if message == "false:player_unavailable"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn loader_failure_categories_are_stable() {
        for error in [
            play::SimulationRequestError::Full,
            play::SimulationRequestError::QueueAdmissionTimeout,
        ] {
            assert_eq!(
                script_loader_block_failure(error),
                ScriptLoaderBlockPlacementFailure::Busy
            );
        }
        for error in [
            play::SimulationRequestError::Closed,
            play::SimulationRequestError::OwnerStopped,
            play::SimulationRequestError::ResponseTimeout,
            play::SimulationRequestError::ShuttingDown,
            play::SimulationRequestError::WorldUnavailable,
        ] {
            assert_eq!(
                script_loader_block_failure(error),
                ScriptLoaderBlockPlacementFailure::RuntimeUnavailable
            );
        }
        assert_eq!(
            script_loader_item_grant_failure(ScriptPlayerInventoryFailure::InventoryFull),
            ScriptLoaderItemGrantFailure::InventoryFull
        );
        assert_eq!(
            script_loader_item_grant_failure(ScriptPlayerInventoryFailure::PlayerUnavailable),
            ScriptLoaderItemGrantFailure::PlayerUnavailable
        );
        assert_eq!(
            script_loader_item_grant_failure(ScriptPlayerInventoryFailure::RuntimeUnavailable),
            ScriptLoaderItemGrantFailure::RuntimeUnavailable
        );
    }
}

#[cfg(test)]
mod entity_damage_tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};

    use super::*;

    fn combat_test_config() -> ServerConfig {
        ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "entity combat plugin test".to_owned(),
            max_players: 4,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
            world: None,
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: crate::ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: crate::server::CommandPermissionConfig::new(
                Vec::<String>::new(),
                true,
            ),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        }
    }

    async fn route_damage(
        router: &ScriptRouter,
        admitted: AdmittedScriptCommand,
        simulation: &play::SimulationHandle,
        owner: &mut play::SimulationOwner,
        sessions: &play::SessionRegistry,
    ) -> ScriptRouterExit {
        let (route_exit, ()) = tokio::join!(
            router.route_entity_damage_admitted(admitted, simulation),
            async {
                assert!(owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                assert_eq!(owner.process_tick(sessions, 1).processed, 1);
            }
        );
        route_exit
    }

    #[tokio::test(flavor = "current_thread")]
    async fn entity_damage_routes_through_generic_combat_owner_and_returns_targeted_results() {
        let config = combat_test_config();
        let sessions = play::SessionRegistry::new();
        let cow_type = resolve_script_entity_type(&config, "minecraft:cow").unwrap();
        let hurt = sessions.spawn_script_router_test_entity(
            cow_type,
            "minecraft:cow",
            mc_entity::Vec3::new(1.5, 64.0, 1.5),
        );
        let kill = sessions.spawn_script_router_test_entity(
            cow_type,
            "minecraft:cow",
            mc_entity::Vec3::new(3.5, 64.0, 1.5),
        );
        let hurt_before = sessions.authoritative_entity_snapshot(hurt).unwrap().health;
        let kill_before = sessions.authoritative_entity_snapshot(kill).unwrap().health;
        assert!(hurt_before > 2.0);
        assert!(kill_before > 0.0);

        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("combat-plugin");
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "combat-plugin"
name = "Combat Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["entity_damage"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            format!(
                r#"
                    function on_server_started(_event)
                        solaris.damage_entity("hurt", {hurt}, 2.0)
                        solaris.damage_entity("kill", {kill}, 1000000.0)
                        solaris.damage_entity("missing", 2147483647, 1.0)
                    end

                    function on_entity_damage_result(event)
                        if event.request_id == "hurt" then
                            assert(event.damaged == true)
                            assert(event.health ~= nil)
                            assert(event.killed == false)
                            assert(event.failure == nil)
                            solaris.broadcast("hurt-ok")
                        elseif event.request_id == "kill" then
                            assert(event.damaged == true)
                            assert(event.health == 0)
                            assert(event.killed == true)
                            assert(event.failure == nil)
                            solaris.broadcast("kill-ok")
                        elseif event.request_id == "missing" then
                            assert(event.damaged == false)
                            assert(event.health == nil)
                            assert(event.killed == false)
                            assert(event.failure == "rejected")
                            solaris.broadcast("missing-ok")
                        end
                    end
                "#,
                hurt = hurt.0,
                kill = kill.0,
            ),
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();

        let mut admitted = Vec::new();
        for expected in ["hurt", "kill", "missing"] {
            let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
                .await
                .unwrap()
                .unwrap();
            let command = boundary.accept_host_command(command).unwrap();
            assert!(matches!(
                command.request(),
                ScriptCommand::DamageEntity { request } if request.request_id() == expected
            ));
            admitted.push(command);
        }

        let (simulation, mut owner) = play::simulation_channel();
        for (command, expected_ack) in
            admitted
                .into_iter()
                .zip(["hurt-ok", "kill-ok", "missing-ok"])
        {
            assert_eq!(
                route_damage(&router, command, &simulation, &mut owner, &sessions).await,
                ScriptRouterExit::Continue
            );
            let callback = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
                .await
                .unwrap()
                .unwrap();
            let callback = boundary.accept_host_command(callback).unwrap();
            assert!(matches!(
                callback.request(),
                ScriptCommand::BroadcastChatMessage { message } if message == expected_ack
            ));
        }

        assert_eq!(
            sessions.authoritative_entity_snapshot(hurt).unwrap().health,
            hurt_before - 2.0
        );
        let killed = sessions.authoritative_entity_snapshot(kill).unwrap();
        assert_eq!(killed.health, 0.0);
        assert_ne!(killed.lifecycle, mc_entity::EntityLifecycle::Alive);

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn entity_damage_failure_categories_are_stable() {
        for error in [
            play::SimulationRequestError::Full,
            play::SimulationRequestError::QueueAdmissionTimeout,
        ] {
            assert_eq!(
                script_entity_damage_failure(error),
                ScriptEntityDamageFailure::Busy
            );
        }
        for error in [
            play::SimulationRequestError::Closed,
            play::SimulationRequestError::OwnerStopped,
            play::SimulationRequestError::ResponseTimeout,
            play::SimulationRequestError::ShuttingDown,
            play::SimulationRequestError::WorldUnavailable,
        ] {
            assert_eq!(
                script_entity_damage_failure(error),
                ScriptEntityDamageFailure::RuntimeUnavailable
            );
        }
        for error in [
            play::SimulationRequestError::ResponseMismatch,
            play::SimulationRequestError::WorldBusy,
            play::SimulationRequestError::WorldMutationFailed,
            play::SimulationRequestError::CrossRegion,
            play::SimulationRequestError::InvalidCommand,
            play::SimulationRequestError::StaleSession,
        ] {
            assert_eq!(
                script_entity_damage_failure(error),
                ScriptEntityDamageFailure::Rejected
            );
        }
    }
}

#[cfg(test)]
mod entity_spawn_tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use mc_script::{
        LuaHostConfig, ScriptEvent, ScriptPlayerContext, ScriptPlayerId, start_lua_host,
    };

    use super::*;

    fn spawn_test_config() -> ServerConfig {
        ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "entity spawn plugin test".to_owned(),
            max_players: 4,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks: Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap()),
            world: None,
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::default()),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: crate::ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: crate::server::CommandPermissionConfig::new(
                Vec::<String>::new(),
                true,
            ),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        }
    }

    fn register_actor(registry: &play::SessionRegistry) -> u64 {
        registry.register_script_router_test_session("EntityActor")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn entity_spawn_routes_through_owner_and_returns_targeted_result() {
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("entity-plugin");
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "entity-plugin"
name = "Entity Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["player.joined"]
spawn_entities = ["minecraft:pig"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_player_joined(event)
                    solaris.spawn_entity("spawn-pig", event.player_id, "minecraft:pig", 2.5, 64.0, 1.5)
                end

                function on_entity_spawn_result(event)
                    solaris.broadcast(tostring(event.request_id) .. ":" .. tostring(event.failure))
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        let config = spawn_test_config();
        let sessions = play::SessionRegistry::new();
        let actor = register_actor(&sessions);
        boundary
            .try_enqueue_event(ScriptEvent::player_joined_with_context(
                ScriptPlayerId::new(actor),
                ScriptPlayerContext::new(
                    crate::login::offline_uuid("EntityActor").to_string(),
                    "EntityActor",
                    false,
                    0.5,
                    64.0,
                    0.5,
                ),
            ))
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        let (simulation, mut owner) = play::simulation_channel();
        let (route_exit, ()) = tokio::join!(
            router.route_entity_spawn_admitted(admitted, &config, &simulation),
            async {
                assert!(owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
            }
        );
        assert_eq!(route_exit, ScriptRouterExit::Continue);
        assert!(sessions.persisted_entity_records().iter().any(|entity| {
            entity.snapshot.type_name == "minecraft:pig"
                && entity.snapshot.position == mc_entity::Vec3::new(2.5, 64.0, 1.5)
        }));

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message } if message == "spawn-pig:nil"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_entity_type_returns_targeted_failure_without_owner_mutation() {
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("entity-plugin");
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "entity-plugin"
name = "Entity Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["player.joined"]
spawn_entities = ["minecraft:not_registered"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_player_joined(event)
                    solaris.spawn_entity("missing-type", event.player_id, "minecraft:not_registered", 2.5, 64.0, 1.5)
                end

                function on_entity_spawn_result(event)
                    solaris.broadcast(tostring(event.failure))
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        let config = spawn_test_config();
        boundary
            .try_enqueue_event(ScriptEvent::player_joined_with_context(
                ScriptPlayerId::new(7),
                ScriptPlayerContext::new(
                    "00000000-0000-0000-0000-000000000007",
                    "EntityActor",
                    false,
                    0.5,
                    64.0,
                    0.5,
                ),
            ))
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        let (simulation, _owner) = play::simulation_channel();
        assert_eq!(
            router
                .route_entity_spawn_admitted(admitted, &config, &simulation)
                .await,
            ScriptRouterExit::Continue
        );
        assert_eq!(simulation.snapshot().depth, 0);

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message } if message == "unknown_entity_type"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn entity_spawn_failure_categories_are_stable() {
        assert_eq!(
            script_entity_spawn_failure(play::SimulationRequestError::StaleSession),
            ScriptEntitySpawnFailure::ActorUnavailable
        );
        for error in [
            play::SimulationRequestError::Full,
            play::SimulationRequestError::QueueAdmissionTimeout,
        ] {
            assert_eq!(
                script_entity_spawn_failure(error),
                ScriptEntitySpawnFailure::Busy
            );
        }
        for error in [
            play::SimulationRequestError::Closed,
            play::SimulationRequestError::OwnerStopped,
            play::SimulationRequestError::ResponseTimeout,
            play::SimulationRequestError::ShuttingDown,
            play::SimulationRequestError::WorldUnavailable,
        ] {
            assert_eq!(
                script_entity_spawn_failure(error),
                ScriptEntitySpawnFailure::RuntimeUnavailable
            );
        }
        for error in [
            play::SimulationRequestError::ResponseMismatch,
            play::SimulationRequestError::WorldBusy,
            play::SimulationRequestError::WorldMutationFailed,
            play::SimulationRequestError::CrossRegion,
            play::SimulationRequestError::InvalidCommand,
        ] {
            assert_eq!(
                script_entity_spawn_failure(error),
                ScriptEntitySpawnFailure::Rejected
            );
        }
    }
}

#[cfg(test)]
mod world_block_tests {
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    use mc_data::Identifier;
    use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};
    use mc_world::{BlockPos, BlockRegistry, Chunk, ChunkPos, WorldStorage};

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn world_block_command_routes_through_owner_and_returns_targeted_result() {
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("block-plugin");
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "block-plugin"
name = "Block Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["world_blocks"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_server_started(_event)
                    solaris.set_block("set-stone", "minecraft:overworld", "minecraft:stone", 1, 64, 1)
                end

                function on_world_block_set_result(event)
                    solaris.broadcast(tostring(event.applied))
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        assert_eq!(host.loaded_plugins(), 1);
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        assert_eq!(admitted.plugin_id(), "block-plugin");

        let blocks = Arc::new(
            BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
        );
        let air = blocks
            .block(&Identifier::parse("minecraft:air").unwrap())
            .unwrap()
            .default;
        let stone = blocks
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .default;
        let chunk = ChunkPos { x: 0, z: 0 };
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(chunk, air, Identifier::parse("minecraft:plains").unwrap()),
            )
            .unwrap();
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let (route_exit, ()) = tokio::join!(
            router.route_world_block_admitted(admitted, blocks.as_ref(), &simulation),
            async {
                assert!(owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                owner
                    .process_commands_with_world(&sessions, Some(&world), None, 1)
                    .await;
            }
        );
        assert_eq!(route_exit, ScriptRouterExit::Continue);
        assert_eq!(
            world
                .lock()
                .await
                .get_cached_block(BlockPos { x: 1, y: 64, z: 1 }),
            Some(stone)
        );

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message } if message == "true"
        ));

        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        let (route_exit, ()) = tokio::join!(
            router.route_world_block_admitted(admitted, blocks.as_ref(), &simulation),
            async {
                assert!(owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                owner
                    .process_commands_with_world(&sessions, Some(&world), None, 1)
                    .await;
            }
        );
        assert_eq!(route_exit, ScriptRouterExit::Continue);
        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message } if message == "true"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn world_block_resolution_validates_closed_contract() {
        let blocks =
            BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
        let valid = ScriptWorldBlockSetRequest::try_new(
            "valid",
            "minecraft:overworld",
            "minecraft:stone",
            1,
            64,
            2,
        )
        .unwrap();
        let (position, state) = resolve_world_block_request(&valid, &blocks).unwrap();
        assert_eq!(position, BlockPos { x: 1, y: 64, z: 2 });
        assert_eq!(
            state,
            blocks
                .block(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default
        );

        let unsupported = ScriptWorldBlockSetRequest::try_new(
            "dimension",
            "minecraft:the_nether",
            "minecraft:stone",
            1,
            64,
            2,
        )
        .unwrap();
        assert_eq!(
            resolve_world_block_request(&unsupported, &blocks),
            Err(ScriptWorldBlockSetFailure::UnsupportedDimension)
        );

        let out_of_world = ScriptWorldBlockSetRequest::try_new(
            "height",
            "minecraft:overworld",
            "minecraft:stone",
            1,
            mc_world::MAX_Y,
            2,
        )
        .unwrap();
        assert_eq!(
            resolve_world_block_request(&out_of_world, &blocks),
            Err(ScriptWorldBlockSetFailure::OutOfWorld)
        );

        let horizontal_out_of_world = ScriptWorldBlockSetRequest::try_new(
            "horizontal",
            "minecraft:overworld",
            "minecraft:stone",
            30_000_001,
            64,
            2,
        )
        .unwrap();
        assert_eq!(
            resolve_world_block_request(&horizontal_out_of_world, &blocks),
            Err(ScriptWorldBlockSetFailure::OutOfWorld)
        );

        let unknown = ScriptWorldBlockSetRequest::try_new(
            "unknown",
            "minecraft:overworld",
            "minecraft:not_a_real_block",
            1,
            64,
            2,
        )
        .unwrap();
        assert_eq!(
            resolve_world_block_request(&unknown, &blocks),
            Err(ScriptWorldBlockSetFailure::UnknownBlock)
        );
    }

    #[test]
    fn world_block_failure_categories_are_stable() {
        for error in [
            play::SimulationRequestError::Full,
            play::SimulationRequestError::QueueAdmissionTimeout,
        ] {
            assert_eq!(
                script_world_block_failure(error),
                ScriptWorldBlockSetFailure::Busy
            );
        }
        for error in [
            play::SimulationRequestError::Closed,
            play::SimulationRequestError::OwnerStopped,
            play::SimulationRequestError::ResponseTimeout,
            play::SimulationRequestError::ShuttingDown,
            play::SimulationRequestError::WorldUnavailable,
        ] {
            assert_eq!(
                script_world_block_failure(error),
                ScriptWorldBlockSetFailure::RuntimeUnavailable
            );
        }
        for error in [
            play::SimulationRequestError::ResponseMismatch,
            play::SimulationRequestError::WorldBusy,
            play::SimulationRequestError::WorldMutationFailed,
            play::SimulationRequestError::CrossRegion,
            play::SimulationRequestError::InvalidCommand,
            play::SimulationRequestError::StaleSession,
        ] {
            assert_eq!(
                script_world_block_failure(error),
                ScriptWorldBlockSetFailure::Rejected
            );
        }
    }
}

#[cfg(test)]
mod world_time_tests {
    use std::fs;
    use std::time::Duration;

    use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn world_time_command_routes_through_owner_and_returns_targeted_result() {
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("clock-plugin");
        fs::create_dir(&plugin).unwrap();
        fs::write(
            plugin.join("plugin.toml"),
            r#"id = "clock-plugin"
name = "Clock Plugin"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["world_time"]
"#,
        )
        .unwrap();
        fs::write(
            plugin.join("main.lua"),
            r#"
                function on_server_started(_event)
                    solaris.set_world_time("set-night", 13000)
                end

                function on_world_time_set_result(_event)
                    solaris.broadcast("world-time-result")
                end
            "#,
        )
        .unwrap();

        let (boundary, host) =
            start_lua_host(LuaHostConfig::new(plugins.path()).strict_discovery(true)).unwrap();
        assert_eq!(host.loaded_plugins(), 1);
        let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        assert_eq!(admitted.plugin_id(), "clock-plugin");

        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let (route_exit, ()) = tokio::join!(
            router.route_world_time_admitted(admitted, &simulation),
            async {
                assert!(owner.wait_for_command().await);
                assert_eq!(simulation.snapshot().depth, 1);
                assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
            }
        );
        assert_eq!(route_exit, ScriptRouterExit::Continue);
        assert_eq!(sessions.world_time(), 13_000);

        let result_command = tokio::time::timeout(Duration::from_secs(1), boundary.recv_command())
            .await
            .unwrap()
            .unwrap();
        let result = boundary.accept_host_command(result_command).unwrap();
        assert!(matches!(
            result.request(),
            ScriptCommand::BroadcastChatMessage { message }
                if message == "world-time-result"
        ));

        drop(router);
        drop(boundary);
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn world_time_failure_categories_are_stable() {
        for error in [
            play::SimulationRequestError::Full,
            play::SimulationRequestError::QueueAdmissionTimeout,
        ] {
            assert_eq!(
                script_world_time_failure(error),
                ScriptWorldTimeSetFailure::Busy
            );
        }
        for error in [
            play::SimulationRequestError::Closed,
            play::SimulationRequestError::OwnerStopped,
            play::SimulationRequestError::ResponseTimeout,
            play::SimulationRequestError::ShuttingDown,
            play::SimulationRequestError::WorldUnavailable,
        ] {
            assert_eq!(
                script_world_time_failure(error),
                ScriptWorldTimeSetFailure::RuntimeUnavailable
            );
        }
        for error in [
            play::SimulationRequestError::ResponseMismatch,
            play::SimulationRequestError::WorldBusy,
            play::SimulationRequestError::WorldMutationFailed,
            play::SimulationRequestError::CrossRegion,
            play::SimulationRequestError::InvalidCommand,
            play::SimulationRequestError::StaleSession,
        ] {
            assert_eq!(
                script_world_time_failure(error),
                ScriptWorldTimeSetFailure::Rejected
            );
        }
    }
}
