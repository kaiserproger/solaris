//! Per-connection protocol state orchestration.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::ChunkGeometry;
use tracing::debug;

use crate::admission::PreAuthPermit;
use crate::chunk_pipeline::ChunkPipelineResources;
use crate::connection::{
    ConnectionReader, ConnectionWriter, MAX_PRE_PLAY_BYTES, MAX_PRE_PLAY_PACKETS,
    PRE_PLAY_READ_TIMEOUT, PRE_PLAY_TOTAL_TIMEOUT, PrePlayBudget,
    read_packet_with_timeout_budgeted,
};
use crate::error::ConnectionError;
use crate::script::PluginZoneAdapter;
use crate::server::{ConnectionWorld, ExtensionEventSink, ScriptEventSink, ServerConfig};
use crate::{RuntimeControlHandle, configuration, login, play, status};

#[derive(Clone)]
pub(crate) struct ConnectionServices {
    pub(crate) config: Arc<ServerConfig>,
    pub(crate) online_authentication: Option<Arc<login::OnlineAuthentication>>,
    pub(crate) chunk_geometry: ChunkGeometry,
    pub(crate) connection_world: ConnectionWorld,
    pub(crate) sessions: Arc<play::SessionRegistry>,
    pub(crate) chunk_pipeline_resources: ChunkPipelineResources,
    pub(crate) dirty_flush: Option<crate::dirty_flush::DirtyFlushNotifier>,
    pub(crate) runtime_control: Option<RuntimeControlHandle>,
    pub(crate) simulation: play::SimulationHandle,
    pub(crate) extension: Option<ExtensionEventSink>,
    pub(crate) scripts: Option<ScriptEventSink>,
    pub(crate) script_zones: Option<PluginZoneAdapter>,
}

async fn before_pre_play_deadline<T, F>(
    deadline: tokio::time::Instant,
    future: F,
) -> Result<T, ConnectionError>
where
    F: Future<Output = Result<T, ConnectionError>>,
{
    match tokio::time::timeout_at(deadline, future).await {
        Ok(result) => result,
        Err(_) => Err(ConnectionError::PrePlayDeadlineExceeded {
            timeout: PRE_PLAY_TOTAL_TIMEOUT,
        }),
    }
}

pub(crate) async fn handle_connection(
    socket: tokio::net::TcpStream,
    peer: SocketAddr,
    services: ConnectionServices,
    pre_auth_permit: PreAuthPermit,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets, matching vanilla.
    socket.set_nodelay(true)?;

    let (reader, writer) = socket.into_split();
    let mut reader = ConnectionReader::new(reader);
    let mut writer = ConnectionWriter::new(writer);
    let mut buf = BytesMut::with_capacity(4096);
    let mut compression = Compression::Disabled;
    let deadline = tokio::time::Instant::now() + PRE_PLAY_TOTAL_TIMEOUT;
    let mut budget = PrePlayBudget::new(MAX_PRE_PLAY_PACKETS, MAX_PRE_PLAY_BYTES);

    let handshake = before_pre_play_deadline(
        deadline,
        read_packet_with_timeout_budgeted::<Handshake, _>(
            &mut reader,
            &mut buf,
            Compression::Disabled,
            State::Handshake,
            PRE_PLAY_READ_TIMEOUT,
            &mut budget,
        ),
    )
    .await?;
    debug!(
        protocol = handshake.protocol_version,
        address = %handshake.server_address,
        port = handshake.server_port,
        next = ?handshake.next_state,
        "handshake received"
    );

    match handshake.next_state {
        NextState::Status => {
            before_pre_play_deadline(
                deadline,
                status::handle(
                    &mut reader,
                    &mut writer,
                    &mut buf,
                    &mut budget,
                    services.config.as_ref(),
                    &services.sessions,
                    services.runtime_control.as_ref(),
                ),
            )
            .await
        }
        NextState::Login | NextState::Transfer => {
            let outcome = before_pre_play_deadline(
                deadline,
                login::handle(
                    &mut reader,
                    &mut writer,
                    &mut buf,
                    &mut budget,
                    services.config.chunk_pipeline.compression_threshold,
                    &mut compression,
                    services.config.chunk_pipeline.compression_level,
                    services.config.command_permissions.login_access(),
                    services.online_authentication.as_deref(),
                    peer.ip(),
                ),
            )
            .await?;
            let Some(login::LoginOutcome {
                profile,
                properties,
            }) = outcome
            else {
                return Ok(());
            };
            let permissions = services
                .config
                .command_permissions
                .permissions_for(&profile, peer);
            let configuration_outcome = before_pre_play_deadline(
                deadline,
                configuration::handle(
                    &mut reader,
                    &mut writer,
                    &mut buf,
                    &mut budget,
                    compression,
                    &profile,
                    configuration::ConfigurationContext {
                        data: services.config.data.as_ref(),
                        tags: services.config.tags.as_ref(),
                        chunk_geometry: services.chunk_geometry,
                        custom_payload_policy: services
                            .extension
                            .as_ref()
                            .map(ExtensionEventSink::custom_payload_policy),
                        loader_manifest: services
                            .config
                            .loader_manifest
                            .as_deref()
                            .filter(|manifest| !manifest.is_empty()),
                    },
                ),
            )
            .await?;
            drop(pre_auth_permit);
            Box::pin(play::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                &properties,
                permissions,
                services.config.as_ref(),
                services.connection_world,
                services.sessions,
                services.chunk_pipeline_resources,
                services.dirty_flush,
                services.runtime_control,
                services.simulation,
                configuration_outcome.custom_payloads,
                configuration_outcome.loader_session,
                services.extension,
                services.scripts,
                services.script_zones,
            ))
            .await
        }
    }
}

#[cfg(test)]
#[path = "connection_driver_tests.rs"]
mod tests;
