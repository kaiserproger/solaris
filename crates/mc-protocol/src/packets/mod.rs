//! Typed packet definitions.
//!
//! Packets are organised by connection *state*. The same packet ID may
//! mean different things in different states, so dispatch is always done
//! via `(state, direction, id)`. Each individual packet type implements
//! [`Packet`] and knows its ID *within its own state/direction*.
//!
//! ```text
//! Handshake     → Status | Login        (transient, single inbound packet)
//! Status        → (closes after pong)
//! Login         → Configuration         (M1.d)
//! Configuration → Play                  (M1.e)
//! Play          → (the rest of the game)
//! ```

use bytes::{Buf, BufMut};

use crate::error::CodecError;

pub mod handshake;
pub mod status;

/// Direction a packet travels: serverbound (client → server) or
/// clientbound (server → client).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Serverbound,
    Clientbound,
}

/// The five top-level connection states a 26.1 connection moves through.
///
/// `Transfer` is technically a sixth `next_state` value in the handshake
/// packet (since 1.20.5), routed similarly to `Login`; we treat it as a
/// `Login` for now and revisit if needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

/// A typed packet. Every packet knows its own ID within its (state,
/// direction) pair and how to encode/decode itself.
///
/// The `Packet` trait deliberately does not expose state/direction — those
/// are properties of the call site (the state machine), not of the type.
pub trait Packet: Sized {
    /// Packet ID, scoped to this packet's (state, direction).
    const ID: i32;

    /// Encode the packet body (without the length prefix or the ID
    /// VarInt — both are added by the framing layer).
    fn encode<B: BufMut>(&self, buf: &mut B) -> Result<(), CodecError>;

    /// Decode the packet body. The leading ID VarInt has already been
    /// consumed by the framing layer.
    fn decode<B: Buf>(buf: &mut B) -> Result<Self, CodecError>;
}
