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

/// Vanilla's minimum supported server view distance.
pub const MIN_VIEW_DISTANCE: i32 = 2;
/// Vanilla's maximum supported server view distance.
pub const MAX_VIEW_DISTANCE: i32 = 32;

mod autoscale_soak;
mod blocking;
mod chunk_pipeline;
mod configuration;
mod connection;
mod connection_driver;
mod control_plane;
mod dirty_flush;
pub mod encryption;
mod error;
mod loader;
mod lock_metrics;
mod login;
mod memory_pressure;
mod play;
mod runtime_tick_metrics;
mod script;
mod server;
mod session_auth;
mod status;

pub use autoscale_soak::{
    AutoscalePrimitiveStatus, AutoscaleSoakProfile, AutoscaleSoakReport, AutoscaleSoakScenario,
    AutoscaleSoakSnapshot,
};
pub use chunk_pipeline::{
    ChunkLoadSource, ChunkPipelineCancellationSnapshot, ChunkPipelineGeneration,
    ChunkPipelinePolicy, ChunkPipelineResourceMetrics, ChunkPipelineResourceSnapshot,
    ChunkPipelineStopReason, ChunkPipelineStopReasonCounts, ChunkPriority, ChunkRequest,
    ChunkScheduler, PreparedChunk, automatic_worker_limits,
};
pub use control_plane::{
    AutoscaleAction, AutoscaleDecision, AutoscalePolicy, AutoscalePressure, AutoscaleProfile,
    RuntimeControlConfig, RuntimeControlHandle, RuntimeControlInput, RuntimeControlLimits,
    RuntimeControlPlane, RuntimeControlSnapshot, RuntimeWorkBudgets, RuntimeWorkDecision,
    RuntimeWorkFocus, RuntimeWorkInput,
};
pub use error::ConnectionError;
pub use loader::{
    LOADER_ARTIFACT_CHUNK_BYTES, LOADER_PROTOCOL_VERSION, LoaderArtifactRequest, LoaderBundle,
    LoaderClientAck, LoaderContentKind, LoaderHandshakeError, LoaderManifest, LoaderPermission,
    LoaderPlatform, LoaderSession, loader_ack_channel, loader_artifact_channel,
    loader_manifest_channel, loader_open_screen_channel, loader_request_channel,
};
pub use lock_metrics::{LockMetricSnapshot, LockMetricsSnapshot, lock_pressure_snapshot};
pub use login::{LoginAccessConfig, offline_uuid};
pub use play::{
    DEFAULT_VIEW_DISTANCE, EntityEffectHandle, EntityEffectRequestError, ITEM_DESPAWN_AGE_TICKS,
    PlayerAttackObservation, RandomTickPolicy,
};
pub use runtime_tick_metrics::{RuntimeLatencyPercentiles, RuntimeTickPercentiles};
pub use script::PluginStorageStartError;
pub use server::{
    BoundServer, CommandPermissionConfig, OutboundPressureHandle, OutboundPressureSnapshot,
    RuntimeTelemetryHandle, RuntimeTelemetrySnapshot, SaveAllReport, SaveAllTimings, SaveHandle,
    ServerConfig, ShutdownHandle, WorldHandle, bind, bind_with_extension, bind_with_scripts, run,
};
pub use session_auth::{
    MojangSessionVerifier, RsaIdentity, RsaIdentityError, SessionVerifier,
    SessionVerifierBuildError, SessionVerifierFuture, VerifiedSession, VerifySession,
    VerifySessionError, minecraft_server_hash,
};

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
