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
    teleports: PluginTeleportAdapter,
    player_queries: PluginPlayerQueryAdapter,
}

impl ScriptRouter {
    pub(crate) fn new_with_zones(
        scripts: ScriptEventSink,
        storage: Option<PluginStorageHandle>,
        zones: PluginZoneAdapter,
    ) -> Self {
        let inventories = PluginInventoryAdapter::new(scripts.clone());
        let teleports = PluginTeleportAdapter::new(scripts.clone());
        let player_queries = PluginPlayerQueryAdapter::new(scripts.clone());
        Self {
            scripts,
            inventories,
            storage,
            zones,
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
            | ScriptCommand::SendCustomPayload { .. }
            | ScriptCommand::PluginStorageGet { .. }
            | ScriptCommand::PluginStorageCompareAndSwap { .. }
            | ScriptCommand::PluginStorageDelete { .. }
            | ScriptCommand::Operation { .. }
            | ScriptCommand::OpenInventoryMenu { .. }
            | ScriptCommand::CloseInventoryMenu { .. }
            | ScriptCommand::InventoryStorageTransaction { .. }
            | ScriptCommand::PlayerInventoryTransaction { .. }
            | ScriptCommand::PlaceLoaderBlock { .. }
            | ScriptCommand::GrantLoaderBlockItem { .. }
            | ScriptCommand::UpsertZone { .. }
            | ScriptCommand::RemoveZone { .. }
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
                | ScriptCommand::Operation { .. }
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
            ScriptCommand::SendCustomPayload { .. } => {
                let (_, player_id, channel, payload) = match admitted.into_send_custom_payload() {
                    Ok(command) => command,
                    Err(error) => {
                        debug!(?error, "script custom payload extraction rejected");
                        return ScriptRouterExit::Continue;
                    }
                };
                match mc_protocol::codec::Identifier::parse(&channel) {
                    Ok(channel) => {
                        if !context.sessions.send_custom_payload(
                            player_id.value(),
                            channel,
                            payload,
                        ) {
                            debug!(
                                player_id = player_id.value(),
                                "script custom payload targeted unknown player"
                            );
                        }
                    }
                    Err(error) => debug!(?error, "script custom payload channel rejected"),
                }
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
            ScriptCommand::OpenClientView { .. } => {
                match context.sessions.route_script_open_client_view(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    Ok(result) => self.scripts.enqueue_event(result.event),
                    Err(error) => debug!(?error, "admitted client view open rejected"),
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::PresentClientView { .. } => {
                if let Err(error) = context.sessions.route_script_present_client_view(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    debug!(?error, "admitted client view present rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::CloseClientView { .. } => {
                if let Err(error) = context.sessions.route_script_close_client_view(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    debug!(?error, "admitted client view close rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::BeginClientSelection { .. } => {
                match context.sessions.route_script_begin_client_selection(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    Ok(result) => self.scripts.enqueue_event(result.event),
                    Err(error) => debug!(?error, "admitted client selection begin rejected"),
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::CancelClientSelection { .. } => {
                if let Err(error) = context.sessions.route_script_cancel_client_selection(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    debug!(?error, "admitted client selection cancel rejected");
                }
                ScriptRouterExit::Continue
            }
            ScriptCommand::ClientSound { .. } => {
                if let Err(error) = context.sessions.route_script_client_sound_command(
                    admitted,
                    context.config.loader_manifest.as_deref(),
                ) {
                    debug!(?error, "admitted client sound command rejected");
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
                match self.zones.route_admitted_with_changed_zones(admitted).await {
                    Ok((_, changed_zones)) => {
                        if let Some(storage) = self.storage.as_ref() {
                            for zone in changed_zones {
                                storage.wake_resident_work_for_zone(zone).await;
                            }
                        }
                    }
                    Err(super::zone::ZoneAdapterError::PublicationClosed) => {
                        return ScriptRouterExit::Stop;
                    }
                    Err(error) => warn!(?error, "admitted script zone command rejected"),
                }
                ScriptRouterExit::Continue
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
            | ScriptCommand::Operation { .. }
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
                Some(state) => {
                    let position = mc_world::BlockPos {
                        x: request.x(),
                        y: request.y(),
                        z: request.z(),
                    };
                    let zone_fence = self.zones.capture_protection_fence();
                    if !zone_fence.allows("minecraft:overworld", position) {
                        Some(ScriptLoaderBlockPlacementFailure::Rejected)
                    } else {
                        match simulation
                            .place_loader_block_server_owned(
                                admitted.plugin_id(),
                                position,
                                state,
                                Some(zone_fence),
                            )
                            .await
                        {
                            Ok(true) => None,
                            Ok(false) => Some(ScriptLoaderBlockPlacementFailure::Rejected),
                            Err(error) => Some(script_loader_block_failure(error)),
                        }
                    }
                }
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
            .damage_script_entity(
                mc_entity::EntityId(entity_id),
                request.amount(),
                admitted.plugin_id(),
            )
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
        let wake = (
            request.dimension().to_owned(),
            [request.x().div_euclid(16), request.z().div_euclid(16)],
        );
        let result = match resolve_world_block_request(request, blocks) {
            Err(failure) => (false, Some(failure)),
            Ok((position, state)) => {
                let zone_fence = self.zones.capture_protection_fence();
                if !zone_fence.allows("minecraft:overworld", position) {
                    (false, Some(ScriptWorldBlockSetFailure::Rejected))
                } else {
                    match simulation
                        .place_loader_block_server_owned(
                            admitted.plugin_id(),
                            position,
                            state,
                            Some(zone_fence),
                        )
                        .await
                    {
                        Ok(true) => (true, None),
                        Ok(false) => (false, Some(ScriptWorldBlockSetFailure::Rejected)),
                        Err(error) => (false, Some(script_world_block_failure(error))),
                    }
                }
            }
        };
        let event = match admitted.world_block_set_result(result.0, result.1) {
            Ok(event) => event,
            Err(error) => {
                warn!(?error, "world-block result construction failed");
                return ScriptRouterExit::Stop;
            }
        };
        match deliver_required_targeted_event(&self.scripts, event).await {
            TargetedEventDelivery::Delivered => {
                if result.0
                    && let Some(storage) = self.storage.as_ref()
                {
                    storage.wake_resident_work(wake.0, vec![wake.1]).await;
                }
                ScriptRouterExit::Continue
            }
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
    use super::*;

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

    use super::*;

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

    use super::*;

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
    use mc_data::Identifier;
    use mc_world::{BlockPos, BlockRegistry};

    use super::*;

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
    use super::*;

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
