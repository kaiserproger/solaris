//! TCP listener, accept loop, and the per-connection state-machine entry
//! point.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_data::VanillaData;
use mc_data::tags::TagsData;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::{BlockRegistry, WorldStorage};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::connection::read_packet;
use crate::error::ConnectionError;
use crate::{configuration, login, play, status};

/// Shared, mutably-accessible handle to the world.
///
/// `WorldStorage::get_chunk` is `&mut self` — it touches an internal
/// LRU on every call — so we wrap it in a tokio Mutex. The mutex is
/// async because chunk reads will eventually await disk I/O (M3.f's
/// region cache + worker pool).
pub type WorldHandle = Arc<Mutex<WorldStorage>>;

/// Settings the network layer needs to serve.
///
/// Constructed by the caller (`mc-server`) from the user-facing TOML
/// config so the network layer does not depend on the binary's config
/// types. `data` is the in-memory index of vanilla registries the
/// Configuration state hands back to clients. `blocks` is the block-
/// state registry built from `blocks.json` — needed by the chunk-data
/// path in M3+; held even when no `world` is configured. `world` is
/// the open world handle when `[data].world_dir` is set; `None` keeps
/// the M1-style chunkless Play state intact.
///
/// `Debug` is not derived: neither `BlockRegistry` nor `WorldStorage`
/// implements `Debug`. Connection-scope logging uses individual fields
/// instead of `{:?}`-printing the whole config.
#[derive(Clone)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    pub motd: String,
    pub max_players: u32,
    pub data: Arc<VanillaData>,
    pub blocks: Arc<BlockRegistry>,
    pub world: Option<WorldHandle>,
    /// Tag set the Configuration handler ships in `UpdateTags` between
    /// the last `RegistryData` and `FinishConfiguration`. May be the
    /// empty default when the sidecar lacks tag JSON; the vanilla
    /// client then complains during registry freeze.
    pub tags: Arc<TagsData>,
}

/// A listener that has been successfully bound but is not yet serving.
///
/// Holding the listener and the accept loop in two distinct steps lets
/// callers (including the integration tests) learn the assigned port
/// when binding to `0.0.0.0:0` *without* a probe/drop/rebind dance that
/// races against the OS reusing the same ephemeral port.
pub struct BoundServer {
    listener: TcpListener,
    config: Arc<ServerConfig>,
}

impl BoundServer {
    /// The socket address the listener is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Accept connections forever, spawning a per-connection task each
    /// time. An error inside a connection task is logged but does not
    /// stop the listener.
    pub async fn serve(self) -> std::io::Result<()> {
        info!(
            addr = %self.local_addr()?,
            registries = self.config.data.registry_count(),
            entries = self.config.data.entry_count(),
            "Solaris is listening"
        );
        let config = self.config;
        loop {
            let (socket, peer) = self.listener.accept().await?;
            debug!(%peer, "accepted connection");
            let config = config.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(socket, &config).await {
                    match err {
                        ConnectionError::Eof => {
                            debug!(%peer, "client closed before completing");
                        }
                        other => {
                            warn!(%peer, error = %other, "connection terminated");
                        }
                    }
                } else {
                    debug!(%peer, "connection finished");
                }
            });
        }
    }
}

/// Bind to `config.bind_address` and return a [`BoundServer`] ready to
/// `.serve()`.
pub async fn bind(config: ServerConfig) -> std::io::Result<BoundServer> {
    let listener = TcpListener::bind(config.bind_address).await?;
    Ok(BoundServer {
        listener,
        config: Arc::new(config),
    })
}

/// Convenience for the binary: `bind` followed by `serve`.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    bind(config).await?.serve().await
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    config: &ServerConfig,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets — same setting
    // vanilla uses.
    socket.set_nodelay(true)?;

    let (mut reader, mut writer) = socket.into_split();
    let mut buf = BytesMut::with_capacity(4096);

    let handshake = read_packet::<Handshake, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Handshake,
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
        NextState::Status => status::handle(&mut reader, &mut writer, &mut buf, config).await,
        NextState::Login | NextState::Transfer => {
            let profile = login::handle(&mut reader, &mut writer, &mut buf).await?;
            configuration::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                &profile,
                &config.data,
                &config.tags,
            )
            .await?;
            play::handle(&mut reader, &mut writer, &mut buf, &profile, config).await
        }
    }
}
