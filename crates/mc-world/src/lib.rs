//! # mc-world
//!
//! Block states, chunk format, world storage.
//!
//! Part of the Solaris engine.

pub mod block;

pub use block::{Block, BlockRegistry, BlockState, BlockStateId, RegistryError};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
