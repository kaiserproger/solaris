//! Per-connection errors.

use mc_protocol::{CodecError, FramingError, State};
use mc_world::WorldError;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("framing error: {0}")]
    Framing(#[from] FramingError),

    #[error("codec error: {0}")]
    Codec(#[from] CodecError),

    #[error("world error: {0}")]
    World(#[from] WorldError),

    /// The peer closed the socket before sending us a complete packet.
    #[error("peer disconnected mid-packet")]
    Eof,

    /// A peer kept extending one incomplete inbound frame beyond the
    /// serverbound buffering budget. This is deliberately lower than the
    /// protocol's clientbound frame ceiling because Solaris accepts no
    /// serverbound packet remotely close to that size.
    #[error("inbound packet buffer exceeded {max} bytes")]
    InboundBufferLimitExceeded { max: usize },

    /// The peer sent a packet whose ID is not the one expected in the
    /// current state. We use this both during the handshake (where we
    /// expect exactly one packet, of one ID) and inside each state for
    /// the small set of packets we currently know how to handle.
    #[error("unexpected packet id {got:#x} in state {state:?}, expected {expected:#x}")]
    UnexpectedPacketId {
        state: State,
        expected: i32,
        got: i32,
    },

    /// A packet's body had trailing unconsumed bytes after decoding.
    /// Vanilla rejects this; we do too.
    #[error("packet of id {id:#x} in state {state:?} left {trailing} unconsumed byte(s)")]
    TrailingBytes {
        state: State,
        id: i32,
        trailing: usize,
    },

    #[error("timed out after {timeout:?} waiting for a packet in state {state:?}")]
    ReadTimeout { state: State, timeout: Duration },

    #[error("pre-Play protocol deadline exceeded after {timeout:?}")]
    PrePlayDeadlineExceeded { timeout: Duration },

    #[error("pre-Play packet budget exceeded: {packets} packets, maximum {max}")]
    PrePlayPacketBudgetExceeded { packets: usize, max: usize },

    #[error("pre-Play byte budget exceeded: {bytes} bytes, maximum {max}")]
    PrePlayByteBudgetExceeded { bytes: usize, max: usize },

    #[error("Play ingress rate limit exceeded for {class} after {violations} violations")]
    PlayRateLimitExceeded {
        class: &'static str,
        violations: u32,
    },

    #[error("outbound socket write remained blocked for {timeout:?}")]
    WriteTimeout { timeout: Duration },

    #[error("ignored more than {max} non-target packet(s) in state {state:?}")]
    IgnoredPacketBudgetExceeded { state: State, max: usize },

    #[error("online-mode authentication failed: {0}")]
    OnlineAuthentication(&'static str),

    #[error("runtime unavailable while {operation}")]
    RuntimeUnavailable { operation: &'static str },

    #[error("invalid non-finite player movement")]
    InvalidPlayerMovement,

    #[error("chunk preparation failed at ({chunk_x},{chunk_z}): {reason}")]
    ChunkPreparation {
        chunk_x: i32,
        chunk_z: i32,
        reason: String,
    },

    #[error(
        "client did not acknowledge required known pack {advertised} and full sidecar RegistryData payloads are unavailable"
    )]
    MissingKnownPack { advertised: String },

    #[error("full RegistryData payload index is missing {entry} from {registry}")]
    MissingRegistryPayload { registry: String, entry: String },

    #[error("Solaris Loader handshake failed: {reason}")]
    LoaderHandshake { reason: String },
}
