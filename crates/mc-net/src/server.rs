//! TCP listener, accept loop, and the per-connection state-machine entry
//! point.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_data::Identifier;
use mc_data::VanillaData;
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;
use mc_data::tags::TagsData;
use mc_entity::EntityId;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::chunk::{MAX_Y, MIN_Y};
use mc_world::{BlockPos, BlockRegistry, BlockStateId, WorldStorage};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use crate::ChunkPipelinePolicy;
use crate::connection::read_packet;
use crate::error::ConnectionError;
use crate::{configuration, login, play, status};

const COW_HALF_WIDTH: f64 = 0.46;

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
    pub view_distance: i32,
    pub data: Arc<VanillaData>,
    pub blocks: Arc<BlockRegistry>,
    pub world: Option<WorldHandle>,
    /// Tag set the Configuration handler ships in `UpdateTags` between
    /// the last `RegistryData` and `FinishConfiguration`. May be the
    /// empty default when the sidecar lacks tag JSON; the vanilla
    /// client then complains during registry freeze.
    pub tags: Arc<TagsData>,
    /// Per-block-state light metadata (emission / opacity /
    /// sky-propagation). Loaded from
    /// `data/vanilla/reports/block_light.json` at startup; the
    /// chunk-streaming path uses it to compute light when the Anvil
    /// nibbles are missing. `None` keeps M1/M3 behaviour (the
    /// chunk-stream path falls back to `LightData::empty()` until
    /// M4.e wires the engine in).
    pub block_light: Option<Arc<BlockLightTable>>,
    /// Item registry (M6.c) loaded from
    /// `data/vanilla/reports/registries.json`. Drives the M6
    /// place-from-held-item lookup. May be empty when running tests
    /// that don't care about inventory; the M6 place flow degrades
    /// gracefully (no item → no placement).
    pub items: Arc<ItemRegistry>,
    /// M13 chunk-pipeline policy. Early M13 slices keep the existing
    /// cooperative stream path but thread this policy through so the
    /// scheduler and worker-pool stages have one runtime source of truth.
    pub chunk_pipeline: ChunkPipelinePolicy,
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
    sessions: Arc<play::SessionRegistry>,
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
        let sessions = self.sessions;
        let entity_sessions = Arc::clone(&sessions);
        let entity_config = Arc::clone(&config);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(play::ENTITY_TICK_PERIOD);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut tick = 0_u64;
            loop {
                ticker.tick().await;
                tick = tick.wrapping_add(1);
                let queries = entity_sessions.tick_entities_and_collect_ground_queries(tick);
                let levels = entity_ground_levels(&entity_config, &queries).await;
                entity_sessions.apply_entity_ground_levels_and_dispatch(tick, &levels);
            }
        });
        loop {
            let (socket, peer) = self.listener.accept().await?;
            debug!(%peer, "accepted connection");
            let config = config.clone();
            let sessions = Arc::clone(&sessions);
            tokio::spawn(async move {
                if let Err(err) = handle_connection(socket, &config, sessions).await {
                    match err {
                        err if is_client_disconnect(&err) => {
                            debug!(%peer, "client disconnected");
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

async fn entity_ground_levels(
    config: &ServerConfig,
    queries: &[(EntityId, f64, f64)],
) -> Vec<(EntityId, f64)> {
    let Some(world) = config.world.as_ref() else {
        return Vec::new();
    };
    if queries.is_empty() {
        return Vec::new();
    }
    let air = config
        .blocks
        .block(&Identifier::parse("minecraft:air").expect("static identifier"))
        .map(|block| block.default)
        .unwrap_or(BlockStateId(0));
    let mut storage = world.lock().await;
    queries
        .iter()
        .filter_map(|&(id, x, z)| {
            ground_y_for_bbox(&mut storage, air, x, z, COW_HALF_WIDTH)
                .map(|ground_y| (id, f64::from(ground_y) + 1.0))
        })
        .collect()
}

fn ground_y_for_bbox(
    storage: &mut WorldStorage,
    air: BlockStateId,
    x: f64,
    z: f64,
    half_width: f64,
) -> Option<i32> {
    let probes = [
        (x - half_width, z - half_width),
        (x - half_width, z + half_width),
        (x + half_width, z - half_width),
        (x + half_width, z + half_width),
    ];
    probes
        .into_iter()
        .filter_map(|(px, pz)| ground_y_at(storage, air, px.floor() as i32, pz.floor() as i32))
        .max()
}

fn ground_y_at(storage: &mut WorldStorage, air: BlockStateId, x: i32, z: i32) -> Option<i32> {
    for y in (MIN_Y..MAX_Y).rev() {
        match storage.get_block(BlockPos { x, y, z }) {
            Ok(Some(state)) if state != air => return Some(y),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn is_client_disconnect(err: &ConnectionError) -> bool {
    match err {
        ConnectionError::Eof => true,
        ConnectionError::Io(err) => matches!(
            err.kind(),
            ErrorKind::BrokenPipe
                | ErrorKind::ConnectionAborted
                | ErrorKind::ConnectionReset
                | ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

/// Bind to `config.bind_address` and return a [`BoundServer`] ready to
/// `.serve()`.
pub async fn bind(config: ServerConfig) -> std::io::Result<BoundServer> {
    let listener = TcpListener::bind(config.bind_address).await?;
    Ok(BoundServer {
        listener,
        config: Arc::new(config),
        sessions: Arc::new(play::SessionRegistry::new()),
    })
}

/// Convenience for the binary: `bind` followed by `serve`.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    bind(config).await?.serve().await
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    config: &ServerConfig,
    sessions: Arc<play::SessionRegistry>,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets — same setting
    // vanilla uses.
    socket.set_nodelay(true)?;

    let (mut reader, mut writer) = socket.into_split();
    let mut buf = BytesMut::with_capacity(4096);
    let mut compression = Compression::Disabled;

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
            let profile = login::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                &mut compression,
                config.chunk_pipeline.compression_level,
            )
            .await?;
            configuration::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                &config.data,
                &config.tags,
            )
            .await?;
            play::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                config,
                sessions,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broken_pipe_is_graceful_disconnect() {
        let err = ConnectionError::Io(std::io::Error::from(ErrorKind::BrokenPipe));
        assert!(is_client_disconnect(&err));
    }

    #[test]
    fn codec_error_is_not_graceful_disconnect() {
        let err = ConnectionError::UnexpectedPacketId {
            state: State::Login,
            expected: 1,
            got: 2,
        };
        assert!(!is_client_disconnect(&err));
    }
}
