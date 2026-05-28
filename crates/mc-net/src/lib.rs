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

mod chunk_pipeline;
mod configuration;
mod connection;
mod error;
mod lock_metrics;
mod login;
mod play;
mod server;
mod status;

pub use chunk_pipeline::{
    ChunkLoadSource, ChunkPipelineGeneration, ChunkPipelinePolicy, ChunkPipelineResourceMetrics,
    ChunkPipelineResourceSnapshot, ChunkPipelineStopReason, ChunkPriority, ChunkRequest,
    ChunkScheduler, PreparedChunk,
};
pub use error::ConnectionError;
pub use lock_metrics::{LockMetricSnapshot, LockMetricsSnapshot, lock_pressure_snapshot};
pub use login::offline_uuid;
pub use play::{DEFAULT_VIEW_DISTANCE, RandomTickPolicy};
pub use server::{
    BoundServer, CommandPermissionConfig, OutboundPressureHandle, OutboundPressureSnapshot,
    ServerConfig, ShutdownHandle, WorldHandle, bind, run,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
