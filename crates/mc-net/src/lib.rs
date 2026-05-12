//! # mc-net
//!
//! Connection management, session lifecycle.
//!
//! Part of the Solaris engine.
//!
//! At M1.c this crate exposes a [`run`] entry point that listens on a TCP
//! address, accepts vanilla 26.1 clients, completes the handshake, and —
//! if the client asked for `Status` — answers a server-list ping. The
//! Login → Configuration → Play path arrives in M1.d / M1.e / M1.g.

mod configuration;
mod connection;
mod error;
mod login;
mod server;
mod status;

pub use error::ConnectionError;
pub use login::offline_uuid;
pub use server::{ServerConfig, run};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
