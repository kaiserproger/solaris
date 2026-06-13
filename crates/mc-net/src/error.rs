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

    #[error("ignored more than {max} non-target packet(s) in state {state:?}")]
    IgnoredPacketBudgetExceeded { state: State, max: usize },

    #[error(
        "client did not acknowledge required known pack {advertised}; full RegistryData payloads are not implemented"
    )]
    MissingKnownPack { advertised: String },
}
