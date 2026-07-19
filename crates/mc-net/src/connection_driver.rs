//! Per-connection protocol state orchestration.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::ChunkGeometry;
use tracing::debug;

use crate::chunk_pipeline::ChunkPipelineResources;
use crate::connection::{
    ConnectionReader, ConnectionWriter, PRE_PLAY_READ_TIMEOUT, read_packet_with_timeout,
};
use crate::error::ConnectionError;
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
    pub(crate) runtime_control: Option<RuntimeControlHandle>,
    pub(crate) simulation: play::SimulationHandle,
    pub(crate) extension: Option<ExtensionEventSink>,
    pub(crate) scripts: Option<ScriptEventSink>,
}

pub(crate) async fn handle_connection(
    socket: tokio::net::TcpStream,
    peer: SocketAddr,
    services: ConnectionServices,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets, matching vanilla.
    socket.set_nodelay(true)?;

    let (reader, writer) = socket.into_split();
    let mut reader = ConnectionReader::new(reader);
    let mut writer = ConnectionWriter::new(writer);
    let mut buf = BytesMut::with_capacity(4096);
    let mut compression = Compression::Disabled;

    let handshake = read_packet_with_timeout::<Handshake, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Handshake,
        PRE_PLAY_READ_TIMEOUT,
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
            status::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                services.config.as_ref(),
                &services.sessions,
                services.runtime_control.as_ref(),
            )
            .await
        }
        NextState::Login | NextState::Transfer => {
            let outcome = login::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                services.config.chunk_pipeline.compression_threshold,
                &mut compression,
                services.config.chunk_pipeline.compression_level,
                services.config.command_permissions.login_access(),
                services.online_authentication.as_deref(),
                peer.ip(),
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
            let configuration_custom_payloads = configuration::handle(
                &mut reader,
                &mut writer,
                &mut buf,
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
                },
            )
            .await?;
            play::handle(
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
                services.runtime_control,
                services.simulation,
                configuration_custom_payloads,
                services.extension,
                services.scripts,
            )
            .await
        }
    }
}
