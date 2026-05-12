//! # mc-protocol
//!
//! Wire protocol: packets, codec, encryption.
//!
//! Part of the Solaris engine.
//!
//! The crate is organised in layers, each building on the previous:
//!
//! 1. [`codec`] — primitive value encode/decode (varint, string, uuid, …).
//!    This is the only layer touched by M1.a; framing, packet structs and
//!    state-aware dispatch arrive in later sub-milestones (see
//!    `docs/milestones/M1.md`).
//! 2. *(M1.b)* `frame` — length-prefixed framing on top of the codec layer,
//!    optionally with zlib compression and AES/CFB8 encryption.
//! 3. *(M1.c+)* `packets` — typed structs for every serverbound and
//!    clientbound packet we care about, grouped by connection state.

pub mod codec;
mod error;
pub mod frame;

pub use error::CodecError;
pub use frame::{Compression, FramingError, RawFrame};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Minecraft Java Edition wire protocol version Solaris targets.
///
/// Locked at the start of M1 from the bundled vanilla server's
/// `version.json` (`.analysis/server.jar`). When we follow a Mojang patch
/// release we bump this and document the diff in an ADR.
pub const PROTOCOL_VERSION: i32 = 775;

/// The Minecraft Java Edition release Solaris is built against.
pub const TARGET_RELEASE: &str = "26.1.2";

/// The data-pack world version that pairs with [`TARGET_RELEASE`].
pub const WORLD_VERSION: u32 = 4790;
