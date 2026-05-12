//! TCP listener, accept loop, and the per-connection state-machine entry
//! point.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::connection::read_packet;
use crate::error::ConnectionError;
use crate::{login, status};

/// Settings the network layer needs to serve.
///
/// Constructed by the caller (`mc-server`) from the user-facing TOML
/// config so the network layer does not depend on the binary's config
/// types.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    pub motd: String,
    pub max_players: u32,
}

/// Bind to `config.bind_address` and run the accept loop until cancelled.
///
/// Each accepted connection is spawned as its own task; an error inside
/// a connection is logged but does not stop the listener.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(config.bind_address).await?;
    let actual = listener.local_addr()?;
    info!(addr = %actual, "Solaris is listening");

    let config = Arc::new(config);
    loop {
        let (socket, peer) = listener.accept().await?;
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

async fn handle_connection(
    socket: tokio::net::TcpStream,
    config: &ServerConfig,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets — same setting
    // vanilla uses.
    socket.set_nodelay(true)?;

    let (mut reader, writer) = socket.into_split();
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
        NextState::Status => status::handle(reader, writer, buf, config).await,
        NextState::Login | NextState::Transfer => {
            let profile = login::handle(reader, writer, buf).await?;
            info!(
                player = %profile.name,
                uuid = %profile.uuid,
                "login complete; configuration state not yet implemented (M1.e), \
                 dropping connection",
            );
            // Configuration state is M1.e. Until it lands we simply
            // close: the vanilla client will display the "connection
            // lost" screen.
            Ok(())
        }
    }
}
