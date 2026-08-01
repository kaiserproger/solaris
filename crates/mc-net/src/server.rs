//! TCP listener, accept loop, and server supervision.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::io::{BufRead, ErrorKind};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mc_data::Identifier;
use mc_data::VanillaData;
use mc_data::biomes::BiomeSpawnRules;
use mc_data::block_facts::{BlockFactsTable, FluidKind};
use mc_data::block_light::BlockLightTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_data::recipes::Recipe;
use mc_data::tags::TagsData;
use mc_extension::{
    CustomPayloadPolicy, CustomPayloadRejection, ExtensionBoundary, InboundEvent,
    OutboundCommand as ExtensionOutboundCommand, PlayerId, ProtocolPhase, QueueError,
    QueueRecvError,
};
use mc_physics::{
    BlockCollisionBox, BlockCollisionHeight, BlockMaterial, BlockMaterialIds, BlockSampler,
    EntityBody, PhysicsConfig,
};
use mc_script::{
    AdmittedScriptCommand, ScriptBoundary, ScriptCommand, ScriptEvent, ScriptPlayerContext,
    ScriptQueueError,
};
use mc_world::{BlockRegistry, ChunkGeometry, MAX_Y, MIN_Y, OVERWORLD_GEOMETRY, WorldStorage};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, Semaphore, broadcast};
use tracing::{debug, info, warn};

use crate::admission::PreAuthAdmission;
use crate::chunk_pipeline::ChunkPipelineResources;
use crate::connection_driver::{ConnectionServices, handle_connection};
use crate::control_plane::{
    RuntimeControlApplyError, RuntimeControlOperation, RuntimeControlOutcome, RuntimeControlSignal,
    RuntimeControlSignalReceiver,
};
use crate::error::ConnectionError;
use crate::runtime_tick_metrics::{
    RuntimeTickMetricsHandle, RuntimeTickMetricsWindow, RuntimeTickPercentiles, RuntimeTickSample,
    spawn_runtime_tick_metrics_worker,
};
use crate::script::{PluginStorageHandle, PluginZoneAdapter, ScriptRouter, ScriptRouterExit};
use crate::{
    ChunkPipelinePolicy, RuntimeControlHandle, RuntimeControlInput, RuntimeWorkBudgets,
    RuntimeWorkInput,
};
use crate::{login, play};

mod natural_spawn_ticker;

static CONSOLE_LINES: OnceLock<broadcast::Sender<String>> = OnceLock::new();
type PhysicsMaterialCache = HashMap<
    (usize, usize),
    (
        std::sync::Weak<BlockRegistry>,
        std::sync::Weak<BlockFactsTable>,
        Arc<BlockMaterialIds>,
    ),
>;

static PHYSICS_MATERIAL_CACHE: OnceLock<std::sync::Mutex<PhysicsMaterialCache>> = OnceLock::new();
const CONNECTION_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_CONNECTION_TASKS: usize = 32;
const MAX_CONNECTION_TASKS: usize = 512;
const MIN_PRE_AUTH_CONNECTIONS: usize = 16;
const MAX_PRE_AUTH_CONNECTIONS: usize = 128;
const MAX_PRE_AUTH_CONNECTIONS_PER_IP: usize = 4;
const ENTITY_TICKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const SCRIPT_COMMIT_FORWARD_TIMEOUT: Duration = Duration::from_secs(5);
const DIRTY_ONLY_FLUSH_MAX_CHUNKS: usize = 64;
const DIRTY_ONLY_FLUSH_STALE_REGION_RETRIES: usize = 3;
const SLOW_SIMULATION_ATTRIBUTION_LIMIT: usize = 8;

fn connection_task_limit(max_players: u32) -> usize {
    usize::try_from(max_players)
        .unwrap_or(usize::MAX)
        .saturating_mul(2)
        .saturating_add(16)
        .clamp(MIN_CONNECTION_TASKS, MAX_CONNECTION_TASKS)
}

fn pre_auth_connection_limit(max_players: u32) -> usize {
    usize::try_from(max_players)
        .unwrap_or(usize::MAX)
        .saturating_add(8)
        .clamp(MIN_PRE_AUTH_CONNECTIONS, MAX_PRE_AUTH_CONNECTIONS)
        .min(connection_task_limit(max_players))
}

#[derive(Debug, Clone, Default)]
pub struct CommandPermissionConfig {
    operators: BTreeSet<String>,
    allow_local_dev_operators: bool,
    login_access: login::LoginAccessConfig,
}

impl CommandPermissionConfig {
    #[must_use]
    pub fn new<I, S>(operators: I, allow_local_dev_operators: bool) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            operators: operators
                .into_iter()
                .filter_map(|entry| {
                    let entry: String = entry.into();
                    let normalized = entry.trim().to_ascii_lowercase();
                    (!normalized.is_empty()).then_some(normalized)
                })
                .collect(),
            allow_local_dev_operators,
            login_access: login::LoginAccessConfig::offline_only(),
        }
    }

    #[must_use]
    pub fn with_login_access(mut self, login_access: login::LoginAccessConfig) -> Self {
        self.login_access = login_access;
        self
    }

    #[must_use]
    pub fn from_operators<I, S>(operators: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(operators, false)
    }

    #[must_use]
    pub(crate) fn permissions_for(
        &self,
        profile: &login::LoggedInProfile,
        peer: SocketAddr,
    ) -> play::commands::CommandPermissions {
        play::commands::CommandPermissions::from_op(self.is_operator(profile, peer))
    }

    #[must_use]
    fn is_operator(&self, profile: &login::LoggedInProfile, peer: SocketAddr) -> bool {
        if self.operators.is_empty() && self.allow_local_dev_operators && is_loopback_peer(peer) {
            return true;
        }
        self.operators.contains(&profile.name.to_ascii_lowercase())
            || self
                .operators
                .contains(&profile.uuid.to_string().to_ascii_lowercase())
    }

    pub(crate) fn login_access(&self) -> &login::LoginAccessConfig {
        &self.login_access
    }
}

fn is_loopback_peer(peer: SocketAddr) -> bool {
    match peer.ip() {
        std::net::IpAddr::V4(ip) => ip.is_loopback(),
        std::net::IpAddr::V6(ip) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|ip| ip.is_loopback())
        }
    }
}

#[derive(Debug)]
struct EntityOwnerServeError {
    error: mc_entity::RegionOwnerLaneError,
}

impl std::fmt::Display for EntityOwnerServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "regional entity owner entered fatal state: {:?}",
            self.error
        )
    }
}

impl std::error::Error for EntityOwnerServeError {}

fn entity_owner_serve_error(error: mc_entity::RegionOwnerLaneError) -> std::io::Error {
    std::io::Error::other(EntityOwnerServeError { error })
}

fn is_entity_owner_serve_error(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|inner| inner.is::<EntityOwnerServeError>())
}

#[derive(Debug)]
struct PoisonedRuntimeServeError {
    lock: &'static str,
}

impl std::fmt::Display for PoisonedRuntimeServeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "authoritative runtime lock poisoned: {}",
            self.lock
        )
    }
}

impl std::error::Error for PoisonedRuntimeServeError {}

fn poisoned_runtime_serve_error(lock: &'static str) -> std::io::Error {
    std::io::Error::other(PoisonedRuntimeServeError { lock })
}

fn is_uncertain_runtime_serve_error(error: &std::io::Error) -> bool {
    is_entity_owner_serve_error(error)
        || error
            .get_ref()
            .is_some_and(|inner| inner.is::<PoisonedRuntimeServeError>())
}

#[derive(Clone, Default)]
pub struct ShutdownHandle {
    requested: Arc<AtomicBool>,
    notify: Arc<Notify>,
    save_coordinator: Arc<Mutex<()>>,
    dirty_tail_generation: Arc<AtomicU64>,
    dirty_tail_notify: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMetricsPolicy {
    pub log_interval_ticks: u64,
    pub slow_tick_ms: u64,
}

#[derive(Debug, Default)]
struct RuntimeMetricsLogGate {
    slow_episode_active: bool,
}

impl RuntimeMetricsLogGate {
    fn should_log(&mut self, tick: u64, tick_us: u64, policy: RuntimeMetricsPolicy) -> bool {
        let periodic = tick.is_multiple_of(policy.log_interval_ticks);
        if !is_slow_tick(tick_us, policy) {
            self.slow_episode_active = false;
            return periodic;
        }
        let should_log = !self.slow_episode_active || periodic;
        self.slow_episode_active = true;
        should_log
    }
}

impl Default for RuntimeMetricsPolicy {
    fn default() -> Self {
        Self {
            log_interval_ticks: 100,
            slow_tick_ms: 50,
        }
    }
}

impl RuntimeMetricsPolicy {
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            log_interval_ticks: self.log_interval_ticks.max(1),
            slow_tick_ms: self.slow_tick_ms,
        }
    }
}

impl ShutdownHandle {
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    pub async fn wait_requested(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            let _ = notified.as_mut().enable();

            if self.is_requested() {
                return;
            }

            notified.await;
        }
    }

    pub(crate) async fn notified(&self) {
        self.wait_requested().await;
    }

    fn save_coordinator(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.save_coordinator)
    }

    fn mark_dirty_tail_progress(&self) {
        self.dirty_tail_generation.fetch_add(1, Ordering::Release);
        self.dirty_tail_notify.notify_waiters();
    }

    fn dirty_tail_generation(&self) -> u64 {
        self.dirty_tail_generation.load(Ordering::Acquire)
    }

    async fn wait_for_dirty_tail_progress(&self, observed: u64) {
        loop {
            let notified = self.dirty_tail_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.dirty_tail_generation() != observed {
                return;
            }
            notified.await;
        }
    }
}

/// Shared, mutably-accessible handle to the world.
///
/// `WorldStorage::get_chunk` is `&mut self` — it touches an internal
/// LRU on every call — so we wrap it in a tokio Mutex. The mutex is
/// async because chunk reads will eventually await disk I/O (M3.f's
/// region cache + worker pool).
pub type WorldHandle = Arc<Mutex<WorldStorage>>;

#[derive(Clone, Default)]
pub(crate) struct ConnectionWorld {
    pub(crate) root: Option<Arc<std::path::PathBuf>>,
    pub(crate) read: Option<mc_world::WorldReadView>,
    pub(crate) mutation: Option<mc_world::WorldMutationView>,
    pub(crate) chunk_source: Option<mc_world::ChunkSourceView>,
}

fn loaded_block_tick_due(
    scheduled_ticks: &mc_world::ScheduledTickView,
    loaded_chunks: &[(i32, i32)],
    world_tick: u64,
) -> bool {
    loaded_chunks
        .iter()
        .any(|&(x, z)| scheduled_ticks.block_due(mc_world::ChunkPos { x, z }, world_tick))
}

fn loaded_fluid_tick_due(
    scheduled_ticks: &mc_world::ScheduledTickView,
    loaded_chunks: &[(i32, i32)],
    world_tick: u64,
) -> bool {
    loaded_chunks
        .iter()
        .any(|&(x, z)| scheduled_ticks.fluid_due(mc_world::ChunkPos { x, z }, world_tick))
}

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
    pub recipes: Arc<Vec<Recipe>>,
    pub loot: Arc<LootTables>,
    /// Per-block-state light metadata (emission / opacity /
    /// sky-propagation). Built by `mc-server` from the required block
    /// report at startup; the chunk-streaming path uses it to compute
    /// light when the Anvil nibbles are missing. `None` is kept for
    /// narrow protocol tests that do not exercise chunk lighting.
    pub block_light: Option<Arc<BlockLightTable>>,
    /// Item registry (M6.c) loaded from
    /// `data/vanilla/reports/registries.json`. Drives the M6
    /// place-from-held-item lookup. May be empty when running tests
    /// that don't care about inventory; the M6 place flow degrades
    /// gracefully (no item → no placement).
    pub items: Arc<ItemRegistry>,
    pub item_facts: Arc<ItemFactsTable>,
    pub block_facts: Arc<BlockFactsTable>,
    pub entity_types: Arc<EntityTypeRegistry>,
    pub biome_spawns: Arc<BiomeSpawnRules>,
    /// M13 chunk-pipeline policy. Early M13 slices keep the existing
    /// cooperative stream path but thread this policy through so the
    /// scheduler and worker-pool stages have one runtime source of truth.
    pub chunk_pipeline: ChunkPipelinePolicy,
    pub random_tick: play::RandomTickPolicy,
    pub command_permissions: CommandPermissionConfig,
    /// Required Solaris Loader bundles negotiated during Configuration.
    /// `None` leaves vanilla clients on the existing handshake.
    pub loader_manifest: Option<Arc<crate::LoaderManifest>>,
    pub shutdown: ShutdownHandle,
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
    online_authentication: Option<Arc<login::OnlineAuthentication>>,
    chunk_geometry: ChunkGeometry,
    connection_world: ConnectionWorld,
    chunk_pipeline_resources: ChunkPipelineResources,
    runtime_control: Option<RuntimeControlHandle>,
    runtime_tick_metrics: RuntimeTickMetricsHandle,
    sessions: Arc<play::SessionRegistry>,
    simulation: play::SimulationHandle,
    simulation_owner: play::SimulationOwner,
    extension: Option<ExtensionEventSink>,
    scripts: Option<ScriptEventSink>,
    script_storage: Option<PluginStorageHandle>,
    script_zones: Option<PluginZoneAdapter>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeTelemetrySnapshot {
    pub tick_percentiles: Option<crate::RuntimeTickPercentiles>,
    pub active_sessions: usize,
    pub ticketed_chunks: usize,
    pub prepared_chunks: usize,
    pub server_entities: usize,
    pub furnace_viewer_sets: usize,
    pub chest_viewer_sets: usize,
    pub entity_spawn_dispatches: u64,
    pub entity_move_dispatches: u64,
    pub entity_data_dispatches: u64,
    pub entity_take_dispatches: u64,
    pub entity_remove_dispatches: u64,
    pub simulation_queue_capacity: usize,
    pub simulation_queue_depth: usize,
    pub simulation_queue_max_depth: usize,
    pub simulation_commands_enqueued: u64,
    pub simulation_commands_dequeued: u64,
    pub simulation_commands_processed: u64,
    pub simulation_item_pickups_processed: u64,
    pub simulation_block_edits_processed: u64,
    pub simulation_container_commits_processed: u64,
    pub simulation_block_entity_commits_processed: u64,
    pub simulation_commands_rejected_full: u64,
    pub simulation_commands_rejected_closed: u64,
    pub simulation_commands_rejected_shutdown: u64,
    pub simulation_commands_rejected_world_busy: u64,
    pub simulation_commands_rejected_world_unavailable: u64,
    pub simulation_commands_rejected_world_mutation: u64,
    pub simulation_commands_rejected_stale_session: u64,
    pub simulation_commands_cancelled: u64,
    pub simulation_max_batch: usize,
    pub memory_used_mb: u64,
    pub memory_limit_mb: u64,
    pub memory_sample_available: bool,
    pub memory_sample_failures: u64,
}

#[cfg(feature = "load-bench")]
#[derive(Debug, Clone)]
pub struct LoadBenchEntitySpec {
    pub type_id: i32,
    pub type_name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[cfg(feature = "load-bench")]
impl LoadBenchEntitySpec {
    #[must_use]
    pub fn new(type_id: i32, type_name: impl Into<String>, x: f64, y: f64, z: f64) -> Self {
        Self {
            type_id,
            type_name: type_name.into(),
            x,
            y,
            z,
        }
    }
}

#[cfg(feature = "load-bench")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadBenchSeedReport {
    pub entities: usize,
    pub hostile_entities: usize,
    pub regions: usize,
    pub max_entities_per_region: usize,
    pub spawn_dispatches: usize,
    pub owner_lanes: usize,
}

#[cfg(feature = "load-bench")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadBenchReadinessReport {
    pub sessions: usize,
    pub desired_chunks: usize,
    pub desired_loaded_chunks: usize,
    pub pending_chunks: usize,
    pub min_desired_loaded_chunks: usize,
    pub max_desired_loaded_chunks: usize,
    pub visible_entity_links: usize,
    pub owner_entities: usize,
    pub active_simulation_entities: usize,
    pub active_hostile_entities: usize,
    pub prepared_chunks: usize,
    pub prepared_in_flight: usize,
    pub pending_subscriber_chunks: usize,
    pub pending_subscribers: usize,
    pub entity_update_budget_per_lane: usize,
    pub entity_update_budget_total: usize,
    pub entity_update_selected: usize,
    pub entity_update_active_population: usize,
    pub entity_update_rotation_ticks: usize,
    pub entity_movement_publication_budget: usize,
}

#[cfg(feature = "load-bench")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LoadBenchActivityReport {
    pub active_simulation_entities: usize,
    pub active_hostile_entities: usize,
    pub entity_update_budget_per_lane: usize,
    pub entity_update_budget_total: usize,
    pub entity_update_selected: usize,
    pub entity_update_active_population: usize,
    pub entity_update_rotation_ticks: usize,
    pub entity_movement_publication_budget: usize,
}

#[cfg(feature = "load-bench")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LoadBenchSimulationCommandStat {
    pub kind: String,
    pub count: u64,
    pub total_us: u64,
    pub max_us: u64,
}

#[cfg(feature = "load-bench")]
#[derive(Clone)]
pub struct LoadBenchHandle {
    sessions: Arc<play::SessionRegistry>,
    simulation: play::SimulationHandle,
}

#[cfg(feature = "load-bench")]
impl LoadBenchHandle {
    #[must_use]
    pub fn seed_entities(&self, specs: Vec<LoadBenchEntitySpec>) -> LoadBenchSeedReport {
        let entities = specs
            .into_iter()
            .map(|spec| {
                mc_entity::SpawnEntity::new(
                    spec.type_id,
                    spec.type_name,
                    mc_entity::Vec3::new(spec.x, spec.y, spec.z),
                )
            })
            .collect();
        let seeded = self.sessions.seed_load_bench_entities(entities);
        LoadBenchSeedReport {
            entities: seeded.entities,
            hostile_entities: seeded.hostile_entities,
            regions: seeded.regions,
            max_entities_per_region: seeded.max_entities_per_region,
            spawn_dispatches: seeded.spawn_dispatches,
            owner_lanes: seeded.owner_lanes,
        }
    }

    #[must_use]
    pub fn readiness(&self) -> LoadBenchReadinessReport {
        let readiness = self.sessions.load_bench_readiness();
        LoadBenchReadinessReport {
            sessions: readiness.sessions,
            desired_chunks: readiness.desired_chunks,
            desired_loaded_chunks: readiness.desired_loaded_chunks,
            pending_chunks: readiness.pending_chunks,
            min_desired_loaded_chunks: readiness.min_desired_loaded_chunks,
            max_desired_loaded_chunks: readiness.max_desired_loaded_chunks,
            visible_entity_links: readiness.visible_entity_links,
            owner_entities: readiness.owner_entities,
            active_simulation_entities: readiness.active_simulation_entities,
            active_hostile_entities: readiness.active_hostile_entities,
            prepared_chunks: readiness.prepared_chunks,
            prepared_in_flight: readiness.prepared_in_flight,
            pending_subscriber_chunks: readiness.pending_subscriber_chunks,
            pending_subscribers: readiness.pending_subscribers,
            entity_update_budget_per_lane: readiness.entity_update_budget_per_lane,
            entity_update_budget_total: readiness.entity_update_budget_total,
            entity_update_selected: readiness.entity_update_selected,
            entity_update_active_population: readiness.entity_update_active_population,
            entity_update_rotation_ticks: readiness.entity_update_rotation_ticks,
            entity_movement_publication_budget: readiness.entity_movement_publication_budget,
        }
    }

    pub fn reset_simulation_command_stats(&self) {
        self.simulation.reset_command_kind_stats();
    }

    #[must_use]
    pub fn activity(&self) -> LoadBenchActivityReport {
        let activity = self.sessions.load_bench_activity();
        LoadBenchActivityReport {
            active_simulation_entities: activity.active_simulation_entities,
            active_hostile_entities: activity.active_hostile_entities,
            entity_update_budget_per_lane: activity.entity_update_budget_per_lane,
            entity_update_budget_total: activity.entity_update_budget_total,
            entity_update_selected: activity.entity_update_selected,
            entity_update_active_population: activity.entity_update_active_population,
            entity_update_rotation_ticks: activity.entity_update_rotation_ticks,
            entity_movement_publication_budget: activity.entity_movement_publication_budget,
        }
    }

    #[must_use]
    pub fn simulation_command_stats(&self) -> Vec<LoadBenchSimulationCommandStat> {
        self.simulation
            .command_kind_snapshot()
            .into_iter()
            .map(|stat| LoadBenchSimulationCommandStat {
                kind: stat.kind.to_owned(),
                count: stat.count,
                total_us: stat.total_us,
                max_us: stat.max_us,
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct EntityBehaviorHandle {
    sessions: Arc<play::SessionRegistry>,
}

impl EntityBehaviorHandle {
    pub fn configure_mob_behavior_table(
        &self,
        table: mc_data::mob_behavior_26_1_2::MobBehaviorTable,
    ) -> Result<(), mc_data::mob_behavior_26_1_2::MobBehaviorError> {
        self.sessions.configure_mob_behavior_table(table)
    }

    pub fn configure_villager_brain_profile(
        &self,
        profile: mc_entity::villager_26_1_2::VillagerBrainProfile,
    ) -> Result<(), mc_entity::villager_26_1_2::VillagerBrainError> {
        self.sessions.configure_villager_brain_profile(profile)
    }
}

#[derive(Clone)]
pub struct RuntimeTelemetryHandle {
    tick_metrics: RuntimeTickMetricsHandle,
    sessions: Arc<play::SessionRegistry>,
    runtime_control: Option<RuntimeControlHandle>,
    simulation: play::SimulationHandle,
}

impl RuntimeTelemetryHandle {
    /// Subscribe to exact simulation-tick progress notifications.
    #[must_use]
    pub fn subscribe_simulation_ticks(&self) -> tokio::sync::watch::Receiver<u64> {
        self.sessions.subscribe_simulation_ticks()
    }

    /// Subscribe to accepted attacks in authority order.
    ///
    /// The channel is bounded. A slow receiver gets `RecvError::Lagged` and
    /// must treat the missing observations as a failed telemetry sample.
    #[must_use]
    pub fn subscribe_player_attacks(
        &self,
    ) -> tokio::sync::broadcast::Receiver<play::PlayerAttackObservation> {
        self.sessions.subscribe_player_attacks()
    }

    /// Subscribe to exact play-session register and unregister notifications.
    #[must_use]
    pub fn subscribe_active_sessions(&self) -> tokio::sync::watch::Receiver<usize> {
        self.sessions.subscribe_active_sessions()
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeTelemetrySnapshot {
        let pressure = self.sessions.pressure_snapshot();
        let simulation = self.simulation.snapshot();
        let memory = self
            .runtime_control
            .as_ref()
            .map(RuntimeControlHandle::memory_pressure_observation)
            .unwrap_or_default();
        RuntimeTelemetrySnapshot {
            tick_percentiles: self.tick_metrics.snapshot(),
            active_sessions: pressure.sessions,
            ticketed_chunks: pressure.ticketed_chunks,
            prepared_chunks: pressure.prepared_chunks,
            server_entities: pressure.server_entities,
            furnace_viewer_sets: pressure.furnace_viewer_sets,
            chest_viewer_sets: pressure.chest_viewer_sets,
            entity_spawn_dispatches: pressure.entity_dispatches.spawn,
            entity_move_dispatches: pressure.entity_dispatches.move_relative,
            entity_data_dispatches: pressure.entity_dispatches.data,
            entity_take_dispatches: pressure.entity_dispatches.take,
            entity_remove_dispatches: pressure.entity_dispatches.remove,
            simulation_queue_capacity: simulation.capacity,
            simulation_queue_depth: simulation.depth,
            simulation_queue_max_depth: simulation.max_depth,
            simulation_commands_enqueued: simulation.enqueued,
            simulation_commands_dequeued: simulation.dequeued,
            simulation_commands_processed: simulation.processed,
            simulation_item_pickups_processed: simulation.item_pickups_processed,
            simulation_block_edits_processed: simulation.block_edits_processed,
            simulation_container_commits_processed: simulation.container_commits_processed,
            simulation_block_entity_commits_processed: simulation.block_entity_commits_processed,
            simulation_commands_rejected_full: simulation.rejected_full,
            simulation_commands_rejected_closed: simulation.rejected_closed,
            simulation_commands_rejected_shutdown: simulation.rejected_shutdown,
            simulation_commands_rejected_world_busy: simulation.rejected_world_busy,
            simulation_commands_rejected_world_unavailable: simulation.rejected_world_unavailable,
            simulation_commands_rejected_world_mutation: simulation.rejected_world_mutation,
            simulation_commands_rejected_stale_session: simulation.rejected_stale_session,
            simulation_commands_cancelled: simulation.cancelled,
            simulation_max_batch: simulation.max_batch,
            memory_used_mb: memory.sample.used_mb,
            memory_limit_mb: memory.sample.limit_mb,
            memory_sample_available: memory.available,
            memory_sample_failures: memory.failures,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ExtensionEventSink {
    boundary: ExtensionBoundary,
    custom_payload_policy: CustomPayloadPolicy,
}

impl ExtensionEventSink {
    fn new(boundary: ExtensionBoundary, custom_payload_policy: CustomPayloadPolicy) -> Self {
        Self {
            boundary,
            custom_payload_policy,
        }
    }

    pub(crate) fn enqueue_event(&self, event: InboundEvent) {
        match self.boundary.try_enqueue_event(event) {
            Ok(()) => {}
            Err(QueueError::Full(_)) => {
                warn!("extension event queue full; dropping event");
            }
            Err(QueueError::Closed(_)) => {
                warn!("extension event queue closed; dropping event");
            }
            Err(_) => {
                warn!("extension event queue rejected event");
            }
        }
    }

    pub(crate) fn enqueue_custom_payload(
        &self,
        player_id: PlayerId,
        phase: ProtocolPhase,
        channel: &str,
        payload: &[u8],
    ) {
        match self
            .custom_payload_policy
            .build_event(player_id, phase, channel, payload)
        {
            Ok(event) => self.enqueue_event(InboundEvent::CustomPayload(event)),
            Err(CustomPayloadRejection::UnknownChannel { channel }) => {
                debug!(channel = %channel, phase = ?phase, "extension custom payload denied by policy");
            }
            Err(CustomPayloadRejection::PayloadTooLarge { len, max }) => {
                warn!(
                    channel,
                    phase = ?phase,
                    len,
                    max,
                    "extension custom payload denied by size policy"
                );
            }
            Err(error) => {
                debug!(
                    channel,
                    phase = ?phase,
                    payload_len = payload.len(),
                    ?error,
                    "extension custom payload denied by policy"
                );
            }
        }
    }

    pub(crate) fn custom_payload_policy(&self) -> &CustomPayloadPolicy {
        &self.custom_payload_policy
    }

    #[cfg(test)]
    pub(crate) fn try_recv_command(&self) -> Result<ExtensionOutboundCommand, QueueRecvError> {
        self.boundary.try_recv_command()
    }

    pub(crate) async fn recv_command(&self) -> Result<ExtensionOutboundCommand, QueueRecvError> {
        self.boundary.recv_command().await
    }
}

#[derive(Clone)]
pub(crate) struct ScriptEventSink {
    boundary: ScriptBoundary,
}

impl ScriptEventSink {
    pub(crate) fn new(boundary: ScriptBoundary) -> Self {
        Self { boundary }
    }

    pub(crate) fn try_enqueue_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        self.boundary.try_enqueue_event(event)
    }

    pub(crate) fn enqueue_event(&self, event: ScriptEvent) {
        let event_name = event.event_name();
        match self.try_enqueue_event(event) {
            Ok(()) => {}
            Err(ScriptQueueError::Full) if event_name == "server.tick" => {
                debug!(event = event_name, "script event queue full; tick dropped");
            }
            Err(ScriptQueueError::Full) => {
                warn!(event = event_name, "script event queue full; event dropped");
            }
            Err(ScriptQueueError::Closed) => {
                warn!(
                    event = event_name,
                    "script event queue closed; event dropped"
                );
            }
            Err(_) => {
                warn!(event = event_name, "script event queue rejected event");
            }
        }
    }

    pub(crate) fn enqueue_server_tick(&self, tick: u64) {
        if let Err(ScriptQueueError::Closed) = self.boundary.try_enqueue_latest_server_tick(tick) {
            warn!("script event queue closed; server tick unavailable");
        }
    }

    pub(crate) async fn enqueue_targeted_event(
        &self,
        event: ScriptEvent,
    ) -> Result<(), ScriptQueueError> {
        self.boundary.enqueue_targeted_event(event).await
    }

    pub(crate) async fn enqueue_required_event(
        &self,
        event: ScriptEvent,
    ) -> Result<(), ScriptQueueError> {
        self.boundary.enqueue_required_event(event).await
    }

    pub(crate) fn close_event_admission(&self) {
        self.boundary.close_event_admission();
    }

    pub(crate) fn accept_host_command(
        &self,
        command: ScriptCommand,
    ) -> Result<AdmittedScriptCommand, mc_script::ScriptCommandAcceptanceError> {
        self.boundary.accept_host_command(command)
    }

    pub(crate) fn player_command_roots(&self) -> Vec<String> {
        self.boundary.player_command_roots()
    }

    pub(crate) fn operator_command_roots(&self) -> Vec<String> {
        self.boundary.operator_command_roots()
    }

    #[cfg(test)]
    pub(crate) fn enqueue_player_command_with_operator(
        &self,
        player_id: u64,
        username: &str,
        raw: &str,
        is_operator: bool,
    ) -> mc_script::PlayerCommandAdmission {
        match self.boundary.try_enqueue_player_command_with_context(
            mc_script::ScriptPlayerId::new(player_id),
            mc_script::ScriptPlayerContext::new(
                format!("player-{player_id}"),
                username,
                is_operator,
                0.0,
                0.0,
                0.0,
            ),
            raw,
        ) {
            Ok(admission) => admission,
            Err(ScriptQueueError::Full) => {
                warn!("script event queue full; player command dropped");
                mc_script::PlayerCommandAdmission::Dropped
            }
            Err(ScriptQueueError::Closed) => {
                warn!("script event queue closed; player command unavailable");
                mc_script::PlayerCommandAdmission::NotOwned
            }
            Err(_) => {
                warn!("script event queue rejected player command");
                mc_script::PlayerCommandAdmission::NotOwned
            }
        }
    }

    pub(crate) fn enqueue_player_command_with_context(
        &self,
        player_id: u64,
        context: ScriptPlayerContext,
        raw: &str,
    ) -> mc_script::PlayerCommandAdmission {
        match self.boundary.try_enqueue_player_command_with_context(
            mc_script::ScriptPlayerId::new(player_id),
            context,
            raw,
        ) {
            Ok(admission) => admission,
            Err(ScriptQueueError::Full) => {
                warn!("script event queue full; player command dropped");
                mc_script::PlayerCommandAdmission::Dropped
            }
            Err(ScriptQueueError::Closed) => {
                warn!("script event queue closed; player command unavailable");
                mc_script::PlayerCommandAdmission::NotOwned
            }
            Err(_) => {
                warn!("script event queue rejected player command");
                mc_script::PlayerCommandAdmission::NotOwned
            }
        }
    }

    async fn recv_command(&self) -> Option<ScriptCommand> {
        self.boundary.recv_command().await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptCommitForwardError {
    Queue(ScriptQueueError),
    RequiredTimeout { timeout: Duration },
}

async fn forward_committed_script_events(
    mut events: play::ScriptCommitEventReceiver,
    scripts: ScriptEventSink,
) -> Result<(), ScriptCommitForwardError> {
    while let Some(envelope) = events.recv().await {
        match envelope.delivery {
            play::ScriptCommitDelivery::Required => {
                match tokio::time::timeout(
                    SCRIPT_COMMIT_FORWARD_TIMEOUT,
                    scripts.enqueue_required_event(envelope.event),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        events.report_required_failure();
                        return Err(ScriptCommitForwardError::Queue(error));
                    }
                    Err(_) => {
                        events.report_required_failure();
                        return Err(ScriptCommitForwardError::RequiredTimeout {
                            timeout: SCRIPT_COMMIT_FORWARD_TIMEOUT,
                        });
                    }
                }
            }
            play::ScriptCommitDelivery::BestEffort => {
                if scripts.try_enqueue_event(envelope.event).is_err() {
                    events.record_best_effort_sink_drop();
                }
            }
        }
    }
    Ok(())
}

async fn watch_script_commit_event_failure(
    mut failure: tokio::sync::watch::Receiver<bool>,
    shutdown: ShutdownHandle,
) {
    loop {
        if *failure.borrow_and_update() {
            warn!("required committed script event delivery failed; requesting shutdown");
            shutdown.request();
            return;
        }
        tokio::select! {
            changed = failure.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = shutdown.notified() => return,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboundPressureSnapshot {
    pub best_effort_animation_drops: u64,
    pub reliable_command_drops: u64,
    pub reliable_command_retries: u64,
    pub reliable_command_retries_in_flight: u64,
    pub max_reliable_command_retries_in_flight: u64,
    pub slow_client_write_timeouts: u64,
    pub slow_client_pressure_sheds: u64,
}

#[derive(Clone)]
pub struct OutboundPressureHandle {
    sessions: Arc<play::SessionRegistry>,
}

impl OutboundPressureHandle {
    #[must_use]
    pub fn snapshot(&self) -> OutboundPressureSnapshot {
        let pressure = self.sessions.pressure_snapshot();
        OutboundPressureSnapshot {
            best_effort_animation_drops: pressure.best_effort_animation_drops,
            reliable_command_drops: pressure.reliable_command_drops,
            reliable_command_retries: pressure.reliable_command_retries,
            reliable_command_retries_in_flight: pressure.reliable_command_retries_in_flight,
            max_reliable_command_retries_in_flight: pressure.max_reliable_command_retries_in_flight,
            slow_client_write_timeouts: pressure.slow_client_write_timeouts,
            slow_client_pressure_sheds: pressure.slow_client_pressure_sheds,
        }
    }

    pub async fn wait_for_change(
        &self,
        before: OutboundPressureSnapshot,
    ) -> OutboundPressureSnapshot {
        loop {
            let observed = self.sessions.pressure_change_generation();
            let after = self.snapshot();
            if after != before {
                return after;
            }
            self.sessions.wait_for_pressure_change(observed).await;
        }
    }
}

#[derive(Debug, Clone)]
pub struct SaveAllReport {
    pub players_saved: usize,
    pub entities_saved: usize,
    pub chunks_flushed: usize,
    pub world_metadata_saved: bool,
    pub timings: SaveAllTimings,
    pub errors: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SaveAllTimings {
    pub queued_us: u64,
    pub players_us: u64,
    pub entities_us: u64,
    pub metadata_us: u64,
    pub flush_plan_us: u64,
    pub flush_write_us: u64,
    pub flush_commit_us: u64,
    pub total_us: u64,
}

impl SaveAllReport {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone)]
pub struct SaveHandle {
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    simulation: play::SimulationHandle,
}

impl SaveHandle {
    pub async fn save_all(&self) -> SaveAllReport {
        save_all_after_simulation_barrier(
            "save handle",
            &self.config,
            &self.sessions,
            &self.simulation,
        )
        .await
    }

    /// Save after `BoundServer::serve` has completed its simulation-owner drain.
    pub async fn save_all_after_drain(&self) -> SaveAllReport {
        save_all_after_drain_with_context("save after drain", &self.config, &self.sessions).await
    }
}

fn handle_accept_failure(
    error: std::io::Error,
    shutdown: &ShutdownHandle,
    runtime_control: Option<&RuntimeControlHandle>,
    chunk_pipeline_resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
) -> std::io::Error {
    warn!(%error, "listener accept failed; draining runtime before returning");
    if let Some(runtime_control) = runtime_control {
        request_runtime_control_drain(
            runtime_control,
            chunk_pipeline_resources,
            sessions,
            shutdown,
        );
    }
    shutdown.request();
    error
}

impl BoundServer {
    /// The socket address the listener is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    #[must_use]
    pub fn save_handle(&self) -> SaveHandle {
        SaveHandle {
            config: Arc::clone(&self.config),
            sessions: Arc::clone(&self.sessions),
            simulation: self.simulation.clone(),
        }
    }

    #[must_use]
    pub fn entity_effect_handle(&self) -> crate::EntityEffectHandle {
        self.simulation.entity_effect_handle()
    }

    #[must_use]
    pub fn entity_behavior_handle(&self) -> EntityBehaviorHandle {
        EntityBehaviorHandle {
            sessions: Arc::clone(&self.sessions),
        }
    }

    #[cfg(feature = "load-bench")]
    #[must_use]
    pub fn load_bench_handle(&self) -> LoadBenchHandle {
        LoadBenchHandle {
            sessions: Arc::clone(&self.sessions),
            simulation: self.simulation.clone(),
        }
    }

    #[must_use]
    pub fn chunk_pipeline_metrics(&self) -> crate::ChunkPipelineResourceMetrics {
        self.chunk_pipeline_resources.metrics()
    }

    #[must_use]
    pub fn chunk_pipeline_idle_handle(&self) -> crate::ChunkPipelineIdleHandle {
        crate::ChunkPipelineIdleHandle::new(self.chunk_pipeline_resources.clone())
    }

    #[must_use]
    pub fn outbound_pressure_handle(&self) -> OutboundPressureHandle {
        OutboundPressureHandle {
            sessions: Arc::clone(&self.sessions),
        }
    }

    #[must_use]
    pub fn runtime_control_handle(&self) -> Option<RuntimeControlHandle> {
        self.runtime_control.clone()
    }

    #[must_use]
    pub fn runtime_telemetry_handle(&self) -> RuntimeTelemetryHandle {
        RuntimeTelemetryHandle {
            tick_metrics: self.runtime_tick_metrics.clone(),
            sessions: Arc::clone(&self.sessions),
            runtime_control: self.runtime_control.clone(),
            simulation: self.simulation.clone(),
        }
    }

    /// Accept connections forever, spawning a per-connection task each
    /// time. Shutdown drains runtime owners and returns without performing
    /// the final save; the caller must save through [`SaveHandle`] only after
    /// this future succeeds. Ordinary per-connection protocol errors are logged
    /// inside their task, while a task panic or an authoritative owner failure
    /// stops admission and enters the coordinated drain path.
    pub async fn serve(self) -> std::io::Result<()> {
        let prewarmed_entity_pathing_states = play::prewarm_entity_pathing_tables();
        info!(
            addr = %self.local_addr()?,
            registries = self.config.data.registry_count(),
            entries = self.config.data.entry_count(),
            pathing_states = prewarmed_entity_pathing_states.get(),
            "Solaris is listening"
        );
        let config = self.config;
        let online_authentication = self.online_authentication;
        let chunk_geometry = self.chunk_geometry;
        let connection_world = self.connection_world;
        let chunk_pipeline_resources = self.chunk_pipeline_resources;
        let runtime_control = self.runtime_control;
        let mut runtime_control_signals = runtime_control
            .as_ref()
            .and_then(RuntimeControlHandle::take_signal_receiver);
        let runtime_tick_metrics = self.runtime_tick_metrics;
        let sessions = self.sessions;
        let mut entity_owner_failure = sessions.subscribe_entity_owner_failure();
        let simulation = self.simulation;
        let mut simulation_owner = self.simulation_owner;
        let extension = self.extension;
        let scripts = self.scripts;
        let script_storage = self.script_storage;
        let script_zones = self.script_zones;
        let shutdown = config.shutdown.clone();
        let connection_task_limit = connection_task_limit(config.max_players);
        let connection_permits = Arc::new(Semaphore::new(connection_task_limit));
        let pre_auth_connection_limit = pre_auth_connection_limit(config.max_players);
        let pre_auth_admission =
            PreAuthAdmission::new(pre_auth_connection_limit, MAX_PRE_AUTH_CONNECTIONS_PER_IP);
        info!(
            connection_task_limit,
            pre_auth_connection_limit,
            pre_auth_per_ip_limit = MAX_PRE_AUTH_CONNECTIONS_PER_IP,
            "bounded concurrent connection tasks"
        );
        if let Some(scripts) = scripts.as_ref() {
            scripts.enqueue_event(ScriptEvent::server_started());
        }
        let (mut script_commit_event_worker, mut script_commit_event_failure_watcher) =
            if let Some(scripts) = scripts.clone() {
                let events = sessions.install_script_commit_event_outbox();
                let failure = sessions.subscribe_script_commit_event_failure();
                let failure_shutdown = shutdown.clone();
                (
                    Some(tokio::spawn(forward_committed_script_events(
                        events, scripts,
                    ))),
                    Some(tokio::spawn(watch_script_commit_event_failure(
                        failure,
                        failure_shutdown,
                    ))),
                )
            } else {
                (None, None)
            };
        let mut connections = tokio::task::JoinSet::new();
        let (entity_world_root, entity_scheduled_ticks) = if let Some(world) = config.world.as_ref()
        {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "entity world root",
                Instant::now(),
                world.lock().await,
            );
            (
                storage.world_root().map(std::path::Path::to_path_buf),
                Some(storage.scheduled_tick_view()),
            )
        } else {
            (None, None)
        };
        let entity_world_read = connection_world.read.clone();
        let entity_world_mutation = connection_world.mutation.clone();
        let entity_pathing_materials = entity_world_read
            .as_ref()
            .map(|_| cached_material_ids(&config));
        let entity_sessions = Arc::clone(&sessions);
        let mut entity_world_journal_failure =
            entity_sessions.subscribe_world_chunk_journal_failure();
        let entity_config = Arc::clone(&config);
        let entity_runtime_control = runtime_control.clone();
        let mut entity_runtime_control_signals = runtime_control_signals.take();
        let entity_tick_metrics = runtime_tick_metrics.clone();
        let entity_chunk_pipeline_resources = chunk_pipeline_resources.clone();
        let entity_scripts = scripts.clone();
        let entity_script_zones = script_zones.clone();
        let (periodic_save_requests, periodic_save_worker) = if entity_world_root.is_some() {
            let periodic_config = Arc::clone(&entity_config);
            let periodic_sessions = Arc::clone(&entity_sessions);
            let periodic_simulation = simulation.clone();
            let periodic_shutdown = shutdown.clone();
            let dirty_config = Arc::clone(&entity_config);
            let dirty_sessions = Arc::clone(&entity_sessions);
            let worker = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
                move || {
                    let config = Arc::clone(&dirty_config);
                    let sessions = Arc::clone(&dirty_sessions);
                    async move {
                        log_dirty_only_flush(
                            "dirty high-water flush",
                            flush_dirty_chunks_only(&config, sessions.simulation_tick()).await,
                        )
                    }
                },
                move || {
                    let config = Arc::clone(&periodic_config);
                    let sessions = Arc::clone(&periodic_sessions);
                    let simulation = periodic_simulation.clone();
                    let shutdown = periodic_shutdown.clone();
                    async move {
                        let Some(report) =
                            save_periodic_checkpoint(&config, &sessions, &simulation, &shutdown)
                                .await
                        else {
                            return;
                        };
                        log_save_report("periodic checkpoint", &report);
                    }
                },
            );
            (Some(worker.notifier()), Some(worker))
        } else {
            (None, None)
        };
        if let (Some(world), Some(dirty_flush)) = (
            entity_config.world.as_ref(),
            periodic_save_requests.as_ref(),
        ) {
            let dirty_flush = dirty_flush.clone();
            let dirty_tail_progress = entity_config.shutdown.clone();
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "dirty flush notification install",
                Instant::now(),
                world.lock().await,
            );
            storage.set_dirty_high_water_notifier(Arc::new(move || {
                dirty_tail_progress.mark_dirty_tail_progress();
                dirty_flush.request_dirty_flush();
            }));
        }
        if let Some(requests) = periodic_save_requests.as_ref() {
            enqueue_startup_dirty_flush(&config, requests).await;
        }
        let connection_services = ConnectionServices {
            config: Arc::clone(&config),
            online_authentication,
            chunk_geometry,
            connection_world: connection_world.clone(),
            sessions: Arc::clone(&sessions),
            chunk_pipeline_resources: chunk_pipeline_resources.clone(),
            dirty_flush: periodic_save_requests.clone(),
            runtime_control: runtime_control.clone(),
            simulation: simulation.clone(),
            extension: extension.clone(),
            scripts: scripts.clone(),
            script_zones: script_zones.clone(),
        };
        let (entity_shutdown, mut entity_shutdown_requested) = tokio::sync::oneshot::channel();
        let mut entity_ticker = tokio::spawn(async move {
            let _pathing_tables_ready = prewarmed_entity_pathing_states;
            let mut ticker = tokio::time::interval(play::ENTITY_TICK_PERIOD);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let metrics_policy = RuntimeMetricsPolicy::default().normalized();
            let mut metrics_log_gate = RuntimeMetricsLogGate::default();
            let simulation_policy = entity_config.random_tick.normalized();
            let mut natural_spawn_ticker =
                natural_spawn_ticker::NaturalSpawnTicker::new(simulation_policy);
            let mut tick_metrics = RuntimeTickMetricsWindow::default();
            let (tick_metrics_publisher, mut tick_metrics_observations, tick_metrics_worker) =
                spawn_runtime_tick_metrics_worker(entity_tick_metrics.clone());
            let (memory_pressure_sampler, memory_pressure_worker) =
                if let Some(control) = entity_runtime_control.as_ref() {
                    let (sampler, worker) = control.spawn_memory_pressure_sampler();
                    (Some(sampler), Some(worker))
                } else {
                    (None, None)
                };
            let mut tick = 0_u64;
            let villager_population_ids = {
                let item_id = |name: &str| {
                    Identifier::parse(name)
                        .ok()
                        .and_then(|id| entity_config.items.id_of(&id))
                };
                let entity_type_id = |name: &str| {
                    Identifier::parse(name)
                        .ok()
                        .and_then(|id| entity_config.entity_types.id_of(&id))
                        .and_then(|id| i32::try_from(id).ok())
                };
                match (
                    item_id("minecraft:bread"),
                    item_id("minecraft:potato"),
                    item_id("minecraft:carrot"),
                    item_id("minecraft:beetroot"),
                    entity_type_id("minecraft:villager"),
                    entity_type_id("minecraft:item"),
                ) {
                    (
                        Some(bread),
                        Some(potato),
                        Some(carrot),
                        Some(beetroot),
                        Some(villager),
                        Some(item),
                    ) => Some((
                        mc_entity::villager_population_26_1_2::VillagerFoodItemIds {
                            bread,
                            potato,
                            carrot,
                            beetroot,
                        },
                        villager,
                        item,
                    )),
                    _ => None,
                }
            };
            let village_defense_golem_type_id = Identifier::parse("minecraft:iron_golem")
                .ok()
                .and_then(|id| entity_config.entity_types.id_of(&id))
                .and_then(|id| i32::try_from(id).ok());
            let mut session_empty_generation = entity_sessions.session_empty_generation();
            let mut player_save_generation = entity_sessions.player_save_generation();
            let mut simulation_command_window = SimulationCommandTelemetryWindow::default();
            let mut simulation_command_gate = SimulationCommandGate::default();
            let mut pushed_simulation_lane_attribution = Vec::new();
            let mut entity_physics_job = None;
            let mut entity_update_budget =
                crate::runtime_entity_budget::EntityUpdateBudgetController::default();
            let mut movement_publication_budget =
                crate::runtime_entity_budget::MovementPublicationBudgetController::default();
            let mut entity_budget_last_reliable_drops = 0_u64;
            let mut scheduled_budget_exhausted_since_publish = false;
            let mut inhabited_time = play::InhabitedTimeAccumulator::default();
            loop {
                let command_arrived = tokio::select! {
                    biased;
                    result = entity_world_journal_failure.changed() => {
                        result.expect("session registry owns the world journal failure sender");
                        if *entity_world_journal_failure.borrow_and_update() {
                            warn!("world chunk journal failed; requesting controlled shutdown");
                            entity_config.shutdown.request();
                        }
                        continue;
                    }
                    _ = &mut entity_shutdown_requested => {
                        if let Some(job) = entity_physics_job.take() {
                            apply_entity_physics_job_result(
                                job.await,
                                &simulation_owner,
                                &entity_config,
                                &entity_sessions,
                                &entity_chunk_pipeline_resources,
                                entity_world_read.as_ref(),
                            )
                            .await;
                        }
                        persist_inhabited_time_tail(
                            &entity_config,
                            entity_world_mutation.as_ref(),
                            &mut inhabited_time,
                        )
                        .await;
                        simulation_owner.shutdown();
                        tick_metrics_publisher.try_publish(
                            tick,
                            &tick_metrics,
                            scheduled_budget_exhausted_since_publish,
                        );
                        info!("simulation drain fenced; entity ticker stopping");
                        break;
                    }
                    generation = wait_for_session_empty_save_request(
                        &entity_sessions,
                        session_empty_generation,
                        periodic_save_requests.as_ref(),
                        tick,
                    ) => {
                        session_empty_generation = generation;
                        continue;
                    }
                    generation = wait_for_player_save_request(
                        &entity_sessions,
                        player_save_generation,
                        periodic_save_requests.as_ref(),
                        tick,
                    ) => {
                        player_save_generation = generation;
                        continue;
                    }
                    result = wait_for_entity_physics_job(&mut entity_physics_job) => {
                        let _completed_job = entity_physics_job.take();
                        apply_entity_physics_job_result(
                            result,
                            &simulation_owner,
                            &entity_config,
                            &entity_sessions,
                            &entity_chunk_pipeline_resources,
                            entity_world_read.as_ref(),
                        )
                        .await;
                        simulation_owner
                            .tick_primed_tnt(
                                &entity_sessions,
                                entity_config.world.as_ref(),
                                entity_config.block_light.as_deref(),
                                &entity_config.block_facts,
                                &entity_config.blocks,
                                entity_pathing_materials.as_deref(),
                                || {
                                    entity_script_zones.as_ref().map(|zones| {
                                        zones.protection_snapshot().unwrap_or_else(|error| {
                                            warn!(
                                                ?error,
                                                "zone protection snapshot unavailable; denying explosion block damage"
                                            );
                                            crate::script::ZoneProtectionSnapshot::unavailable()
                                        })
                                    })
                                },
                            )
                            .await;
                        continue;
                    }
                    observation = tick_metrics_observations.recv(), if !tick_metrics_observations.is_closed() => {
                        let Some(observation) = observation else {
                            continue;
                        };
                        if let Some(control) = entity_runtime_control.as_ref() {
                            let outcome = apply_runtime_control_operation(
                                control,
                                &entity_chunk_pipeline_resources,
                                &entity_sessions,
                                &entity_config.shutdown,
                                RuntimeControlOperation::ObserveWork(runtime_work_input(
                                    &observation.percentiles,
                                    observation.scheduled_budget_exhausted,
                                )),
                            );
                            if let Some(RuntimeControlOutcome::Work(decision)) = outcome.as_ref()
                                && decision.action == crate::AutoscaleAction::ScaleDown
                            {
                                info!(
                                    tick,
                                    source_tick = observation.percentiles.source_tick,
                                    action = ?decision.action,
                                    focus = ?decision.focus,
                                    entity_pathing_candidates =
                                        decision.budgets.entity_pathing_candidates,
                                    random_tick_chunk_budget =
                                        decision.budgets.random_tick_chunks,
                                    scheduled_tick_budget = decision.budgets.scheduled_ticks,
                                    reason = %decision.reason,
                                    "runtime work budgets changed"
                                );
                            } else if let Some(RuntimeControlOutcome::Work(decision)) =
                                outcome.as_ref()
                                && decision.action == crate::AutoscaleAction::ScaleUp
                            {
                                debug!(
                                    tick,
                                    source_tick = observation.percentiles.source_tick,
                                    entity_pathing_candidates =
                                        decision.budgets.entity_pathing_candidates,
                                    random_tick_chunk_budget =
                                        decision.budgets.random_tick_chunks,
                                    scheduled_tick_budget = decision.budgets.scheduled_ticks,
                                    reason = %decision.reason,
                                    "runtime work budgets recovering"
                                );
                            }
                        }
                        continue;
                    }
                    signal = recv_runtime_control_signal(&mut entity_runtime_control_signals) => {
                        let Some(signal) = signal else {
                            entity_runtime_control_signals = None;
                            continue;
                        };
                        if let Some(control) = entity_runtime_control.as_ref() {
                            observe_runtime_control_signal(
                                control,
                                &entity_chunk_pipeline_resources,
                                &entity_sessions,
                                &entity_config.shutdown,
                                signal,
                            );
                        }
                        continue;
                    }
                    // An overdue tick is immediately ready. Commands must win the
                    // biased select so overloaded ticks cannot starve player actions.
                    ready = simulation_owner.wait_for_command(), if simulation_command_gate.accepts_off_tick_batch() => {
                        if !ready {
                            if let Some(job) = entity_physics_job.take() {
                                apply_entity_physics_job_result(
                                    job.await,
                                    &simulation_owner,
                                    &entity_config,
                                    &entity_sessions,
                                    &entity_chunk_pipeline_resources,
                                    entity_world_read.as_ref(),
                                )
                                .await;
                            }
                            persist_inhabited_time_tail(
                                &entity_config,
                                entity_world_mutation.as_ref(),
                                &mut inhabited_time,
                            )
                            .await;
                            simulation_owner.shutdown();
                            tick_metrics_publisher.try_publish(
                                tick,
                                &tick_metrics,
                                scheduled_budget_exhausted_since_publish,
                            );
                            info!("simulation command channel closed; entity ticker stopping");
                            break;
                        }
                        true
                    }
                    _ = ticker.tick() => false,
                };
                if command_arrived {
                    let started = Instant::now();
                    let report = simulation_owner
                        .process_ready_commands_with_world_views(
                            &entity_sessions,
                            entity_config.world.as_ref(),
                            play::SimulationWorldAccess {
                                read: entity_world_read.as_ref(),
                                mutation: entity_world_mutation.as_ref(),
                                cpu: Some(&entity_chunk_pipeline_resources),
                                light: entity_config.block_light.as_ref(),
                            },
                            entity_config.block_light.as_deref(),
                            play::SIMULATION_COMMAND_BATCH_LIMIT,
                        )
                        .await;
                    simulation_command_window
                        .record_off_tick(elapsed_us(started), report.processed);
                    simulation_command_gate.record_off_tick_batch();
                    pushed_simulation_lane_attribution.extend(report.lane_attribution);
                    continue;
                }
                simulation_command_gate.record_tick_boundary();
                let tick_started = Instant::now();
                tick = entity_sessions.simulation_tick().saturating_add(1);
                if let Some(scripts) = entity_scripts.as_ref() {
                    scripts.enqueue_server_tick(tick);
                }
                let work_budgets = entity_runtime_control
                    .as_ref()
                    .map(|control| control.snapshot().work_budgets)
                    .unwrap_or(RuntimeWorkBudgets {
                        random_tick_chunks: simulation_policy.chunk_budget,
                        scheduled_ticks: simulation_policy.fluid_tick_budget,
                        ..RuntimeWorkBudgets::default()
                    });

                let started = Instant::now();
                let mut simulation_commands = simulation_owner
                    .process_commands_with_world_views(
                        &entity_sessions,
                        entity_config.world.as_ref(),
                        play::SimulationWorldAccess {
                            read: entity_world_read.as_ref(),
                            mutation: entity_world_mutation.as_ref(),
                            cpu: Some(&entity_chunk_pipeline_resources),
                            light: entity_config.block_light.as_ref(),
                        },
                        entity_config.block_light.as_deref(),
                        play::SIMULATION_COMMAND_BATCH_LIMIT,
                    )
                    .await;
                let simulation_command_telemetry = simulation_command_window
                    .finish_tick(elapsed_us(started), simulation_commands.processed);
                simulation_commands.processed = simulation_command_telemetry.processed;
                pushed_simulation_lane_attribution
                    .append(&mut simulation_commands.lane_attribution);
                simulation_commands.lane_attribution =
                    std::mem::take(&mut pushed_simulation_lane_attribution);
                let mut simulation_commands_us = simulation_command_telemetry.elapsed_us;
                let simulation_command_scope = simulation_command_telemetry.scope.as_str();
                let mut simulation_command_cpu_admission_wait_us = simulation_commands
                    .lane_attribution
                    .iter()
                    .map(|attribution| attribution.cpu_admission_wait_us)
                    .sum::<u64>();
                let mut simulation_command_post_admission_us = simulation_commands
                    .lane_attribution
                    .iter()
                    .flat_map(|lane| &lane.commands)
                    .map(|attribution| attribution.post_admission_command_us)
                    .sum::<u64>();
                let started = Instant::now();
                let world_time = simulation_owner.advance_world_time(&entity_sessions, 1);
                tick = entity_sessions.simulation_tick();
                entity_sessions.synchronize_entity_lifecycle_epoch(tick);
                simulation_owner
                    .tick_dying_entities(&entity_sessions, entity_sessions.simulation_tick());
                let world_time_us = elapsed_us(started);
                natural_spawn_ticker.tick(
                    &entity_sessions,
                    tick,
                    entity_world_read.as_ref(),
                    entity_pathing_materials.as_deref(),
                );
                let started = Instant::now();
                simulation_owner
                    .run_sheep_grazing(
                        &entity_config,
                        &entity_sessions,
                        entity_world_read.as_ref(),
                        entity_world_mutation.as_ref(),
                        tick,
                    )
                    .await;
                let sheep_grazing_us = elapsed_us(started);
                let mut animal_breeding_us = 0;
                let physics_was_in_flight = entity_physics_job.is_some();
                let started = Instant::now();
                let queries = if physics_was_in_flight {
                    Vec::new()
                } else {
                    simulation_owner.collect_entity_physics_queries(
                        &entity_sessions,
                        &entity_chunk_pipeline_resources,
                        tick,
                        play::EntitySimulationTickPolicy {
                            entity_updates_per_lane: entity_update_budget.configured_per_lane(),
                            pathing_candidates_per_entity: work_budgets.entity_pathing_candidates,
                            simulation_distance: simulation_policy.simulation_distance,
                        },
                        simulation_owner.entity_world_context(
                            entity_world_read.as_ref(),
                            entity_pathing_materials.as_deref(),
                            entity_config.blocks.as_ref(),
                            entity_config.items.as_ref(),
                        ),
                    )
                };
                let entity_goals_us = elapsed_us(started);
                let started = Instant::now();
                simulation_owner.tick_hostile_attacks(
                    &entity_sessions,
                    tick,
                    play::air_state_id(&entity_config.blocks),
                );
                let hostile_attacks_us = elapsed_us(started);
                if tick.is_multiple_of(u64::from(ANIMAL_BREEDING_TICK_INTERVAL_TICKS)) {
                    let started = Instant::now();
                    simulation_owner.tick_animal_breeding(
                        &entity_sessions,
                        ANIMAL_BREEDING_TICK_INTERVAL_TICKS,
                    );
                    animal_breeding_us = elapsed_us(started);
                }
                if let Some((food_items, villager_type_id, item_type_id)) = villager_population_ids
                {
                    simulation_owner.tick_villager_population(
                        &entity_sessions,
                        tick,
                        food_items,
                        villager_type_id,
                        item_type_id,
                        1,
                    );
                }
                if let Some(iron_golem_type_id) = village_defense_golem_type_id {
                    simulation_owner.tick_village_defense(
                        &entity_sessions,
                        tick,
                        iron_golem_type_id,
                        entity_world_read.as_ref(),
                        entity_pathing_materials.as_deref(),
                    );
                }
                let entity_query_count = queries.len();
                let (steps, entity_physics_us, entity_dispatch_us) = if physics_was_in_flight {
                    (Vec::new(), 0, 0)
                } else {
                    let started = Instant::now();
                    let inputs = prepare_entity_physics_inputs(
                        &entity_config,
                        entity_world_read.as_ref(),
                        &queries,
                    );
                    if inputs.len() > ENTITY_PHYSICS_INLINE_LIMIT {
                        entity_physics_job = Some(spawn_entity_physics_job(
                            tick,
                            queries,
                            entity_chunk_pipeline_resources.clone(),
                            inputs,
                        ));
                        (Vec::new(), elapsed_us(started), 0)
                    } else {
                        let physics_snapshot =
                            inputs.first().map(|input| Arc::clone(&input.snapshot));
                        let steps = step_entity_physics_inputs(
                            entity_chunk_pipeline_resources.clone(),
                            inputs,
                        )
                        .await;
                        let entity_physics_us = elapsed_us(started);
                        let world_is_current = physics_snapshot.as_ref().is_none_or(|snapshot| {
                            entity_world_read.as_ref().is_some_and(|world_read| {
                                entity_physics_snapshot_is_current(world_read, snapshot)
                            })
                        });
                        if !world_is_current {
                            debug!(
                                tick,
                                "discarded inline entity physics after world snapshot changed"
                            );
                            (Vec::new(), entity_physics_us, 0)
                        } else {
                            let arrow_physics_facts =
                                physics_snapshot.map_or_else(Vec::new, |snapshot| {
                                    arrow_physics_facts_from_steps(
                                        tick, &queries, &snapshot, &steps,
                                    )
                                });
                            let started = Instant::now();
                            let accepted_steps = simulation_owner.apply_entity_physics_if_current(
                                &entity_sessions,
                                &entity_chunk_pipeline_resources,
                                tick,
                                &queries,
                                &steps,
                                &arrow_physics_facts,
                            );
                            let entity_dispatch_us = elapsed_us(started);
                            let landed_falling_blocks =
                                entity_sessions.landed_falling_blocks(&accepted_steps);
                            if !landed_falling_blocks.is_empty() {
                                simulation_owner
                                    .land_falling_blocks(
                                        &entity_config,
                                        &entity_sessions,
                                        entity_world_read.as_ref(),
                                        &landed_falling_blocks,
                                    )
                                    .await;
                            }
                            (steps, entity_physics_us, entity_dispatch_us)
                        }
                    }
                };
                let entity_step_count = steps.len();
                if entity_physics_job.is_none() {
                    simulation_owner
                        .tick_primed_tnt(
                            &entity_sessions,
                            entity_config.world.as_ref(),
                            entity_config.block_light.as_deref(),
                            &entity_config.block_facts,
                            &entity_config.blocks,
                            entity_pathing_materials.as_deref(),
                            || {
                                entity_script_zones.as_ref().map(|zones| {
                                    zones.protection_snapshot().unwrap_or_else(|error| {
                                        warn!(
                                            ?error,
                                            "zone protection snapshot unavailable; denying explosion block damage"
                                        );
                                        crate::script::ZoneProtectionSnapshot::unavailable()
                                    })
                                })
                            },
                        )
                        .await;
                }

                let started = Instant::now();
                let campfire_tick = simulation_owner
                    .run_campfire_cooking_ticks(
                        &entity_config,
                        &entity_sessions,
                        entity_world_read.as_ref(),
                        entity_world_mutation.as_ref(),
                    )
                    .await;
                let campfire_tick_us = elapsed_us(started);

                let started = Instant::now();
                let furnace_updated = simulation_owner
                    .run_furnace_ticks(
                        &entity_config,
                        &entity_sessions,
                        entity_world_read.as_ref(),
                        entity_world_mutation.as_ref(),
                    )
                    .await;
                let furnace_tick_us = elapsed_us(started);

                let loaded_chunks = entity_sessions.loaded_chunks_sorted();
                let spawning_chunks = entity_sessions.spawning_chunks_sorted();
                let started = Instant::now();
                let inhabited_updates = inhabited_time.observe_tick(tick, &spawning_chunks);
                let missing = entity_world_mutation
                    .as_ref()
                    .map_or(inhabited_updates.clone(), |mutation| {
                        mutation.increment_chunk_inhabited_times(&inhabited_updates)
                    });
                inhabited_time.restore(missing);
                let inhabited_time_us = elapsed_us(started);
                // `entity_save_us` is the synchronous save work executed inside this
                // tick. It is intentionally zero: the request below is non-blocking,
                // its tiny enqueue cost remains visible in total/unattributed tick time,
                // and actual checkpoint I/O is reported by `SaveAllTimings` from the
                // dedicated save worker.
                let entity_save_us = 0;
                if tick.is_multiple_of(simulation_policy.save_interval_ticks)
                    && entity_sessions.active_session_count() > 0
                {
                    request_full_checkpoint(
                        periodic_save_requests.as_ref(),
                        tick,
                        "periodic interval",
                    );
                }

                let started = Instant::now();
                let ambient_protection = entity_script_zones.as_ref().map(|zones| {
                    zones.protection_snapshot().unwrap_or_else(|error| {
                        warn!(
                            ?error,
                            "zone protection snapshot unavailable; denying ambient block mutation"
                        );
                        crate::script::ZoneProtectionSnapshot::unavailable()
                    })
                });
                let random_tick = simulation_owner
                    .run_random_ticks_with_budget(
                        &entity_config,
                        &entity_sessions,
                        play::SimulationWorldAccess {
                            read: entity_world_read.as_ref(),
                            mutation: entity_world_mutation.as_ref(),
                            cpu: Some(&entity_chunk_pipeline_resources),
                            light: entity_config.block_light.as_ref(),
                        },
                        ambient_protection.as_ref(),
                        tick,
                        work_budgets.random_tick_chunks,
                    )
                    .await;
                let random_tick_us = elapsed_us(started);

                let (block_tick, block_tick_us) =
                    if entity_scheduled_ticks
                        .as_ref()
                        .is_some_and(|scheduled_ticks| {
                            loaded_block_tick_due(scheduled_ticks, &loaded_chunks, tick)
                        })
                    {
                        let started = Instant::now();
                        let job = spawn_scheduled_block_tick_job(
                            tick,
                            work_budgets.scheduled_ticks,
                            Arc::clone(&entity_config),
                            Arc::clone(&entity_sessions),
                            entity_world_read.clone(),
                            entity_world_mutation.clone(),
                            ambient_protection.map(Arc::new),
                            entity_chunk_pipeline_resources.clone(),
                        );
                        let (result, mid_tick_commands) =
                            await_scheduled_block_tick_job_with_commands(
                                job,
                                &mut simulation_owner,
                                &entity_config,
                                &entity_sessions,
                                entity_world_read.as_ref(),
                                entity_world_mutation.as_ref(),
                                &entity_chunk_pipeline_resources,
                            )
                            .await;
                        simulation_commands_us =
                            simulation_commands_us.saturating_add(mid_tick_commands.elapsed_us);
                        simulation_commands.processed = simulation_commands
                            .processed
                            .saturating_add(mid_tick_commands.report.processed);
                        simulation_commands.remaining_depth =
                            mid_tick_commands.report.remaining_depth;
                        simulation_command_cpu_admission_wait_us =
                            simulation_command_cpu_admission_wait_us.saturating_add(
                                mid_tick_commands
                                    .report
                                    .lane_attribution
                                    .iter()
                                    .map(|attribution| attribution.cpu_admission_wait_us)
                                    .sum::<u64>(),
                            );
                        simulation_command_post_admission_us = simulation_command_post_admission_us
                            .saturating_add(
                                mid_tick_commands
                                    .report
                                    .lane_attribution
                                    .iter()
                                    .flat_map(|lane| &lane.commands)
                                    .map(|attribution| attribution.post_admission_command_us)
                                    .sum::<u64>(),
                            );
                        simulation_commands
                            .lane_attribution
                            .extend(mid_tick_commands.report.lane_attribution);
                        let block_tick_us =
                            elapsed_us(started).saturating_sub(mid_tick_commands.elapsed_us);
                        let report = match result {
                            Ok(completed) => {
                                debug!(
                                    tick = completed.tick,
                                    drained = completed.report.drained,
                                    applied = completed.report.applied,
                                    elapsed_us = completed.elapsed_us,
                                    "scheduled block tick job completed"
                                );
                                completed.report
                            }
                            Err(error) if error.is_cancelled() => {
                                debug!("scheduled block tick job cancelled");
                                play::ScheduledBlockTickReport {
                                    budget: work_budgets.scheduled_ticks.max(1),
                                    ..play::ScheduledBlockTickReport::default()
                                }
                            }
                            Err(error) => {
                                warn!(%error, "scheduled block tick job failed");
                                play::ScheduledBlockTickReport {
                                    budget: work_budgets.scheduled_ticks.max(1),
                                    ..play::ScheduledBlockTickReport::default()
                                }
                            }
                        };
                        (report, block_tick_us)
                    } else {
                        (
                            play::ScheduledBlockTickReport {
                                budget: work_budgets.scheduled_ticks.max(1),
                                ..play::ScheduledBlockTickReport::default()
                            },
                            0,
                        )
                    };

                let started = Instant::now();
                let fluid_tick = if entity_scheduled_ticks
                    .as_ref()
                    .is_some_and(|scheduled_ticks| {
                        loaded_fluid_tick_due(scheduled_ticks, &loaded_chunks, tick)
                    }) {
                    simulation_owner
                        .run_scheduled_fluid_ticks_with_budget(
                            &entity_config,
                            &entity_sessions,
                            entity_world_read.as_ref(),
                            entity_world_mutation.as_ref(),
                            tick,
                            work_budgets.scheduled_ticks,
                        )
                        .await
                } else {
                    play::ScheduledFluidTickReport {
                        budget: work_budgets.scheduled_ticks.max(1),
                        ..play::ScheduledFluidTickReport::default()
                    }
                };
                let fluid_tick_us = elapsed_us(started);

                let tick_us = elapsed_us(tick_started)
                    .saturating_add(simulation_command_telemetry.off_tick_elapsed_us);
                let (_, _, selected_entity_updates, active_entity_population) =
                    entity_sessions.entity_update_budget_observation();
                let target_tick_us = entity_runtime_control
                    .as_ref()
                    .map(|control| {
                        control
                            .snapshot()
                            .policy
                            .target_tick_ms
                            .saturating_mul(1_000)
                    })
                    .unwrap_or(50_000);
                let outbound_pressure = entity_sessions.pressure_snapshot();
                let reliable_drops_increased =
                    outbound_pressure.reliable_command_drops > entity_budget_last_reliable_drops;
                entity_budget_last_reliable_drops = outbound_pressure.reliable_command_drops;
                let entity_pressure = crate::runtime_entity_budget::EntityUpdatePressure {
                    reliable_drops_increased,
                    reliable_retries_in_flight: outbound_pressure
                        .reliable_command_retries_in_flight,
                    simulation_queue_depth: simulation_commands.remaining_depth,
                };
                let entity_update_budget_snapshot = entity_update_budget.observe(
                    crate::runtime_entity_budget::EntityUpdateBudgetObservation {
                        tick_us,
                        entity_goals_us,
                        selected: selected_entity_updates,
                        active_population: active_entity_population,
                        lane_count: entity_chunk_pipeline_resources.cpu_limit().max(1),
                        target_tick_us,
                        pressure: entity_pressure,
                    },
                );
                let movement_budget =
                    movement_publication_budget.observe(tick_us, target_tick_us, entity_pressure);
                entity_sessions.set_entity_movement_publication_budget(movement_budget);
                let current_tick_sample = RuntimeTickSample {
                    tick_us,
                    world_time_us,
                    sheep_grazing_us,
                    animal_breeding_us,
                    hostile_attacks_us,
                    entity_goals_us,
                    entity_physics_us,
                    entity_dispatch_us,
                    campfire_tick_us,
                    inhabited_time_us,
                    entity_save_us,
                    random_tick_us,
                    block_tick_us,
                    fluid_tick_us,
                };
                let attributed_tick_us = runtime_attributed_tick_us(
                    &current_tick_sample,
                    simulation_commands_us,
                    furnace_tick_us,
                );
                let unattributed_tick_us = tick_us.saturating_sub(attributed_tick_us);
                tick_metrics.record(current_tick_sample);
                scheduled_budget_exhausted_since_publish |=
                    block_tick.budget_exhausted || fluid_tick.budget_exhausted;
                if let Some(control) = entity_runtime_control.as_ref() {
                    if let Some(sampler) = memory_pressure_sampler.as_ref() {
                        sampler.request();
                    }
                    observe_runtime_control_tick(
                        control,
                        &entity_chunk_pipeline_resources,
                        &entity_sessions,
                        &entity_config.shutdown,
                        tick_us,
                    );
                }
                if tick.is_multiple_of(metrics_policy.log_interval_ticks) {
                    if tick_metrics_publisher.try_publish(
                        tick,
                        &tick_metrics,
                        scheduled_budget_exhausted_since_publish,
                    ) {
                        scheduled_budget_exhausted_since_publish = false;
                    }
                    if let Some(percentiles) = entity_tick_metrics.snapshot()
                        && tracing::enabled!(tracing::Level::DEBUG)
                    {
                        debug!(
                            tick,
                            world_time,
                            tick_window_source_tick = percentiles.source_tick,
                            tick_window_submit_us = percentiles.observer_submit_us,
                            tick_window_compute_us = percentiles.observer_compute_us,
                            tick_window_skipped = percentiles.observer_skipped_windows,
                            tick_window_samples = percentiles.tick.samples,
                            tick_window_capacity = tick_metrics.capacity(),
                            tick_p50_us = percentiles.tick.p50_us,
                            tick_p95_us = percentiles.tick.p95_us,
                            tick_p99_us = percentiles.tick.p99_us,
                            tick_max_us = percentiles.tick.max_us,
                            world_time_p50_us = percentiles.world_time.p50_us,
                            world_time_p95_us = percentiles.world_time.p95_us,
                            world_time_p99_us = percentiles.world_time.p99_us,
                            world_time_max_us = percentiles.world_time.max_us,
                            sheep_grazing_p50_us = percentiles.sheep_grazing.p50_us,
                            sheep_grazing_p95_us = percentiles.sheep_grazing.p95_us,
                            sheep_grazing_p99_us = percentiles.sheep_grazing.p99_us,
                            sheep_grazing_max_us = percentiles.sheep_grazing.max_us,
                            animal_breeding_p50_us = percentiles.animal_breeding.p50_us,
                            animal_breeding_p95_us = percentiles.animal_breeding.p95_us,
                            animal_breeding_p99_us = percentiles.animal_breeding.p99_us,
                            animal_breeding_max_us = percentiles.animal_breeding.max_us,
                            hostile_attacks_p50_us = percentiles.hostile_attacks.p50_us,
                            hostile_attacks_p95_us = percentiles.hostile_attacks.p95_us,
                            hostile_attacks_p99_us = percentiles.hostile_attacks.p99_us,
                            hostile_attacks_max_us = percentiles.hostile_attacks.max_us,
                            entity_goals_p50_us = percentiles.entity_goals.p50_us,
                            entity_goals_p95_us = percentiles.entity_goals.p95_us,
                            entity_goals_p99_us = percentiles.entity_goals.p99_us,
                            entity_goals_max_us = percentiles.entity_goals.max_us,
                            entity_physics_p50_us = percentiles.entity_physics.p50_us,
                            entity_physics_p95_us = percentiles.entity_physics.p95_us,
                            entity_physics_p99_us = percentiles.entity_physics.p99_us,
                            entity_physics_max_us = percentiles.entity_physics.max_us,
                            entity_dispatch_p50_us = percentiles.entity_dispatch.p50_us,
                            entity_dispatch_p95_us = percentiles.entity_dispatch.p95_us,
                            entity_dispatch_p99_us = percentiles.entity_dispatch.p99_us,
                            entity_dispatch_max_us = percentiles.entity_dispatch.max_us,
                            campfire_tick_p50_us = percentiles.campfire_tick.p50_us,
                            campfire_tick_p95_us = percentiles.campfire_tick.p95_us,
                            campfire_tick_p99_us = percentiles.campfire_tick.p99_us,
                            campfire_tick_max_us = percentiles.campfire_tick.max_us,
                            inhabited_time_p50_us = percentiles.inhabited_time.p50_us,
                            inhabited_time_p95_us = percentiles.inhabited_time.p95_us,
                            inhabited_time_p99_us = percentiles.inhabited_time.p99_us,
                            inhabited_time_max_us = percentiles.inhabited_time.max_us,
                            entity_save_p50_us = percentiles.entity_save.p50_us,
                            entity_save_p95_us = percentiles.entity_save.p95_us,
                            entity_save_p99_us = percentiles.entity_save.p99_us,
                            entity_save_max_us = percentiles.entity_save.max_us,
                            random_tick_p50_us = percentiles.random_tick.p50_us,
                            random_tick_p95_us = percentiles.random_tick.p95_us,
                            random_tick_p99_us = percentiles.random_tick.p99_us,
                            random_tick_max_us = percentiles.random_tick.max_us,
                            block_tick_p50_us = percentiles.block_tick.p50_us,
                            block_tick_p95_us = percentiles.block_tick.p95_us,
                            block_tick_p99_us = percentiles.block_tick.p99_us,
                            block_tick_max_us = percentiles.block_tick.max_us,
                            fluid_tick_p50_us = percentiles.fluid_tick.p50_us,
                            fluid_tick_p95_us = percentiles.fluid_tick.p95_us,
                            fluid_tick_p99_us = percentiles.fluid_tick.p99_us,
                            fluid_tick_max_us = percentiles.fluid_tick.max_us,
                            "runtime tick percentile window"
                        );
                    }
                }
                if metrics_log_gate.should_log(tick, tick_us, metrics_policy) {
                    let pressure = entity_sessions.pressure_snapshot();
                    let lock_pressure = crate::lock_metrics::snapshot();
                    if is_slow_tick(tick_us, metrics_policy) {
                        warn!(
                            tick,
                            world_time,
                            tick_us,
                            world_time_us,
                            sheep_grazing_us,
                            animal_breeding_us,
                            hostile_attacks_us,
                            entity_goals_us,
                            entity_physics_us,
                            entity_dispatch_us,
                            campfire_tick_us,
                            furnace_tick_us,
                            furnace_updated,
                            unattributed_tick_us,
                            inhabited_time_us,
                            entity_save_us,
                            random_tick_us,
                            block_tick_us,
                            fluid_tick_us,
                            simulation_commands_us,
                            simulation_commands_processed = simulation_commands.processed,
                            simulation_commands_remaining = simulation_commands.remaining_depth,
                            simulation_command_scope,
                            simulation_command_cpu_admission_wait_us,
                            simulation_command_post_admission_us,
                            entity_queries = entity_query_count,
                            entity_steps = entity_step_count,
                            entity_update_budget_per_lane =
                                entity_update_budget_snapshot.configured_per_lane,
                            entity_update_budget_total =
                                entity_update_budget_snapshot.effective_total,
                            entity_update_selected = entity_update_budget_snapshot.selected,
                            entity_update_active_population =
                                entity_update_budget_snapshot.active_population,
                            entity_update_rotation_ticks =
                                entity_update_budget_snapshot.estimated_rotation_ticks,
                            entity_physics_in_flight = entity_physics_job.is_some(),
                            campfire_persisted = campfire_tick.persisted,
                            campfire_completed = campfire_tick.completed,
                            campfire_dropped = campfire_tick.dropped,
                            random_sampled = random_tick.sampled,
                            random_eligible = random_tick.eligible,
                            random_applied = random_tick.applied,
                            block_drained = block_tick.drained,
                            block_applied = block_tick.applied,
                            block_budget = block_tick.budget,
                            block_budget_exhausted = block_tick.budget_exhausted,
                            fluid_drained = fluid_tick.drained,
                            fluid_applied = fluid_tick.applied,
                            fluid_budget = fluid_tick.budget,
                            fluid_budget_exhausted = fluid_tick.budget_exhausted,
                            sessions = pressure.sessions,
                            ticketed_chunks = pressure.ticketed_chunks,
                            prepared_chunks = pressure.prepared_chunks,
                            server_entities = pressure.server_entities,
                            entity_spawn_dispatches = pressure.entity_dispatches.spawn,
                            entity_move_dispatches = pressure.entity_dispatches.move_relative,
                            entity_data_dispatches = pressure.entity_dispatches.data,
                            entity_take_dispatches = pressure.entity_dispatches.take,
                            entity_remove_dispatches = pressure.entity_dispatches.remove,
                            best_effort_animation_drops = pressure.best_effort_animation_drops,
                            reliable_command_drops = pressure.reliable_command_drops,
                            reliable_command_retries = pressure.reliable_command_retries,
                            reliable_command_retries_in_flight =
                                pressure.reliable_command_retries_in_flight,
                            furnace_viewer_sets = pressure.furnace_viewer_sets,
                            chest_viewer_sets = pressure.chest_viewer_sets,
                            world_lock_waits = lock_pressure.world_storage.wait_count,
                            world_lock_wait_us = lock_pressure.world_storage.wait_us,
                            world_lock_max_wait_us = lock_pressure.world_storage.max_wait_us,
                            world_lock_hold_us = lock_pressure.world_storage.hold_us,
                            world_lock_max_hold_us = lock_pressure.world_storage.max_hold_us,
                            session_lock_waits = lock_pressure.session_registry.wait_count,
                            session_lock_wait_us = lock_pressure.session_registry.wait_us,
                            session_lock_max_wait_us = lock_pressure.session_registry.max_wait_us,
                            session_lock_hold_us = lock_pressure.session_registry.hold_us,
                            session_lock_max_hold_us = lock_pressure.session_registry.max_hold_us,
                            container_lock_wait_us = lock_pressure.container_registry.wait_us,
                            container_lock_max_wait_us =
                                lock_pressure.container_registry.max_wait_us,
                            container_lock_hold_us = lock_pressure.container_registry.hold_us,
                            container_lock_max_hold_us =
                                lock_pressure.container_registry.max_hold_us,
                            save_flush_lock_wait_us = lock_pressure.save_all_flush.wait_us,
                            save_flush_lock_hold_us = lock_pressure.save_all_flush.hold_us,
                            chunk_prepare_lock_wait_us = lock_pressure.chunk_prepare.wait_us,
                            chunk_prepare_lock_hold_us = lock_pressure.chunk_prepare.hold_us,
                            player_persistence_lock_wait_us =
                                lock_pressure.player_persistence.wait_us,
                            player_persistence_lock_hold_us =
                                lock_pressure.player_persistence.hold_us,
                            "runtime tick exceeded performance budget"
                        );
                        let attributed_lane_waits = simulation_commands
                            .lane_attribution
                            .iter()
                            .take(SLOW_SIMULATION_ATTRIBUTION_LIMIT)
                            .map(|lane| lane.cpu_admission_wait_us)
                            .collect::<Vec<_>>();
                        let attributed_commands = simulation_commands
                            .lane_attribution
                            .iter()
                            .flat_map(|lane| &lane.commands)
                            .take(SLOW_SIMULATION_ATTRIBUTION_LIMIT)
                            .collect::<Vec<_>>();
                        let attributed_command_count = simulation_commands
                            .lane_attribution
                            .iter()
                            .map(|lane| lane.commands.len())
                            .sum::<usize>();
                        let omitted_lanes = simulation_commands
                            .lane_attribution
                            .len()
                            .saturating_sub(attributed_lane_waits.len());
                        let omitted_commands =
                            attributed_command_count.saturating_sub(attributed_commands.len());
                        if !attributed_lane_waits.is_empty() || !attributed_commands.is_empty() {
                            warn!(
                                tick,
                                simulation_command_scope,
                                cpu_admission_wait_us_by_lane = ?attributed_lane_waits,
                                simulation_commands = ?attributed_commands,
                                omitted_lanes,
                                omitted_commands,
                                "slow simulation command attribution"
                            );
                        }
                    } else {
                        debug!(
                            tick,
                            world_time,
                            tick_us,
                            world_time_us,
                            sheep_grazing_us,
                            animal_breeding_us,
                            hostile_attacks_us,
                            entity_goals_us,
                            entity_physics_us,
                            entity_dispatch_us,
                            campfire_tick_us,
                            furnace_tick_us,
                            furnace_updated,
                            unattributed_tick_us,
                            inhabited_time_us,
                            entity_save_us,
                            random_tick_us,
                            block_tick_us,
                            fluid_tick_us,
                            simulation_commands_us,
                            simulation_commands_processed = simulation_commands.processed,
                            simulation_commands_remaining = simulation_commands.remaining_depth,
                            simulation_command_scope,
                            simulation_command_cpu_admission_wait_us,
                            simulation_command_post_admission_us,
                            entity_queries = entity_query_count,
                            entity_steps = entity_step_count,
                            entity_update_budget_per_lane =
                                entity_update_budget_snapshot.configured_per_lane,
                            entity_update_budget_total =
                                entity_update_budget_snapshot.effective_total,
                            entity_update_selected = entity_update_budget_snapshot.selected,
                            entity_update_active_population =
                                entity_update_budget_snapshot.active_population,
                            entity_update_rotation_ticks =
                                entity_update_budget_snapshot.estimated_rotation_ticks,
                            entity_physics_in_flight = entity_physics_job.is_some(),
                            campfire_persisted = campfire_tick.persisted,
                            campfire_completed = campfire_tick.completed,
                            campfire_dropped = campfire_tick.dropped,
                            random_sampled = random_tick.sampled,
                            random_eligible = random_tick.eligible,
                            random_applied = random_tick.applied,
                            block_drained = block_tick.drained,
                            block_applied = block_tick.applied,
                            block_budget = block_tick.budget,
                            block_budget_exhausted = block_tick.budget_exhausted,
                            fluid_drained = fluid_tick.drained,
                            fluid_applied = fluid_tick.applied,
                            fluid_budget = fluid_tick.budget,
                            fluid_budget_exhausted = fluid_tick.budget_exhausted,
                            sessions = pressure.sessions,
                            ticketed_chunks = pressure.ticketed_chunks,
                            prepared_chunks = pressure.prepared_chunks,
                            server_entities = pressure.server_entities,
                            entity_spawn_dispatches = pressure.entity_dispatches.spawn,
                            entity_move_dispatches = pressure.entity_dispatches.move_relative,
                            entity_data_dispatches = pressure.entity_dispatches.data,
                            entity_take_dispatches = pressure.entity_dispatches.take,
                            entity_remove_dispatches = pressure.entity_dispatches.remove,
                            best_effort_animation_drops = pressure.best_effort_animation_drops,
                            reliable_command_drops = pressure.reliable_command_drops,
                            reliable_command_retries = pressure.reliable_command_retries,
                            reliable_command_retries_in_flight =
                                pressure.reliable_command_retries_in_flight,
                            furnace_viewer_sets = pressure.furnace_viewer_sets,
                            chest_viewer_sets = pressure.chest_viewer_sets,
                            world_lock_waits = lock_pressure.world_storage.wait_count,
                            world_lock_wait_us = lock_pressure.world_storage.wait_us,
                            world_lock_max_wait_us = lock_pressure.world_storage.max_wait_us,
                            world_lock_hold_us = lock_pressure.world_storage.hold_us,
                            world_lock_max_hold_us = lock_pressure.world_storage.max_hold_us,
                            session_lock_waits = lock_pressure.session_registry.wait_count,
                            session_lock_wait_us = lock_pressure.session_registry.wait_us,
                            session_lock_max_wait_us = lock_pressure.session_registry.max_wait_us,
                            session_lock_hold_us = lock_pressure.session_registry.hold_us,
                            session_lock_max_hold_us = lock_pressure.session_registry.max_hold_us,
                            container_lock_wait_us = lock_pressure.container_registry.wait_us,
                            container_lock_max_wait_us =
                                lock_pressure.container_registry.max_wait_us,
                            container_lock_hold_us = lock_pressure.container_registry.hold_us,
                            container_lock_max_hold_us =
                                lock_pressure.container_registry.max_hold_us,
                            save_flush_lock_wait_us = lock_pressure.save_all_flush.wait_us,
                            save_flush_lock_hold_us = lock_pressure.save_all_flush.hold_us,
                            chunk_prepare_lock_wait_us = lock_pressure.chunk_prepare.wait_us,
                            chunk_prepare_lock_hold_us = lock_pressure.chunk_prepare.hold_us,
                            player_persistence_lock_wait_us =
                                lock_pressure.player_persistence.wait_us,
                            player_persistence_lock_hold_us =
                                lock_pressure.player_persistence.hold_us,
                            "runtime tick metrics"
                        );
                    }
                }
            }
            drop(memory_pressure_sampler);
            if let Some(worker) = memory_pressure_worker
                && let Err(error) = worker.await
            {
                warn!(%error, "memory pressure sampler worker failed");
            }
            drop(tick_metrics_publisher);
            drop(tick_metrics_observations);
            if let Err(error) = tick_metrics_worker.await {
                warn!(%error, "runtime tick metrics worker failed");
            }
        });
        let runtime_control_signal_watcher = runtime_control.as_ref().map(|control| {
            let control = control.clone();
            let sessions = Arc::clone(&sessions);
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                forward_slow_client_sheds_to_runtime_control(sessions, control, shutdown).await;
            })
        });
        let mut command_tasks = tokio::task::JoinSet::new();
        if let Some(extension_commands) = extension.clone() {
            let extension_sessions = Arc::clone(&sessions);
            let extension_shutdown = shutdown.clone();
            command_tasks.spawn(async move {
                run_extension_commands(extension_commands, extension_sessions, extension_shutdown)
                    .await;
                "extension command"
            });
        }
        if let Some(script_commands) = scripts.clone() {
            let script_config = Arc::clone(&config);
            let script_sessions = Arc::clone(&sessions);
            let script_runtime_control = runtime_control.clone();
            let script_simulation = simulation.clone();
            let script_chunk_pipeline_resources = chunk_pipeline_resources.clone();
            let script_shutdown = shutdown.clone();
            let script_zones = script_zones
                .clone()
                .expect("script boundary and zone adapter are created together");
            command_tasks.spawn(async move {
                run_script_commands(ScriptCommandTask {
                    scripts: script_commands,
                    storage: script_storage,
                    config: script_config,
                    sessions: script_sessions,
                    runtime_control: script_runtime_control,
                    simulation: script_simulation,
                    chunk_pipeline_resources: script_chunk_pipeline_resources,
                    shutdown: script_shutdown,
                    zones: script_zones,
                })
                .await;
                "script command"
            });
        }
        let console_config = Arc::clone(&config);
        let console_sessions = Arc::clone(&sessions);
        let console_runtime_control = runtime_control.clone();
        let console_simulation = simulation.clone();
        let console_chunk_pipeline_resources = chunk_pipeline_resources.clone();
        command_tasks.spawn(async move {
            run_console_commands(
                console_config,
                console_sessions,
                console_runtime_control,
                console_simulation,
                console_chunk_pipeline_resources,
            )
            .await;
            "console command"
        });
        let mut entity_ticker_result = None;
        let mut command_drain_error = None;
        let mut connection_task_error = None;
        let mut entity_owner_error = None;
        let mut accept_error = None;
        loop {
            tokio::select! {
                result = self.listener.accept(), if connection_permits.available_permits() > 0 => {
                    let (socket, peer) = match result {
                        Ok(accepted) => accepted,
                        Err(error) => {
                            accept_error = Some(handle_accept_failure(
                                error,
                                &shutdown,
                                runtime_control.as_ref(),
                                &chunk_pipeline_resources,
                                &sessions,
                            ));
                            break;
                        }
                    };
                    let Some(pre_auth_permit) = pre_auth_admission.try_acquire(peer.ip()) else {
                        debug!(%peer, "pre-auth admission rejected connection");
                        continue;
                    };
                    debug!(%peer, "accepted connection");
                    let connection_permit = Arc::clone(&connection_permits)
                        .try_acquire_owned()
                        .expect("accept branch is enabled only while a connection permit exists");
                    let services = connection_services.clone();
                    connections.spawn(async move {
                        let _connection_permit = connection_permit;
                        if let Err(err) = Box::pin(handle_connection(
                            socket,
                            peer,
                            services,
                            pre_auth_permit,
                        ))
                        .await
                        {
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
                result = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(err)) = result {
                        connection_task_error = Some(connection_task_join_error(err));
                        if let Some(runtime_control) = runtime_control.as_ref() {
                            request_runtime_control_drain(
                                runtime_control,
                                &chunk_pipeline_resources,
                                &sessions,
                                &shutdown,
                            );
                        }
                        shutdown.request();
                        break;
                    }
                }
                changed = entity_owner_failure.changed() => {
                    let fatal_error = match changed {
                        Ok(()) => entity_owner_failure
                            .borrow_and_update()
                            .as_ref()
                            .map(|fatal| fatal.error),
                        Err(_) => Some(mc_entity::RegionOwnerLaneError::Closed),
                    };
                    if let Some(error) = fatal_error {
                        entity_owner_error = Some(entity_owner_serve_error(error));
                        if let Some(runtime_control) = runtime_control.as_ref() {
                            request_runtime_control_drain(
                                runtime_control,
                                &chunk_pipeline_resources,
                                &sessions,
                                &shutdown,
                            );
                        }
                        shutdown.request();
                        break;
                    }
                }
                result = command_tasks.join_next(), if !command_tasks.is_empty() => {
                    if let Some(result) = result
                        && let Err(error) = log_command_task_exit(result, shutdown.is_requested())
                    {
                        command_drain_error = Some(error);
                    }
                    if let Some(runtime_control) = runtime_control.as_ref() {
                        request_runtime_control_drain(
                            runtime_control,
                            &chunk_pipeline_resources,
                            &sessions,
                            &shutdown,
                        );
                    }
                    shutdown.request();
                    break;
                }
                result = &mut entity_ticker => {
                    entity_ticker_result = Some(handle_entity_ticker_exit(&shutdown, result));
                    break;
                }
                () = shutdown.notified() => {
                    if let Some(runtime_control) = runtime_control.as_ref() {
                        request_runtime_control_drain(
                            runtime_control,
                            &chunk_pipeline_resources,
                            &sessions,
                            &shutdown,
                        );
                    }
                    info!("shutdown requested; listener stopping");
                    break;
                }
            };
        }
        let connection_drain_result = drain_connections(&mut connections).await;
        if let Some(watcher) = runtime_control_signal_watcher
            && let Err(error) = watcher.await
        {
            warn!(%error, "runtime control signal watcher failed");
        }
        drain_chunk_pipeline(&chunk_pipeline_resources).await;
        let periodic_save_drain_result = drain_periodic_save_worker(periodic_save_worker).await;
        let entity_drain_result = match entity_ticker_result {
            Some(result) => result,
            None => {
                let simulation_barrier_result = simulation
                    .save_barrier(config.world.is_some())
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        std::io::Error::other(format!(
                            "simulation shutdown barrier failed: {error:?}"
                        ))
                    });
                let _ = entity_shutdown.send(());
                let ticker_result = drain_entity_ticker(entity_ticker).await;
                simulation_barrier_result.and(ticker_result)
            }
        };
        sessions.close_script_commit_event_outbox();
        let mut script_commit_event_drain_result = match script_commit_event_worker.take() {
            Some(worker) => match worker.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(std::io::Error::other(format!(
                    "committed script event drain failed: {error:?}"
                ))),
                Err(error) => Err(std::io::Error::other(format!(
                    "committed script event worker failed: {error}"
                ))),
            },
            None => Ok(()),
        };
        if let Some(watcher) = script_commit_event_failure_watcher.take() {
            watcher.abort();
            let _ = watcher.await;
        }
        let script_commit_events = sessions.script_commit_event_outbox_snapshot();
        if script_commit_events.required_overflow != 0
            || script_commit_events.required_closed != 0
            || script_commit_events.required_abandoned_on_receiver_drop != 0
        {
            script_commit_event_drain_result = Err(std::io::Error::other(format!(
                "required committed script event delivery failed: overflow={}, closed={}, abandoned={}, max_depth={}, capacity={}",
                script_commit_events.required_overflow,
                script_commit_events.required_closed,
                script_commit_events.required_abandoned_on_receiver_drop,
                script_commit_events.max_depth,
                script_commit_events.capacity,
            )));
        }
        let server_stopping_event_result = if let Some(scripts) = scripts.as_ref() {
            let result = scripts
                .enqueue_required_event(ScriptEvent::server_stopping("server stopping"))
                .await
                .map_err(|error| {
                    std::io::Error::other(format!("server stopping script event failed: {error:?}"))
                });
            scripts.close_event_admission();
            result
        } else {
            Ok(())
        };
        while let Some(result) = command_tasks.join_next().await {
            if let Err(error) = log_command_task_exit(result, true)
                && command_drain_error.is_none()
            {
                command_drain_error = Some(error);
            }
        }
        if let Some(error) = accept_error {
            return Err(error);
        }
        if let Some(error) = entity_owner_error {
            return Err(error);
        }
        if let Some(error) = connection_task_error {
            return Err(error);
        }
        if let Some(error) = command_drain_error {
            return Err(error);
        }
        connection_drain_result?;
        entity_drain_result?;
        periodic_save_drain_result?;
        script_commit_event_drain_result?;
        server_stopping_event_result
    }

    /// Serve until shutdown, drain every admitted mutation, and perform the
    /// final save. Callers that need to bind before spawning should use this
    /// instead of the drain-only [`Self::serve`].
    pub async fn serve_and_save(self) -> std::io::Result<()> {
        serve_then_final_save(self).await
    }
}

fn request_full_checkpoint(
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
    trigger: &'static str,
) {
    let Some(requests) = requests else {
        return;
    };
    debug!(tick, trigger, "full checkpoint requested");
    requests.request_full_checkpoint();
}

async fn persist_inhabited_time_tail(
    config: &ServerConfig,
    mutation: Option<&mc_world::WorldMutationView>,
    accumulator: &mut play::InhabitedTimeAccumulator,
) {
    let updates = accumulator.drain();
    let missing = mutation.map_or_else(
        || updates.clone(),
        |mutation| mutation.increment_chunk_inhabited_times(&updates),
    );
    if missing.is_empty() {
        return;
    }
    let Some(world) = config.world.as_ref() else {
        warn!(
            chunks = missing.len(),
            "cannot persist inhabited time without world storage"
        );
        return;
    };
    let mut storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "persist inhabited time tail",
        Instant::now(),
        world.lock().await,
    );
    let mut loaded = Vec::with_capacity(missing.len());
    for update @ (position, _) in missing {
        match storage.get_chunk_without_generation(position) {
            Ok(Some(_)) => loaded.push(update),
            Ok(None) => warn!(?position, "inhabited-time chunk vanished before shutdown"),
            Err(error) => warn!(?position, %error, "failed to load inhabited-time chunk"),
        }
    }
    let still_missing = storage
        .mutation_view()
        .increment_chunk_inhabited_times(&loaded);
    if !still_missing.is_empty() {
        warn!(
            chunks = still_missing.len(),
            "inhabited-time chunks vanished during shutdown publication"
        );
    }
}

async fn wait_for_session_empty_save_request(
    sessions: &play::SessionRegistry,
    observed: u64,
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
) -> u64 {
    sessions.wait_for_session_empty(observed).await;
    request_full_checkpoint(requests, tick, "last session unregistered");
    sessions.session_empty_generation()
}

async fn wait_for_player_save_request(
    sessions: &play::SessionRegistry,
    observed: u64,
    requests: Option<&crate::dirty_flush::DirtyFlushNotifier>,
    tick: u64,
) -> u64 {
    sessions.wait_for_player_save_request(observed).await;
    request_full_checkpoint(requests, tick, "player disconnected");
    sessions.player_save_generation()
}

pub(crate) async fn save_periodic_checkpoint(
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    simulation: &play::SimulationHandle,
    shutdown: &ShutdownHandle,
) -> Option<SaveAllReport> {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = tokio::select! {
        biased;
        () = shutdown.notified() => return None,
        guard = coordinator.lock() => guard,
    };
    let coordinator_us = elapsed_us(queue_started);
    let barrier_started = Instant::now();
    let barrier = tokio::select! {
        biased;
        () = shutdown.notified() => return None,
        result = simulation.save_barrier(config.world.is_some()) => result,
    };
    let snapshot = match barrier {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let elapsed = elapsed_us(total_started);
            return Some(SaveAllReport {
                players_saved: 0,
                entities_saved: 0,
                chunks_flushed: 0,
                world_metadata_saved: false,
                timings: SaveAllTimings {
                    queued_us: coordinator_us.saturating_add(elapsed_us(barrier_started)),
                    total_us: elapsed,
                    ..SaveAllTimings::default()
                },
                errors: vec![format!("simulation barrier failed: {error:?}")],
            });
        }
    };
    let barrier_us = elapsed_us(barrier_started);
    Some(
        save_all_with_context_snapshot_locked(
            "periodic checkpoint",
            config,
            sessions,
            Some(snapshot),
            false,
            coordinator_us.saturating_add(barrier_us),
            total_started,
        )
        .await,
    )
}

#[derive(Debug)]
struct DirtyOnlyFlushReport {
    planned_chunks: usize,
    flushed_chunks: usize,
    remaining_dirty: usize,
    immediately_flushable: bool,
}

/// Pressure-only fast path for the measured full-checkpoint storm: persist one
/// bounded chunk batch without taking a simulation save barrier or touching
/// players, entities, metadata, or WAL checkpoints. Generation checks in
/// `commit_dirty_flush` fence newer edits; periodic, disconnect, shutdown, and
/// explicit full checkpoints remain the fallback durability path.
async fn flush_dirty_chunks_only(
    config: &ServerConfig,
    simulation_tick: u64,
) -> Result<DirtyOnlyFlushReport, String> {
    let Some(world) = config.world.as_ref() else {
        return Ok(DirtyOnlyFlushReport {
            planned_chunks: 0,
            flushed_chunks: 0,
            remaining_dirty: 0,
            immediately_flushable: false,
        });
    };
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    let mut stale_retries = 0usize;
    loop {
        let (plan, dirty_before) = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "dirty-only flush plan",
                Instant::now(),
                world.lock().await,
            );
            if storage.world_root().is_none() {
                return Ok(DirtyOnlyFlushReport {
                    planned_chunks: 0,
                    flushed_chunks: 0,
                    remaining_dirty: storage.dirty_count(),
                    immediately_flushable: false,
                });
            }
            let dirty_before = storage.dirty_count();
            let plan = storage
                .plan_dirty_flush_at_tick_bounded(simulation_tick, DIRTY_ONLY_FLUSH_MAX_CHUNKS)
                .map_err(|error| format!("dirty-only flush plan failed: {error}"))?;
            (plan, dirty_before)
        };
        let planned_chunks = plan.chunk_count();
        if plan.is_empty() {
            return Ok(DirtyOnlyFlushReport {
                planned_chunks: 0,
                flushed_chunks: 0,
                remaining_dirty: dirty_before,
                immediately_flushable: false,
            });
        }
        let commit = match crate::dirty_flush::write_dirty_flush_blocking_typed(plan).await {
            Ok(commit) => commit,
            Err(error)
                if error.is_stale_region()
                    && stale_retries < DIRTY_ONLY_FLUSH_STALE_REGION_RETRIES =>
            {
                stale_retries += 1;
                continue;
            }
            Err(error) => return Err(format!("dirty-only flush write failed: {error}")),
        };
        let install = {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "dirty-only flush install",
                Instant::now(),
                world.lock().await,
            );
            match storage.install_dirty_flush(commit) {
                Ok(install) => install,
                Err(mc_world::WorldError::StaleRegion(_))
                    if stale_retries < DIRTY_ONLY_FLUSH_STALE_REGION_RETRIES =>
                {
                    stale_retries += 1;
                    continue;
                }
                Err(error) => return Err(format!("dirty-only flush install failed: {error}")),
            }
        };
        let synced = crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install)
            .await
            .map_err(|error| format!("dirty-only flush sync failed: {error}"))?;
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::SaveAllFlush,
            "dirty-only flush finalize",
            Instant::now(),
            world.lock().await,
        );
        let flushed_chunks = storage.finalize_dirty_flush(synced).cleaned_chunks();
        return Ok(DirtyOnlyFlushReport {
            planned_chunks,
            flushed_chunks,
            remaining_dirty: storage.dirty_count(),
            immediately_flushable: storage.has_flushable_dirty_chunks(),
        });
    }
}

fn log_dirty_only_flush(
    context: &'static str,
    result: Result<DirtyOnlyFlushReport, String>,
) -> crate::dirty_flush::DirtyFlushCompletion {
    match result {
        Ok(report) => {
            debug!(
                planned = report.planned_chunks,
                flushed = report.flushed_chunks,
                remaining_dirty = report.remaining_dirty,
                immediately_flushable = report.immediately_flushable,
                %context,
                "bounded dirty-only flush completed"
            );
            return if report.immediately_flushable {
                crate::dirty_flush::DirtyFlushCompletion::MoreDirty
            } else if report.remaining_dirty == 0 {
                crate::dirty_flush::DirtyFlushCompletion::Complete
            } else {
                crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer
            };
        }
        Err(error) => {
            warn!(%error, %context, "bounded dirty-only flush failed");
        }
    }
    crate::dirty_flush::DirtyFlushCompletion::Failed
}

async fn enqueue_startup_dirty_flush(
    config: &ServerConfig,
    requests: &crate::dirty_flush::DirtyFlushNotifier,
) {
    let Some(dirty_chunks) = startup_dirty_flush_dirty_count(config).await else {
        return;
    };
    info!(dirty = dirty_chunks, "startup dirty-only flush scheduled");
    requests.request_dirty_flush();
}

async fn startup_dirty_flush_dirty_count(config: &ServerConfig) -> Option<usize> {
    if config.shutdown.is_requested() {
        return None;
    }
    startup_dirty_flush_remaining_dirty_count(config).await
}

async fn startup_dirty_flush_remaining_dirty_count(config: &ServerConfig) -> Option<usize> {
    let world = config.world.as_ref()?;
    let storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::WorldStorage,
        "startup dirty-only flush dirty count",
        Instant::now(),
        world.lock().await,
    );
    storage.world_root()?;
    let dirty_chunks = storage.stats().dirty_chunks;
    (dirty_chunks > 0).then_some(dirty_chunks)
}

async fn run_extension_commands(
    extension: ExtensionEventSink,
    sessions: Arc<play::SessionRegistry>,
    shutdown: ShutdownHandle,
) {
    loop {
        tokio::select! {
            biased;
            () = shutdown.notified() => return,
            command = extension.recv_command() => {
                match command {
                    Ok(command) => handle_extension_command(&extension, &sessions, command),
                    Err(QueueRecvError::Closed) => {
                        debug!("extension command queue closed; stopping command drain");
                        return;
                    }
                    Err(_) => {}
                }
            }
        }
    }
}

struct ScriptCommandTask {
    scripts: ScriptEventSink,
    storage: Option<PluginStorageHandle>,
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    runtime_control: Option<RuntimeControlHandle>,
    simulation: play::SimulationHandle,
    chunk_pipeline_resources: ChunkPipelineResources,
    shutdown: ShutdownHandle,
    zones: PluginZoneAdapter,
}

async fn run_script_commands(task: ScriptCommandTask) {
    let ScriptCommandTask {
        scripts,
        storage,
        config,
        sessions,
        runtime_control,
        simulation,
        chunk_pipeline_resources,
        shutdown,
        zones,
    } = task;
    let router = ScriptRouter::new_with_zones(scripts.clone(), storage, zones);
    let mut shutdown_observed = shutdown.is_requested();
    loop {
        tokio::select! {
            biased;
            command = scripts.recv_command() => {
                let Some(command) = command else {
                    debug!("script command queue closed; stopping command drain");
                    break;
                };
                if router.route(
                    command,
                    ScriptRouter::context(
                        &config,
                        &sessions,
                        runtime_control.as_ref(),
                        &simulation,
                        &chunk_pipeline_resources,
                        &shutdown,
                    ),
                ).await == ScriptRouterExit::Stop {
                    return;
                }
            }
            () = router.wait_for_storage_stop() => {
                debug!("plugin storage actor stopped; stopping script command drain");
                break;
            }
            () = shutdown.notified(), if !shutdown_observed => {
                shutdown_observed = true;
                debug!("shutdown requested; draining commands until the script host closes");
            }
        }
    }
    let _ = router.zones().close();
}

pub(crate) fn resolve_script_entity_type(config: &ServerConfig, entity_type: &str) -> Option<i32> {
    let identifier = Identifier::parse(entity_type).ok()?;
    (identifier.as_str() == entity_type)
        .then(|| config.entity_types.id_of(&identifier))??
        .try_into()
        .ok()
}

fn runtime_task_join_error(task: &'static str, error: tokio::task::JoinError) -> std::io::Error {
    if error.is_panic() {
        let payload = error.into_panic();
        if let Some(owner_error) = play::entity_owner_fatal_from_panic(payload.as_ref()) {
            return entity_owner_serve_error(owner_error);
        }
        if let Some(lock) = mc_entity::authoritative_lock_poison_from_panic(payload.as_ref()) {
            return poisoned_runtime_serve_error(lock);
        }
        if let Some(lock) =
            crate::lock_policy::authoritative_lock_poison_from_panic(payload.as_ref())
        {
            return poisoned_runtime_serve_error(lock);
        }
        return std::io::Error::other(format!("{task} task panicked"));
    }
    std::io::Error::other(format!("{task} task join failed: {error}"))
}

fn connection_task_join_error(error: tokio::task::JoinError) -> std::io::Error {
    runtime_task_join_error("connection", error)
}

fn log_command_task_exit(
    result: Result<&'static str, tokio::task::JoinError>,
    shutdown_requested: bool,
) -> std::io::Result<()> {
    match result {
        Ok(task) if shutdown_requested => {
            debug!(task, "command task stopped during shutdown");
            Ok(())
        }
        Ok(task) => {
            warn!(task, "command task stopped unexpectedly");
            Ok(())
        }
        Err(error) => Err(runtime_task_join_error("command", error)),
    }
}

fn validate_extension_custom_payload_command(
    extension: &ExtensionEventSink,
    player_id: PlayerId,
    channel: String,
    payload: bytes::Bytes,
) -> Option<(mc_protocol::codec::Identifier, Vec<u8>)> {
    let policy = extension.custom_payload_policy();
    if !policy.allows_channel(&channel) {
        debug!(
            player_id = player_id.value(),
            channel, "extension custom payload command rejected by channel policy"
        );
        return None;
    }
    let max_payload_bytes = policy.max_payload_bytes();
    if payload.len() > max_payload_bytes {
        warn!(
            player_id = player_id.value(),
            len = payload.len(),
            max = max_payload_bytes,
            "extension custom payload command rejected by size policy"
        );
        return None;
    }
    let channel = match mc_protocol::codec::Identifier::parse(&channel) {
        Ok(channel) => channel,
        Err(error) => {
            debug!(
                player_id = player_id.value(),
                ?error,
                "extension custom payload command rejected invalid channel"
            );
            return None;
        }
    };
    Some((channel, payload.to_vec()))
}

fn handle_extension_command(
    extension: &ExtensionEventSink,
    sessions: &play::SessionRegistry,
    command: ExtensionOutboundCommand,
) {
    match command {
        ExtensionOutboundCommand::DisconnectPlayer { player_id, reason } => {
            if !sessions.disconnect_player(player_id.value(), reason) {
                debug!(
                    player_id = player_id.value(),
                    "extension disconnect command targeted unknown player"
                );
            }
        }
        ExtensionOutboundCommand::SendCustomPayload {
            player_id,
            channel,
            payload,
        } => {
            let Some((channel, payload)) =
                validate_extension_custom_payload_command(extension, player_id, channel, payload)
            else {
                return;
            };
            if !sessions.send_custom_payload(player_id.value(), channel, payload) {
                debug!(
                    player_id = player_id.value(),
                    "extension custom payload command targeted unknown player"
                );
            }
        }
        _ => debug!("unknown extension command ignored"),
    }
}

fn handle_entity_ticker_exit(
    shutdown: &ShutdownHandle,
    result: Result<(), tokio::task::JoinError>,
) -> std::io::Result<()> {
    let result = match result {
        Ok(()) if shutdown.is_requested() => {
            debug!("entity ticker stopped after shutdown request");
            Ok(())
        }
        Ok(()) => {
            warn!("entity ticker stopped unexpectedly; requesting server shutdown");
            Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "entity ticker stopped unexpectedly",
            ))
        }
        Err(error) => {
            warn!(%error, "entity ticker task failed; requesting server shutdown");
            Err(runtime_task_join_error("entity ticker", error))
        }
    };
    shutdown.request();
    result
}

async fn drain_entity_ticker(entity_ticker: tokio::task::JoinHandle<()>) -> std::io::Result<()> {
    drain_entity_ticker_with_timeout(entity_ticker, ENTITY_TICKER_DRAIN_TIMEOUT).await
}

async fn drain_entity_ticker_with_timeout(
    mut entity_ticker: tokio::task::JoinHandle<()>,
    timeout: Duration,
) -> std::io::Result<()> {
    match tokio::time::timeout(timeout, &mut entity_ticker).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(runtime_task_join_error("entity ticker", error)),
        Err(_) => {
            warn!("entity ticker drain timed out; cancelling task");
            entity_ticker.abort();
            match entity_ticker.await {
                Ok(()) => {}
                Err(error) if error.is_cancelled() => {}
                Err(error) => warn!(%error, "entity ticker failed while being cancelled"),
            }
            Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "entity ticker drain timed out",
            ))
        }
    }
}

async fn drain_periodic_save_worker(
    worker: Option<crate::dirty_flush::DirtyFlushCoordinator>,
) -> std::io::Result<()> {
    let Some(worker) = worker else {
        return Ok(());
    };
    match worker.drain().await {
        crate::dirty_flush::DirtyFlushDrainOutcome::Complete => Ok(()),
        crate::dirty_flush::DirtyFlushDrainOutcome::Failed(
            crate::dirty_flush::DirtyFlushDrainError::WorkerJoin(error),
        ) => Err(runtime_task_join_error("periodic save", error)),
        crate::dirty_flush::DirtyFlushDrainOutcome::Failed(error) => Err(std::io::Error::other(
            format!("periodic save worker: {error}"),
        )),
    }
}

async fn drain_connections(connections: &mut tokio::task::JoinSet<()>) -> std::io::Result<()> {
    drain_connections_with_timeout(connections, CONNECTION_DRAIN_TIMEOUT).await
}

async fn drain_connections_with_timeout(
    connections: &mut tokio::task::JoinSet<()>,
    timeout: Duration,
) -> std::io::Result<()> {
    let started = Instant::now();
    let mut join_error = None;
    while !connections.is_empty() {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            if let Some(error) = cancel_connection_tasks(connections).await {
                return Err(error);
            }
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "connection drain timed out",
            ));
        };
        match tokio::time::timeout(remaining, connections.join_next()).await {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(error))) => {
                warn!(%error, "connection task join failed");
                let error = connection_task_join_error(error);
                if is_uncertain_runtime_serve_error(&error) || join_error.is_none() {
                    join_error = Some(error);
                }
            }
            Ok(None) => break,
            Err(_) => {
                if let Some(error) = cancel_connection_tasks(connections).await {
                    return Err(error);
                }
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "connection drain timed out",
                ));
            }
        }
    }
    match join_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

async fn cancel_connection_tasks(
    connections: &mut tokio::task::JoinSet<()>,
) -> Option<std::io::Error> {
    warn!(
        remaining = connections.len(),
        "connection drain timed out; cancelling tasks"
    );
    connections.abort_all();
    let mut failure = None;
    while let Some(result) = connections.join_next().await {
        match result {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                warn!(%error, "connection task failed while being cancelled");
                let error = connection_task_join_error(error);
                if is_uncertain_runtime_serve_error(&error) || failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    failure
}

async fn drain_chunk_pipeline(resources: &ChunkPipelineResources) {
    resources.wait_for_idle().await;
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimulationCommandTelemetryScope {
    Tick,
    SincePreviousTickBoundary,
}

impl SimulationCommandTelemetryScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::SincePreviousTickBoundary => "since_previous_tick_boundary",
        }
    }
}

#[derive(Debug, Default)]
struct SimulationCommandGate {
    processed_since_tick: bool,
}

impl SimulationCommandGate {
    fn accepts_off_tick_batch(&self) -> bool {
        !self.processed_since_tick
    }

    fn record_off_tick_batch(&mut self) {
        self.processed_since_tick = true;
    }

    fn record_tick_boundary(&mut self) {
        self.processed_since_tick = false;
    }
}

#[derive(Debug, Default)]
struct SimulationCommandTelemetryWindow {
    off_tick_elapsed_us: u64,
    off_tick_processed: usize,
    includes_off_tick: bool,
}

impl SimulationCommandTelemetryWindow {
    fn record_off_tick(&mut self, elapsed_us: u64, processed: usize) {
        self.off_tick_elapsed_us = self.off_tick_elapsed_us.saturating_add(elapsed_us);
        self.off_tick_processed = self.off_tick_processed.saturating_add(processed);
        self.includes_off_tick = true;
    }

    fn finish_tick(
        &mut self,
        tick_elapsed_us: u64,
        tick_processed: usize,
    ) -> SimulationCommandTelemetry {
        let off_tick_elapsed_us = std::mem::take(&mut self.off_tick_elapsed_us);
        let off_tick_processed = std::mem::take(&mut self.off_tick_processed);
        let scope = if std::mem::take(&mut self.includes_off_tick) {
            SimulationCommandTelemetryScope::SincePreviousTickBoundary
        } else {
            SimulationCommandTelemetryScope::Tick
        };
        SimulationCommandTelemetry {
            elapsed_us: tick_elapsed_us.saturating_add(off_tick_elapsed_us),
            off_tick_elapsed_us,
            processed: tick_processed.saturating_add(off_tick_processed),
            scope,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SimulationCommandTelemetry {
    elapsed_us: u64,
    off_tick_elapsed_us: u64,
    processed: usize,
    scope: SimulationCommandTelemetryScope,
}

fn runtime_control_tick_input(tick_us: u64) -> RuntimeControlInput {
    RuntimeControlInput {
        tick_ms: tick_us.div_ceil(1_000),
        memory_used_mb: 0,
        memory_limit_mb: 0,
    }
}

fn runtime_work_input(
    percentiles: &RuntimeTickPercentiles,
    scheduled_budget_exhausted: bool,
) -> RuntimeWorkInput {
    RuntimeWorkInput {
        tick_p95_us: percentiles.tick.p95_us,
        entity_goals_p95_us: percentiles.entity_goals.p95_us,
        entity_physics_p95_us: percentiles.entity_physics.p95_us,
        entity_dispatch_p95_us: percentiles.entity_dispatch.p95_us,
        random_tick_p95_us: percentiles.random_tick.p95_us,
        block_tick_p95_us: percentiles.block_tick.p95_us,
        fluid_tick_p95_us: percentiles.fluid_tick.p95_us,
        scheduled_budget_exhausted,
    }
}

fn observe_runtime_control_tick(
    control: &RuntimeControlHandle,
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    shutdown: &ShutdownHandle,
    tick_us: u64,
) -> Option<crate::AutoscaleDecision> {
    match apply_runtime_control_operation(
        control,
        resources,
        sessions,
        shutdown,
        RuntimeControlOperation::Observe(runtime_control_tick_input(tick_us)),
    ) {
        Some(RuntimeControlOutcome::Autoscale(decision)) => Some(decision),
        Some(RuntimeControlOutcome::Work(_)) => unreachable!("tick observation is autoscale"),
        None => None,
    }
}

fn observe_runtime_control_signal(
    control: &RuntimeControlHandle,
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    shutdown: &ShutdownHandle,
    signal: RuntimeControlSignal,
) -> Option<crate::AutoscaleDecision> {
    match apply_runtime_control_operation(
        control,
        resources,
        sessions,
        shutdown,
        RuntimeControlOperation::ObserveSignal(signal),
    ) {
        Some(RuntimeControlOutcome::Autoscale(decision)) => Some(decision),
        Some(RuntimeControlOutcome::Work(_)) => unreachable!("signal observation is autoscale"),
        None => None,
    }
}

fn request_runtime_control_drain(
    control: &RuntimeControlHandle,
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    shutdown: &ShutdownHandle,
) -> Option<crate::AutoscaleDecision> {
    match apply_runtime_control_operation(
        control,
        resources,
        sessions,
        shutdown,
        RuntimeControlOperation::RequestDrain,
    ) {
        Some(RuntimeControlOutcome::Autoscale(decision)) => Some(decision),
        Some(RuntimeControlOutcome::Work(_)) => unreachable!("drain is autoscale"),
        None => None,
    }
}

fn apply_runtime_control_operation(
    control: &RuntimeControlHandle,
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    shutdown: &ShutdownHandle,
    operation: RuntimeControlOperation,
) -> Option<RuntimeControlOutcome> {
    match control.apply(operation, |outcome, proposed| {
        if let RuntimeControlOutcome::Autoscale(decision) = outcome {
            apply_runtime_control_decision(resources, sessions, decision, proposed.draining)?;
        }
        Ok(())
    }) {
        Ok(outcome) => Some(outcome),
        Err(RuntimeControlApplyError::ControlledStop { reason }) => {
            warn!(%reason, "runtime control application requires controlled shutdown");
            shutdown.request();
            None
        }
    }
}

fn apply_runtime_control_decision(
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    decision: &crate::AutoscaleDecision,
    draining: bool,
) -> Result<(), RuntimeControlApplyError> {
    let previous_cpu_limit = resources.cpu_limit();
    if decision.action == crate::AutoscaleAction::Hold {
        // Hold is the per-tick steady state. Avoid a synchronous regional-owner
        // command that would invalidate read routes without changing capacity.
        if decision.pressure == Some(crate::AutoscalePressure::Memory) {
            let removed = sessions.shed_prepared_chunks();
            if removed > 0 {
                debug!(removed, "memory pressure released shared prepared chunks");
            }
        }
        return Ok(());
    }
    let cpu_limit = resources.apply_runtime_control_action(decision.action, draining);
    if draining || cpu_limit != previous_cpu_limit {
        let entity_owner_lanes = sessions.reconfigure_entity_owner_lanes(cpu_limit);
        if entity_owner_lanes != cpu_limit {
            return Err(RuntimeControlApplyError::controlled_stop(format!(
                "runtime CPU admission applied {cpu_limit} workers but entity authority applied {entity_owner_lanes} owner lanes"
            )));
        }
    }
    if cpu_limit != previous_cpu_limit {
        info!(
            action = ?decision.action,
            cpu_limit,
            entity_owner_lanes = cpu_limit,
            reason = %decision.reason,
            "runtime background CPU admission changed"
        );
    }
    if decision.pressure == Some(crate::AutoscalePressure::Memory) {
        let removed = sessions.shed_prepared_chunks();
        if removed > 0 {
            debug!(removed, "memory pressure released shared prepared chunks");
        }
    }
    Ok(())
}

async fn recv_runtime_control_signal(
    signals: &mut Option<RuntimeControlSignalReceiver>,
) -> Option<RuntimeControlSignal> {
    match signals.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn forward_slow_client_sheds_to_runtime_control(
    sessions: Arc<play::SessionRegistry>,
    control: RuntimeControlHandle,
    shutdown: ShutdownHandle,
) {
    let mut generation = sessions.pressure_change_generation();
    let mut slow_client_pressure_sheds = 0;
    let initial = sessions.pressure_snapshot().slow_client_pressure_sheds;
    if initial > slow_client_pressure_sheds && !control.push_slow_client_shed() {
        debug!("runtime control signal consumer closed");
        return;
    }
    slow_client_pressure_sheds = initial;
    loop {
        tokio::select! {
            () = shutdown.notified() => return,
            () = sessions.wait_for_pressure_change(generation) => {
                generation = sessions.pressure_change_generation();
                let current = sessions.pressure_snapshot().slow_client_pressure_sheds;
                if current > slow_client_pressure_sheds && !control.push_slow_client_shed() {
                    debug!("runtime control signal consumer closed");
                    return;
                }
                slow_client_pressure_sheds = current;
            }
        }
    }
}

fn is_slow_tick(tick_us: u64, policy: RuntimeMetricsPolicy) -> bool {
    policy.slow_tick_ms > 0 && tick_us >= policy.slow_tick_ms.saturating_mul(1_000)
}

fn runtime_attributed_tick_us(
    sample: &RuntimeTickSample,
    simulation_commands_us: u64,
    furnace_tick_us: u64,
) -> u64 {
    [
        simulation_commands_us,
        sample.world_time_us,
        sample.sheep_grazing_us,
        sample.animal_breeding_us,
        sample.hostile_attacks_us,
        sample.entity_goals_us,
        sample.entity_physics_us,
        sample.entity_dispatch_us,
        sample.campfire_tick_us,
        furnace_tick_us,
        sample.inhabited_time_us,
        sample.entity_save_us,
        sample.random_tick_us,
        sample.block_tick_us,
        sample.fluid_tick_us,
    ]
    .into_iter()
    .fold(0, u64::saturating_add)
}

async fn run_console_commands(
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    runtime_control: Option<RuntimeControlHandle>,
    simulation: play::SimulationHandle,
    chunk_pipeline_resources: ChunkPipelineResources,
) {
    let mut lines = console_line_receiver();
    loop {
        let line = tokio::select! {
            line = lines.recv() => {
                match line {
                    Ok(line) => line,
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(skipped, "console command input lagged; dropping old lines");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
            () = config.shutdown.notified() => {
                info!("shutdown requested; console task stopping");
                return;
            }
        };
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }
        if execute_console_command(
            raw,
            "console save-all",
            "console stop",
            &config,
            &sessions,
            runtime_control.as_ref(),
            &simulation,
            &chunk_pipeline_resources,
        )
        .await
        {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_console_command(
    raw: &str,
    save_context: &'static str,
    stop_context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    runtime_control: Option<&RuntimeControlHandle>,
    simulation: &play::SimulationHandle,
    chunk_pipeline_resources: &ChunkPipelineResources,
) -> bool {
    match play::commands::parse_admin_command(raw, play::commands::CommandPermissions::CONSOLE) {
        Ok(play::commands::AdminCommand::SaveAll) => {
            let report =
                save_all_after_simulation_barrier(save_context, config, sessions, simulation).await;
            log_save_report(save_context, &report);
            false
        }
        Ok(play::commands::AdminCommand::Stop) => {
            request_stop(
                &config.shutdown,
                runtime_control,
                chunk_pipeline_resources,
                sessions,
            );
            info!(
                context = stop_context,
                "console stop requested runtime drain"
            );
            true
        }
        Ok(play::commands::AdminCommand::TimeSet(time)) => {
            match simulation.set_world_time_server_owned(time).await {
                Ok(()) => info!(time, "console set world time"),
                Err(error) => warn!(?error, time, "console failed to set world time"),
            }
            false
        }
        Ok(play::commands::AdminCommand::DaylightCycle(value)) => {
            if let Some(value) = value {
                sessions.set_daylight_cycle_enabled(value);
            }
            info!(
                value = sessions.daylight_cycle_enabled(),
                "console read daylight cycle"
            );
            false
        }
        Ok(play::commands::AdminCommand::PlayersSleepingPercentage(value)) => {
            if let Some(value) = value {
                sessions.set_players_sleeping_percentage(value);
            }
            info!(
                value = sessions.players_sleeping_percentage(),
                "console read players sleeping percentage"
            );
            false
        }
        Ok(command) => {
            warn!(
                ?command,
                "console command requires a player source in this M35 slice"
            );
            false
        }
        Err(error) => {
            warn!(
                error = console_command_error(error),
                "console command rejected"
            );
            false
        }
    }
}

fn console_line_receiver() -> broadcast::Receiver<String> {
    CONSOLE_LINES
        .get_or_init(|| {
            let (sender, _) = broadcast::channel(32);
            let reader_sender = sender.clone();
            if let Err(err) = std::thread::Builder::new()
                .name("solaris-console-input".to_owned())
                .spawn(move || {
                    let stdin = std::io::stdin();
                    let mut stdin = stdin.lock();
                    loop {
                        let mut line = String::new();
                        match stdin.read_line(&mut line) {
                            Ok(0) => return,
                            Ok(_) => {
                                let _ = reader_sender.send(line);
                            }
                            Err(err) => {
                                warn!(error = %err, "console command input failed");
                                return;
                            }
                        }
                    }
                })
            {
                warn!(error = %err, "console command input thread failed to start");
            }
            sender
        })
        .subscribe()
}

pub(crate) fn request_stop(
    shutdown: &ShutdownHandle,
    runtime_control: Option<&RuntimeControlHandle>,
    chunk_pipeline_resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
) {
    if let Some(runtime_control) = runtime_control {
        request_runtime_control_drain(
            runtime_control,
            chunk_pipeline_resources,
            sessions,
            shutdown,
        );
    }
    shutdown.request();
}

fn log_save_report(context: &'static str, report: &SaveAllReport) {
    if report.is_ok() {
        info!(
            players = report.players_saved,
            entities = report.entities_saved,
            chunks = report.chunks_flushed,
            world_metadata = report.world_metadata_saved,
            %context,
            "save-all complete"
        );
    } else {
        for error in &report.errors {
            warn!(%context, %error, "save-all error");
        }
    }
}

fn console_command_error(error: play::commands::CommandError) -> &'static str {
    match error {
        play::commands::CommandError::Unknown => "unknown command",
        play::commands::CommandError::PermissionDenied => "permission denied",
        play::commands::CommandError::Usage(usage) => usage,
    }
}

fn prepare_entity_physics_inputs(
    config: &ServerConfig,
    world_read: Option<&mc_world::WorldReadView>,
    queries: &[play::EntityPhysicsQuery],
) -> Vec<EntityPhysicsInput> {
    let Some(world_read) = world_read else {
        return Vec::new();
    };
    if queries.is_empty() {
        return Vec::new();
    }
    let materials = cached_material_ids(config);
    let plans = entity_physics_sample_plans(queries);
    let chunk_positions = entity_physics_chunk_positions(&plans);
    let world_snapshot = world_read.snapshot_chunks(&chunk_positions);
    let chunks = chunk_positions
        .into_iter()
        .map(|position| (position, world_snapshot.chunk(position)))
        .collect();
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks,
        materials,
        blocks: Some(Arc::clone(&config.blocks)),
    });
    entity_physics_inputs_from_snapshot(plans, snapshot)
}

struct CompletedScheduledBlockTicks {
    tick: u64,
    report: play::ScheduledBlockTickReport,
    elapsed_us: u64,
}

#[allow(clippy::too_many_arguments)]
fn spawn_scheduled_block_tick_job(
    tick: u64,
    budget: usize,
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    world_read: Option<mc_world::WorldReadView>,
    world_mutation: Option<mc_world::WorldMutationView>,
    protection: Option<Arc<crate::script::ZoneProtectionSnapshot>>,
    cpu_resources: ChunkPipelineResources,
) -> tokio::task::JoinHandle<CompletedScheduledBlockTicks> {
    let prepare_task = cpu_resources.begin_prepare_task();
    tokio::spawn(async move {
        let _prepare_task = prepare_task;
        let started = Instant::now();
        let report = play::run_scheduled_block_ticks_background(
            &config,
            &sessions,
            play::SimulationWorldAccess {
                read: world_read.as_ref(),
                mutation: world_mutation.as_ref(),
                cpu: Some(&cpu_resources),
                light: config.block_light.as_ref(),
            },
            protection,
            tick,
            budget,
        )
        .await;
        CompletedScheduledBlockTicks {
            tick,
            report,
            elapsed_us: elapsed_us(started),
        }
    })
}

#[derive(Default)]
struct MidTickSimulationCommands {
    report: play::SimulationTickReport,
    elapsed_us: u64,
}

async fn await_scheduled_block_tick_job_with_commands(
    mut job: tokio::task::JoinHandle<CompletedScheduledBlockTicks>,
    simulation_owner: &mut play::SimulationOwner,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    world_read: Option<&mc_world::WorldReadView>,
    world_mutation: Option<&mc_world::WorldMutationView>,
    cpu_resources: &ChunkPipelineResources,
) -> (
    Result<CompletedScheduledBlockTicks, tokio::task::JoinError>,
    MidTickSimulationCommands,
) {
    let mut commands = MidTickSimulationCommands::default();
    loop {
        tokio::select! {
            biased;
            result = &mut job => return (result, commands),
            ready = simulation_owner.wait_for_command() => {
                if !ready {
                    return (job.await, commands);
                }
                let started = Instant::now();
                let report = simulation_owner
                    .process_ready_commands_with_world_views(
                        sessions,
                        config.world.as_ref(),
                        play::SimulationWorldAccess {
                            read: world_read,
                            mutation: world_mutation,
                            cpu: Some(cpu_resources),
                            light: config.block_light.as_ref(),
                        },
                        config.block_light.as_deref(),
                        play::SIMULATION_COMMAND_BATCH_LIMIT,
                    )
                    .await;
                commands.elapsed_us = commands.elapsed_us.saturating_add(elapsed_us(started));
                commands.report.processed =
                    commands.report.processed.saturating_add(report.processed);
                commands.report.remaining_depth = report.remaining_depth;
                commands.report.lane_attribution.extend(report.lane_attribution);
            }
        }
    }
}

struct CompletedEntityPhysics {
    tick: u64,
    expected: Vec<play::EntityPhysicsQuery>,
    snapshot: Arc<EntityPhysicsSnapshot>,
    steps: Vec<play::EntityPhysicsStep>,
    arrow_physics_facts: Vec<play::ArrowPhysicsFact>,
}

fn spawn_entity_physics_job(
    tick: u64,
    expected: Vec<play::EntityPhysicsQuery>,
    cpu_resources: ChunkPipelineResources,
    inputs: Vec<EntityPhysicsInput>,
) -> tokio::task::JoinHandle<CompletedEntityPhysics> {
    debug_assert!(inputs.len() > ENTITY_PHYSICS_INLINE_LIMIT);
    let snapshot = Arc::clone(&inputs.first().expect("large physics batch").snapshot);
    let prepare_task = cpu_resources.begin_prepare_task();
    tokio::spawn(async move {
        let _prepare_task = prepare_task;
        let steps = step_entity_physics_inputs(cpu_resources, inputs).await;
        let arrow_physics_facts =
            arrow_physics_facts_from_steps(tick, &expected, &snapshot, &steps);
        CompletedEntityPhysics {
            tick,
            expected,
            snapshot,
            steps,
            arrow_physics_facts,
        }
    })
}

async fn wait_for_entity_physics_job(
    job: &mut Option<tokio::task::JoinHandle<CompletedEntityPhysics>>,
) -> Result<CompletedEntityPhysics, tokio::task::JoinError> {
    match job.as_mut() {
        Some(job) => job.await,
        None => std::future::pending().await,
    }
}

async fn apply_entity_physics_job_result(
    result: Result<CompletedEntityPhysics, tokio::task::JoinError>,
    simulation_owner: &play::SimulationOwner,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    cpu_resources: &ChunkPipelineResources,
    world_read: Option<&mc_world::WorldReadView>,
) {
    let completed = match result {
        Ok(completed) => completed,
        Err(error) if error.is_cancelled() => {
            debug!("entity physics job cancelled");
            return;
        }
        Err(error) => {
            warn!(%error, "entity physics job failed");
            return;
        }
    };
    let world_is_current = world_read
        .map(|world_read| entity_physics_snapshot_is_current(world_read, &completed.snapshot))
        .unwrap_or_else(|| completed.snapshot.chunks.is_empty());
    if !world_is_current {
        debug!(
            tick = completed.tick,
            entity_count = completed.expected.len(),
            "discarded entity physics result after world snapshot changed"
        );
        return;
    }
    let produced_steps = completed.steps.len();
    let accepted_steps = simulation_owner.apply_entity_physics_if_current(
        sessions,
        cpu_resources,
        completed.tick,
        &completed.expected,
        &completed.steps,
        &completed.arrow_physics_facts,
    );
    if accepted_steps.len() != produced_steps {
        debug!(
            tick = completed.tick,
            produced_steps,
            accepted_steps = accepted_steps.len(),
            "discarded stale entity physics results"
        );
    }
    let landed_falling_blocks = sessions.landed_falling_blocks(&accepted_steps);
    if !landed_falling_blocks.is_empty() {
        simulation_owner
            .land_falling_blocks(config, sessions, world_read, &landed_falling_blocks)
            .await;
    }
}

fn entity_physics_snapshot_is_current(
    world_read: &mc_world::WorldReadView,
    expected: &EntityPhysicsSnapshot,
) -> bool {
    let positions = expected.chunks.keys().copied().collect::<Vec<_>>();
    let current = world_read.snapshot_chunks(&positions);
    expected.chunks.iter().all(|(&position, expected_chunk)| {
        match (expected_chunk.as_ref(), current.chunk(position)) {
            (Some(expected_chunk), Some(current_chunk)) => {
                Arc::ptr_eq(expected_chunk, &current_chunk)
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    })
}

async fn step_entity_physics_inputs(
    cpu_resources: ChunkPipelineResources,
    inputs: Vec<EntityPhysicsInput>,
) -> Vec<play::EntityPhysicsStep> {
    if inputs.is_empty() {
        return Vec::new();
    }

    if inputs.len() <= ENTITY_PHYSICS_INLINE_LIMIT {
        return inputs.into_iter().map(step_sampled_entity).collect();
    }

    let workers = entity_physics_worker_count(&cpu_resources, inputs.len());
    let batch_size = inputs.len().div_ceil(workers);
    let mut batches = Vec::with_capacity(workers);
    let mut inputs = inputs.into_iter();
    for _ in 0..workers {
        let batch = inputs.by_ref().take(batch_size).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }
        let permit = match cpu_resources.acquire_cpu().await {
            Ok(permit) => permit,
            Err(error) => {
                warn!(%error, "entity physics CPU admission closed");
                break;
            }
        };
        batches.push(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            batch
                .into_iter()
                .map(step_sampled_entity)
                .collect::<Vec<_>>()
        }));
    }

    let mut steps = Vec::with_capacity(batches.len().saturating_mul(batch_size));
    for batch in batches {
        match batch.await {
            Ok(mut batch) => steps.append(&mut batch),
            Err(err) if err.is_cancelled() => debug!("entity physics worker cancelled"),
            Err(err) => warn!(error = %err, "entity physics worker failed"),
        }
    }
    steps
}

const ENTITY_PHYSICS_INLINE_LIMIT: usize = 256;
const ANIMAL_BREEDING_TICK_INTERVAL_TICKS: u16 = 20;

fn entity_physics_worker_count(
    cpu_resources: &ChunkPipelineResources,
    input_count: usize,
) -> usize {
    // Common herd sizes finish inside one tick inline. Blocking workers are for
    // larger batches where their scheduling overhead is amortized.
    if input_count == 0 {
        return 0;
    }
    cpu_resources
        .cpu_limit()
        .min(input_count.div_ceil(ENTITY_PHYSICS_INLINE_LIMIT))
        .max(1)
}

struct EntityPhysicsInput {
    query: play::EntityPhysicsQuery,
    snapshot: Arc<EntityPhysicsSnapshot>,
    complete_samples: bool,
}

struct EntityPhysicsSamplePlan {
    query: play::EntityPhysicsQuery,
    bounds: EntityPhysicsSampleBounds,
}

struct EntityPhysicsSnapshot {
    chunks: HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>>,
    materials: Arc<BlockMaterialIds>,
    blocks: Option<Arc<BlockRegistry>>,
}

struct SampledPhysicsWorld {
    snapshot: Arc<EntityPhysicsSnapshot>,
    entity_bottom: f64,
    fall_distance: f64,
    powder_snow_collision: PowderSnowCollision,
}

#[derive(Clone, Copy, Default)]
enum PowderSnowCollision {
    #[default]
    None,
    WalkableMob,
    FallingBlock,
}

impl SampledPhysicsWorld {
    fn for_query(snapshot: Arc<EntityPhysicsSnapshot>, query: play::EntityPhysicsQuery) -> Self {
        let powder_snow_collision = match query.kind {
            play::EntityPhysicsKind::PowderSnowWalkableLiving => PowderSnowCollision::WalkableMob,
            play::EntityPhysicsKind::FallingBlock => PowderSnowCollision::FallingBlock,
            _ => PowderSnowCollision::None,
        };
        Self {
            snapshot,
            entity_bottom: query.position.y,
            fall_distance: query.fall_distance,
            powder_snow_collision,
        }
    }

    fn without_entity_context(snapshot: Arc<EntityPhysicsSnapshot>) -> Self {
        Self {
            snapshot,
            entity_bottom: f64::NEG_INFINITY,
            fall_distance: 0.0,
            powder_snow_collision: PowderSnowCollision::None,
        }
    }

    fn state_id_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(MIN_Y..MAX_Y).contains(&y) {
            return None;
        }
        let cpos = mc_world::ChunkPos {
            x: x.div_euclid(mc_world::SECTION_DIM as i32),
            z: z.div_euclid(mc_world::SECTION_DIM as i32),
        };
        let chunk = self.snapshot.chunks.get(&cpos).and_then(Option::as_ref)?;
        let local_x = x.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        let local_z = z.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, y, local_z).map(|state| state.0)
    }
}

fn arrow_physics_facts_from_steps(
    tick: u64,
    expected: &[play::EntityPhysicsQuery],
    snapshot: &Arc<EntityPhysicsSnapshot>,
    steps: &[play::EntityPhysicsStep],
) -> Vec<play::ArrowPhysicsFact> {
    let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(snapshot));
    let mut expected = expected.iter();

    steps
        .iter()
        .filter_map(|step| {
            // Physics preserves query order and may omit rejected inputs. Walking
            // the ordered source once avoids an all-query index allocation.
            let query = expected.find(|query| query.id == step.id)?;
            let play::EntityPhysicsKind::ArrowProjectile { embedded_block, .. } = query.kind else {
                return None;
            };
            let endpoint_block =
                collision_block_touching_arrow_endpoint(&sampler, step.position, query.aabb);
            let block_hit = if query.velocity != mc_entity::Vec3::ZERO {
                endpoint_block.map(|(block_position, block_state)| play::ArrowBlockHitFact {
                    arrow_id: step.id,
                    block_state: mc_world::BlockStateId(block_state),
                    block_position,
                    // `step.position` is the contact endpoint resolved against this snapshot.
                    location: step.position,
                })
            } else {
                None
            };
            let retained_block_state = embedded_block.map(|position| {
                sampler
                    .state_id_at(position.x, position.y, position.z)
                    .unwrap_or(snapshot.materials.air)
            });
            let current_block_state = retained_block_state
                .or_else(|| endpoint_block.map(|(_, state)| state))
                .or_else(|| {
                    sampler.state_id_at(
                        step.position.x.floor() as i32,
                        step.position.y.floor() as i32,
                        step.position.z.floor() as i32,
                    )
                })
                .unwrap_or(snapshot.materials.air);
            let retained_supports_arrow = retained_block_state
                .is_some_and(|state| snapshot.materials.classify(state).is_solid());
            let embedded_in_block = if embedded_block.is_some() {
                retained_supports_arrow
            } else {
                endpoint_block.is_some()
            };
            let in_water = arrow_bounds_overlap_water(&sampler, step.position, query.aabb);
            Some(play::ArrowPhysicsFact {
                arrow_id: step.id,
                block_hit,
                embedded_in_block,
                current_block_state: mc_world::BlockStateId(current_block_state),
                should_fall: !embedded_in_block,
                fall_velocity_scale: arrow_fall_velocity_scale(step.id, tick),
                in_water,
                // Weather is not yet a world authority; water is the complete
                // supported source for this combined vanilla predicate.
                in_water_or_rain: in_water,
            })
        })
        .collect()
}

fn arrow_bounds_overlap_water(
    sampler: &SampledPhysicsWorld,
    position: mc_entity::Vec3,
    aabb: mc_physics::Aabb,
) -> bool {
    const BOUNDS_EPSILON: f64 = 1.0e-9;
    let min_x = (position.x - aabb.half_width + BOUNDS_EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width - BOUNDS_EPSILON).floor() as i32;
    let min_y = (position.y + BOUNDS_EPSILON).floor() as i32;
    let max_y = (position.y + aabb.height - BOUNDS_EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width + BOUNDS_EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width - BOUNDS_EPSILON).floor() as i32;
    (min_y..=max_y).any(|y| {
        (min_z..=max_z)
            .any(|z| (min_x..=max_x).any(|x| sampler.material_at(x, y, z) == BlockMaterial::Water))
    })
}

fn arrow_fall_velocity_scale(entity: mc_entity::EntityId, tick: u64) -> mc_entity::Vec3 {
    fn component(seed: u64) -> f64 {
        let mut value = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        (value >> 11) as f64 * (0.2 / ((1_u64 << 53) as f64))
    }

    let seed = tick ^ (entity.0 as i64 as u64).rotate_left(32);
    mc_entity::Vec3::new(
        component(seed),
        component(seed.wrapping_add(1)),
        component(seed.wrapping_add(2)),
    )
}

fn collision_block_touching_arrow_endpoint(
    sampler: &SampledPhysicsWorld,
    position: mc_entity::Vec3,
    aabb: mc_physics::Aabb,
) -> Option<(mc_entity::projectile_26_1_2::BlockPosition, u32)> {
    const CONTACT_EPSILON: f64 = 1.0e-9;
    let min_x = (position.x - aabb.half_width - CONTACT_EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width + CONTACT_EPSILON).floor() as i32;
    let min_y = (position.y - CONTACT_EPSILON).floor() as i32;
    let max_y = (position.y + aabb.height + CONTACT_EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width - CONTACT_EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width + CONTACT_EPSILON).floor() as i32;
    let mut first_colliding_state = None;

    for y in min_y..=max_y {
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let Some(state) = sampler.state_id_at(x, y, z) else {
                    continue;
                };
                sampler.collision_boxes_at(x, y, z, &mut |collision_box| {
                    let [
                        box_min_x,
                        box_min_y,
                        box_min_z,
                        box_max_x,
                        box_max_y,
                        box_max_z,
                    ] = collision_box.as_blocks();
                    let touches = position.x + aabb.half_width + CONTACT_EPSILON
                        >= f64::from(x) + box_min_x
                        && position.x - aabb.half_width - CONTACT_EPSILON
                            <= f64::from(x) + box_max_x
                        && position.y + aabb.height + CONTACT_EPSILON >= f64::from(y) + box_min_y
                        && position.y - CONTACT_EPSILON <= f64::from(y) + box_max_y
                        && position.z + aabb.half_width + CONTACT_EPSILON
                            >= f64::from(z) + box_min_z
                        && position.z - aabb.half_width - CONTACT_EPSILON
                            <= f64::from(z) + box_max_z;
                    if touches {
                        let candidate = (x, y, z, state);
                        if first_colliding_state.is_none_or(|first| candidate < first) {
                            first_colliding_state = Some(candidate);
                        }
                    }
                });
            }
        }
    }
    first_colliding_state.map(|(x, y, z, state)| {
        (
            mc_entity::projectile_26_1_2::BlockPosition::new(x, y, z),
            state,
        )
    })
}

#[derive(Clone, Copy)]
struct EntityPhysicsSampleBounds {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    min_z: i32,
    max_z: i32,
}

impl BlockSampler for SampledPhysicsWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.state_id_at(x, y, z)
            .map_or(BlockMaterial::Air, |state| {
                self.snapshot.materials.classify(state)
            })
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.state_id_at(x, y, z)
            .and_then(|state| self.snapshot.materials.collision_height(state))
    }

    fn max_collision_box_y(&self) -> u8 {
        let max_y = mc_data::collision_shapes::vanilla_collision_shapes().max_box_y();
        u8::try_from((max_y + 255) / 256).expect("vanilla collision height fits u8")
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        let Some(state) = self.state_id_at(x, y, z) else {
            return;
        };
        let exact_shape = self
            .snapshot
            .blocks
            .as_ref()
            .and_then(|blocks| blocks.by_id(mc_world::BlockStateId(state)))
            .and_then(|block| {
                mc_data::collision_shapes::vanilla_collision_shapes()
                    .get_for_state(state, &block.block.id, &block.properties)
                    .map(|shape| (block.block.id.as_str(), shape))
            });
        if let Some(("minecraft:powder_snow", _)) = exact_shape {
            if self.fall_distance > 2.5 {
                emit(BlockCollisionBox::from_fixed_4096([
                    0,
                    0,
                    0,
                    4096,
                    (0.9_f32 * 4096.0) as i16,
                    4096,
                ]));
                return;
            }
            match self.powder_snow_collision {
                PowderSnowCollision::FallingBlock => {
                    emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 16, 16));
                    return;
                }
                PowderSnowCollision::WalkableMob
                    if self.entity_bottom > f64::from(y) + 1.0 - 1.0e-5_f32 as f64 =>
                {
                    emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 16, 16));
                    return;
                }
                PowderSnowCollision::None | PowderSnowCollision::WalkableMob => {}
            }
        }
        if let Some((_, boxes)) = exact_shape {
            for collision_box in boxes.iter() {
                emit(BlockCollisionBox::from_fixed_4096(
                    collision_box.coordinates(),
                ));
            }
        } else if let Some(height) = self.snapshot.materials.collision_height(state) {
            let max_y = (height.as_blocks() * 16.0) as u8;
            emit(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, max_y, 16));
        }
    }
}

#[cfg(test)]
fn sample_entity_physics_input(
    query: play::EntityPhysicsQuery,
    storage: &mut WorldStorage,
    materials: &BlockMaterialIds,
) -> EntityPhysicsInput {
    let mut plans = entity_physics_sample_plans(&[query]);
    let chunks = entity_physics_chunk_snapshots(&plans, |cpos| storage.cached_chunk_snapshot(cpos));
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks,
        materials: Arc::new(materials.clone()),
        blocks: Some(storage.registry_arc()),
    });
    entity_physics_inputs_from_snapshot(std::mem::take(&mut plans), snapshot)
        .pop()
        .expect("one query yields one physics input")
}

fn entity_physics_sample_plans(
    queries: &[play::EntityPhysicsQuery],
) -> Vec<EntityPhysicsSamplePlan> {
    queries
        .iter()
        .copied()
        .map(|query| EntityPhysicsSamplePlan {
            query,
            bounds: entity_physics_sample_bounds(query),
        })
        .collect()
}

#[cfg(test)]
fn entity_physics_chunk_snapshots(
    plans: &[EntityPhysicsSamplePlan],
    mut cached_chunk: impl FnMut(mc_world::ChunkPos) -> Option<mc_world::ChunkSnapshot>,
) -> HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>> {
    entity_physics_chunk_positions(plans)
        .into_iter()
        .map(|position| (position, cached_chunk(position)))
        .collect()
}

fn entity_physics_chunk_positions(plans: &[EntityPhysicsSamplePlan]) -> Vec<mc_world::ChunkPos> {
    let mut chunks = HashSet::new();
    for plan in plans {
        if plan.bounds.max_y < MIN_Y || plan.bounds.min_y >= MAX_Y {
            continue;
        }
        let min_chunk_x = plan.bounds.min_x.div_euclid(mc_world::SECTION_DIM as i32);
        let max_chunk_x = plan.bounds.max_x.div_euclid(mc_world::SECTION_DIM as i32);
        let min_chunk_z = plan.bounds.min_z.div_euclid(mc_world::SECTION_DIM as i32);
        let max_chunk_z = plan.bounds.max_z.div_euclid(mc_world::SECTION_DIM as i32);
        for x in min_chunk_x..=max_chunk_x {
            for z in min_chunk_z..=max_chunk_z {
                let cpos = mc_world::ChunkPos { x, z };
                chunks.insert(cpos);
            }
        }
    }
    chunks.into_iter().collect()
}

fn entity_physics_inputs_from_snapshot(
    plans: Vec<EntityPhysicsSamplePlan>,
    snapshot: Arc<EntityPhysicsSnapshot>,
) -> Vec<EntityPhysicsInput> {
    plans
        .into_iter()
        .map(|plan| {
            let complete_samples = entity_physics_samples_are_complete(&plan, &snapshot.chunks);
            EntityPhysicsInput {
                query: plan.query,
                snapshot: Arc::clone(&snapshot),
                complete_samples,
            }
        })
        .collect()
}

fn entity_physics_samples_are_complete(
    plan: &EntityPhysicsSamplePlan,
    chunks: &HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>>,
) -> bool {
    if plan.bounds.max_y < MIN_Y || plan.bounds.min_y >= MAX_Y {
        return true;
    }
    let min_chunk_x = plan.bounds.min_x.div_euclid(mc_world::SECTION_DIM as i32);
    let max_chunk_x = plan.bounds.max_x.div_euclid(mc_world::SECTION_DIM as i32);
    let min_chunk_z = plan.bounds.min_z.div_euclid(mc_world::SECTION_DIM as i32);
    let max_chunk_z = plan.bounds.max_z.div_euclid(mc_world::SECTION_DIM as i32);
    for x in min_chunk_x..=max_chunk_x {
        for z in min_chunk_z..=max_chunk_z {
            if !chunks
                .get(&mc_world::ChunkPos { x, z })
                .is_some_and(Option::is_some)
            {
                return false;
            }
        }
    }
    true
}

fn entity_physics_sample_bounds(query: play::EntityPhysicsQuery) -> EntityPhysicsSampleBounds {
    let config = physics_config_for_query(query);
    let body = EntityBody {
        position: physics_vec(query.position),
        velocity: physics_vec(query.velocity),
        aabb: query.aabb,
        on_ground: query.on_ground,
    };
    let next_x = body.position.x + body.velocity.x * config.tick_seconds;
    let next_y = body.position.y + body.velocity.y * config.tick_seconds;
    let next_z = body.position.z + body.velocity.z * config.tick_seconds;
    let half = body.aabb.half_width;
    let min_x = (body.position.x.min(next_x) - half - 1.0).floor() as i32;
    let max_x = (body.position.x.max(next_x) + half + 1.0).floor() as i32;
    let min_z = (body.position.z.min(next_z) - half - 1.0).floor() as i32;
    let max_z = (body.position.z.max(next_z) + half + 1.0).floor() as i32;
    let min_y = (body.position.y.min(next_y) - 2.0).floor() as i32;
    let max_y = (body.position.y.max(next_y) + body.aabb.height + 2.0).floor() as i32;

    EntityPhysicsSampleBounds {
        min_x,
        max_x,
        min_y,
        max_y,
        min_z,
        max_z,
    }
}

fn step_sampled_entity(input: EntityPhysicsInput) -> play::EntityPhysicsStep {
    if !input.complete_samples {
        return play::EntityPhysicsStep {
            id: input.query.id,
            position: input.query.position,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: input.query.on_ground,
            horizontal_collision: false,
        };
    }
    let sampler = SampledPhysicsWorld::for_query(input.snapshot, input.query);
    let result = mc_physics::step_entity(
        EntityBody {
            position: physics_vec(input.query.position),
            velocity: physics_vec(input.query.velocity),
            aabb: input.query.aabb,
            on_ground: input.query.on_ground,
        },
        &sampler,
        physics_config_for_query(input.query),
    );
    play::EntityPhysicsStep {
        id: input.query.id,
        position: entity_vec(result.body.position),
        velocity: entity_vec(result.body.velocity),
        on_ground: result.body.on_ground,
        horizontal_collision: result.horizontal_collision
            && matches!(
                input.query.kind,
                play::EntityPhysicsKind::Living
                    | play::EntityPhysicsKind::PowderSnowWalkableLiving
                    | play::EntityPhysicsKind::AquaticLiving
            ),
    }
}

fn physics_config_for_query(query: play::EntityPhysicsQuery) -> PhysicsConfig {
    match query.kind {
        play::EntityPhysicsKind::Default => PhysicsConfig::default(),
        play::EntityPhysicsKind::Living | play::EntityPhysicsKind::PowderSnowWalkableLiving => {
            PhysicsConfig::living_entity()
        }
        play::EntityPhysicsKind::AquaticLiving => PhysicsConfig::aquatic_entity(),
        play::EntityPhysicsKind::FallingBlock => PhysicsConfig::default(),
        play::EntityPhysicsKind::ArrowProjectile { .. } => {
            let mut config = PhysicsConfig::arrow_projectile();
            // Retained projectile velocity is blocks per Minecraft tick. This
            // adapter resolves only the authoritative collision endpoint; the
            // projectile kernel owns drag and gravity after impact ordering.
            config.tick_seconds = 1.0;
            config.gravity = 0.0;
            config.air_drag = 1.0;
            config.vertical_air_drag = 1.0;
            config.water_drag = 1.0;
            config
        }
    }
}

fn physics_vec(vec: mc_entity::Vec3) -> mc_physics::Vec3 {
    mc_physics::Vec3::new(vec.x, vec.y, vec.z)
}

fn entity_vec(vec: mc_physics::Vec3) -> mc_entity::Vec3 {
    mc_entity::Vec3::new(vec.x, vec.y, vec.z)
}

fn cached_material_ids(config: &ServerConfig) -> Arc<BlockMaterialIds> {
    let key = (
        Arc::as_ptr(&config.blocks) as usize,
        Arc::as_ptr(&config.block_facts) as usize,
    );
    let cache_lock = PHYSICS_MATERIAL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut cache =
        crate::lock_policy::lock_benign_mutex(cache_lock, "server.physics_material_cache");
    if let Some((blocks, facts, materials)) = cache.get(&key)
        && blocks
            .upgrade()
            .is_some_and(|blocks| Arc::ptr_eq(&blocks, &config.blocks))
        && facts
            .upgrade()
            .is_some_and(|facts| Arc::ptr_eq(&facts, &config.block_facts))
    {
        return Arc::clone(materials);
    }

    let materials = Arc::new(material_ids(&config.blocks, &config.block_facts));
    cache.insert(
        key,
        (
            Arc::downgrade(&config.blocks),
            Arc::downgrade(&config.block_facts),
            Arc::clone(&materials),
        ),
    );
    materials
}

fn material_ids(blocks: &BlockRegistry, facts: &BlockFactsTable) -> BlockMaterialIds {
    let state = |name: &str| {
        blocks
            .block(&Identifier::parse(name).expect("static identifier"))
            .map(|block| block.default.0)
    };
    let passable = crate::play::passive_entity_passable_blocks(blocks)
        .into_iter()
        .map(|state| state.0)
        .collect();
    let farmland = blocks
        .block(&Identifier::parse("minecraft:farmland").expect("static identifier"))
        .map(|block| block.states.iter().map(|state| state.0).collect())
        .unwrap_or_default();

    BlockMaterialIds::new(
        state("minecraft:air").unwrap_or(0),
        state("minecraft:water"),
        state("minecraft:lava"),
    )
    .with_water_states(fluid_material_states(blocks, facts, FluidKind::Water))
    .with_lava_states(fluid_material_states(blocks, facts, FluidKind::Lava))
    .with_passable(passable)
    .with_collision_height(farmland, BlockCollisionHeight::from_sixteenths(15))
}

fn fluid_material_states(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    kind: FluidKind,
) -> Vec<u32> {
    blocks
        .states()
        .filter(|state| {
            facts
                .fluid(state.id.0)
                .is_some_and(|fluid| fluid.kind == kind)
        })
        .map(|state| state.id.0)
        .collect()
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
    bind_internal(config, None, None).await
}

/// Bind with an explicit extension boundary. The default [`bind`] path keeps
/// extension dispatch disabled.
pub async fn bind_with_extension(
    config: ServerConfig,
    boundary: ExtensionBoundary,
    custom_payload_policy: CustomPayloadPolicy,
) -> std::io::Result<BoundServer> {
    bind_internal(
        config,
        Some(ExtensionEventSink::new(boundary, custom_payload_policy)),
        None,
    )
    .await
}

/// Bind with the bounded server-side script API enabled.
pub async fn bind_with_scripts(
    config: ServerConfig,
    boundary: ScriptBoundary,
) -> std::io::Result<BoundServer> {
    bind_internal(config, None, Some(ScriptEventSink::new(boundary))).await
}

async fn bind_internal(
    mut config: ServerConfig,
    extension: Option<ExtensionEventSink>,
    scripts: Option<ScriptEventSink>,
) -> std::io::Result<BoundServer> {
    if config.recipes.is_empty() {
        config.recipes = Arc::new(mc_data::recipes::solaris_required_recipes());
    }
    validate_public_security_config(config.bind_address, &config.command_permissions)?;
    let online_authentication =
        build_online_authentication(config.command_permissions.login_access())?;
    let listener = TcpListener::bind(config.bind_address).await?;
    let chunk_pipeline_resources = ChunkPipelineResources::new(config.chunk_pipeline);
    let runtime_control = config
        .chunk_pipeline
        .runtime_control
        .map(RuntimeControlHandle::new);
    let runtime_tick_metrics = RuntimeTickMetricsHandle::default();
    let entity_world_root = if let Some(world) = config.world.as_ref() {
        world
            .lock()
            .await
            .world_root()
            .map(std::path::Path::to_path_buf)
    } else {
        None
    };
    let script_zones = scripts
        .as_ref()
        .map(|scripts| PluginZoneAdapter::new(scripts.clone()));
    let (sessions, pending_entity_commits) = if let Some(root) = entity_world_root.as_deref() {
        let (journal, pending) = play::persistence::FileRegionalDecisionJournal::open(root)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
        (
            Arc::new(
                play::SessionRegistry::try_new_with_entity_owner_journal(
                    chunk_pipeline_resources.cpu_limit(),
                    Box::new(journal),
                )
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "failed to start regional entity owner runtime: {:?}",
                        error.error
                    ))
                })?,
            ),
            pending,
        )
    } else {
        (
            Arc::new(
                play::SessionRegistry::try_new_with_entity_owner_lanes(
                    chunk_pipeline_resources.cpu_limit(),
                )
                .map_err(|error| {
                    std::io::Error::other(format!(
                        "failed to start regional entity owner runtime: {:?}",
                        error.error
                    ))
                })?,
            ),
            Vec::new(),
        )
    };
    let script_storage = match (scripts.as_ref(), entity_world_root.as_deref()) {
        (Some(scripts), Some(root)) => Some(
            PluginStorageHandle::start(
                root,
                scripts.clone(),
                config.shutdown.clone(),
                Arc::clone(&sessions),
                Arc::clone(&config.items),
                Arc::clone(&config.item_facts),
            )
            .map_err(plugin_storage_bind_error)?,
        ),
        _ => None,
    };
    if let (Some(root), Some(world)) = (entity_world_root.as_deref(), config.world.as_ref()) {
        let (journal, pending) = play::world_journal::WorldChunkJournal::open(
            root,
            Arc::clone(&config.blocks),
            Arc::clone(&config.items),
        )
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
        let chunks = journal
            .decode_pending(&pending)
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
        let pending_images = chunks.len();
        if pending_images != 0 {
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "restore world chunk journal",
                Instant::now(),
                world.lock().await,
            );
            let mut restored = 0usize;
            for chunk in chunks {
                restored += usize::from(
                    storage
                        .replay_journal_chunk(chunk)
                        .map_err(std::io::Error::other)?,
                );
            }
            info!(
                pending_images,
                restored, "replayed pending world chunk journal images"
            );
        }
        sessions.install_world_chunk_journal(journal);
    }
    let (simulation, simulation_owner) =
        play::simulation_channel_with_explosion_seed(config.random_tick.seed as i64);
    play::configure_session_arrow_kill_rewards(&sessions, &config);
    play::configure_session_player_combat(&sessions, &config);
    play::prepare_spawn_chunk(&config, chunk_pipeline_resources.clone())
        .await
        .map_err(|error| {
            std::io::Error::other(format!("failed to prepare the spawn chunk: {error}"))
        })?;
    let mut chunk_geometry = OVERWORLD_GEOMETRY;
    let connection_world = if let Some(world) = config.world.as_ref() {
        let access = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "bind connection world",
                Instant::now(),
                world.lock().await,
            );
            if let Some(spawn) = storage.cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 }) {
                chunk_geometry = spawn.geometry();
            }
            ConnectionWorld {
                root: storage
                    .world_root()
                    .map(std::path::Path::to_path_buf)
                    .map(Arc::new),
                read: Some(storage.read_view()),
                mutation: Some(storage.mutation_view()),
                chunk_source: Some(storage.chunk_source_view()),
            }
        };
        if let Some(root) = access.root.as_deref() {
            match play::persistence::load_world_metadata(root) {
                Ok(Some(metadata)) => {
                    let expected = play::persistence::world_identity(root);
                    if !metadata.world_identity.is_empty() && metadata.world_identity != expected {
                        return Err(std::io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "world metadata identity mismatch: stored={}, expected={expected}",
                                metadata.world_identity
                            ),
                        ));
                    }
                    simulation_owner.restore_world_time(&sessions, metadata.world_time);
                    sessions.set_daylight_cycle_enabled(metadata.daylight_cycle_enabled);
                    sessions.set_players_sleeping_percentage(metadata.players_sleeping_percentage);
                    sessions.set_keep_inventory(metadata.keep_inventory);
                    info!(
                        world_time = metadata.world_time,
                        daylight_cycle_enabled = metadata.daylight_cycle_enabled,
                        players_sleeping_percentage = metadata.players_sleeping_percentage,
                        keep_inventory = metadata.keep_inventory,
                        "loaded world metadata"
                    );
                }
                Ok(None) => {}
                Err(err) => {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("world metadata load failed: {err}"),
                    ));
                }
            }
            match play::persistence::load_persisted_entities(
                root,
                &config.items,
                &config.entity_types,
            ) {
                Ok(entities) => {
                    let entities = play::persistence::replay_regional_commit_decisions(
                        entities,
                        &pending_entity_commits,
                    )
                    .map_err(|error| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            format!("regional entity recovery failed: {error}"),
                        )
                    })?;
                    let lifecycle_epoch = entities.lifecycle_clock;
                    let expected = entities.records.len();
                    let restored = simulation_owner.restore_persisted_entities(&sessions, entities);
                    if restored != expected {
                        return Err(std::io::Error::new(
                            ErrorKind::InvalidData,
                            format!(
                                "regional entity recovery restored {restored} of {expected} entities"
                            ),
                        ));
                    }
                    sessions.synchronize_entity_lifecycle_epoch(lifecycle_epoch);
                    if restored > 0 {
                        info!(restored, "loaded persisted entities");
                    }
                }
                Err(err) => {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        format!("persisted entity load failed: {err}"),
                    ));
                }
            }
        }
        access
    } else {
        ConnectionWorld::default()
    };
    play::hydrate_persisted_campfire_cooking_strict(&config, &sessions)
        .await
        .map_err(|error| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("persisted campfire recovery failed: {error}"),
            )
        })?;
    play::recover_pending_campfire_outputs(&config, &sessions, &simulation_owner)
        .await
        .map_err(|error| {
            std::io::Error::new(
                ErrorKind::InvalidData,
                format!("pending campfire output recovery failed: {error}"),
            )
        })?;
    Ok(BoundServer {
        listener,
        config: Arc::new(config),
        online_authentication,
        chunk_geometry,
        connection_world,
        chunk_pipeline_resources,
        runtime_control,
        runtime_tick_metrics,
        sessions,
        simulation,
        simulation_owner,
        extension,
        scripts,
        script_storage,
        script_zones,
    })
}

fn plugin_storage_bind_error(error: crate::PluginStorageStartError) -> std::io::Error {
    let kind = match &error {
        crate::PluginStorageStartError::Io(source) => source.kind(),
        crate::PluginStorageStartError::Malformed(_)
        | crate::PluginStorageStartError::JournalTooLarge
        | crate::PluginStorageStartError::LiveQuotaExceeded => ErrorKind::InvalidData,
    };
    std::io::Error::new(kind, error)
}

fn validate_public_security_config(
    bind_address: SocketAddr,
    command_permissions: &CommandPermissionConfig,
) -> std::io::Result<()> {
    if !is_public_bind(bind_address) {
        return Ok(());
    }
    if command_permissions.allow_local_dev_operators {
        return Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "allow_local_dev_operators cannot be enabled on a public bind address",
        ));
    }
    if command_permissions.login_access().online_mode {
        Ok(())
    } else {
        Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "offline-mode Solaris authentication cannot be used on a public bind address",
        ))
    }
}

fn build_online_authentication(
    access: &login::LoginAccessConfig,
) -> std::io::Result<Option<Arc<login::OnlineAuthentication>>> {
    if !access.online_mode {
        return Ok(None);
    }
    let verifier = match access.session_verifier() {
        Some(verifier) => verifier,
        None => Arc::new(crate::MojangSessionVerifier::new().map_err(|error| {
            std::io::Error::other(format!(
                "failed to construct Mojang session verifier: {error}"
            ))
        })?),
    };
    let identity = crate::RsaIdentity::generate().map_err(|error| {
        std::io::Error::other(format!(
            "failed to generate online-mode RSA identity: {error}"
        ))
    })?;
    Ok(Some(Arc::new(login::OnlineAuthentication::new(
        identity,
        verifier,
        access.prevent_proxy_connections(),
    ))))
}

fn is_public_bind(addr: SocketAddr) -> bool {
    if is_loopback_peer(addr) {
        return false;
    }
    match addr.ip() {
        std::net::IpAddr::V4(ip) => !ip.is_private() && !ip.is_link_local(),
        std::net::IpAddr::V6(ip) => !ip.is_unique_local(),
    }
}

#[cfg(test)]
async fn save_all(config: &ServerConfig, sessions: &play::SessionRegistry) -> SaveAllReport {
    save_all_with_context("save-all", config, sessions).await
}

async fn save_all_after_drain_with_context(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
) -> SaveAllReport {
    save_all_with_context_snapshot(context, config, sessions, None, true).await
}

pub(crate) async fn save_all_after_simulation_barrier(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    simulation: &play::SimulationHandle,
) -> SaveAllReport {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    let coordinator_us = elapsed_us(queue_started);
    let barrier_started = Instant::now();
    let mut journal_failure = sessions.subscribe_world_chunk_journal_failure();
    let snapshot = loop {
        let dirty_tail_generation = config.shutdown.dirty_tail_generation();
        let snapshot = match simulation.save_barrier(config.world.is_some()).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return save_barrier_error_report(
                    total_started,
                    coordinator_us.saturating_add(elapsed_us(barrier_started)),
                    format!("simulation barrier failed: {error:?}"),
                );
            }
        };
        let captures_all_dirty_chunks = snapshot
            .world_flush_plan
            .as_ref()
            .is_none_or(mc_world::DirtyFlushPlan::captures_all_dirty_chunks);
        if captures_all_dirty_chunks {
            break snapshot;
        }
        if *journal_failure.borrow() {
            return save_barrier_error_report(
                total_started,
                coordinator_us.saturating_add(elapsed_us(barrier_started)),
                "world chunk journal failed while completing save barrier".to_string(),
            );
        }

        tokio::select! {
            () = config
                .shutdown
                .wait_for_dirty_tail_progress(dirty_tail_generation) => {}
            changed = journal_failure.changed() => {
                if changed.is_err() || *journal_failure.borrow() {
                    return save_barrier_error_report(
                        total_started,
                        coordinator_us.saturating_add(elapsed_us(barrier_started)),
                        "world chunk journal failed while completing save barrier".to_string(),
                    );
                }
            }
        }
    };
    let barrier_us = elapsed_us(barrier_started);
    save_all_with_context_snapshot_locked(
        context,
        config,
        sessions,
        Some(snapshot),
        false,
        coordinator_us.saturating_add(barrier_us),
        total_started,
    )
    .await
}

fn save_barrier_error_report(
    total_started: Instant,
    queued_us: u64,
    error: String,
) -> SaveAllReport {
    SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        timings: SaveAllTimings {
            queued_us,
            total_us: elapsed_us(total_started),
            ..SaveAllTimings::default()
        },
        errors: vec![error],
    }
}

#[cfg(test)]
async fn save_all_with_context(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
) -> SaveAllReport {
    save_all_with_context_snapshot(context, config, sessions, None, false).await
}

async fn save_all_with_context_snapshot(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    snapshot: Option<play::SimulationSaveSnapshot>,
    require_clean_dirty_flush: bool,
) -> SaveAllReport {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let coordinator = config.shutdown.save_coordinator();
    let _save_guard = coordinator.lock().await;
    save_all_with_context_snapshot_locked(
        context,
        config,
        sessions,
        snapshot,
        require_clean_dirty_flush,
        elapsed_us(queue_started),
        total_started,
    )
    .await
}

async fn save_all_with_context_snapshot_locked(
    context: &'static str,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    snapshot: Option<play::SimulationSaveSnapshot>,
    require_clean_dirty_flush: bool,
    queued_us: u64,
    total_started: Instant,
) -> SaveAllReport {
    let barrier_snapshot = snapshot.is_some();
    let mut report = SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        timings: SaveAllTimings {
            queued_us,
            ..SaveAllTimings::default()
        },
        errors: Vec::new(),
    };
    let Some(world) = config.world.as_ref() else {
        report.timings.total_us = elapsed_us(total_started);
        return report;
    };
    let root = {
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "save-all world root",
            Instant::now(),
            world.lock().await,
        );
        storage.world_root().map(std::path::Path::to_path_buf)
    };
    let Some(root) = root else {
        report.timings.total_us = elapsed_us(total_started);
        return report;
    };
    let (
        simulation_tick,
        players,
        entities,
        entity_journal_phases,
        world_chunk_journal_watermark,
        world_time,
        daylight_cycle_enabled,
        players_sleeping_percentage,
        keep_inventory,
        mut world_flush_plan,
    ) = match snapshot {
        Some(snapshot) => (
            snapshot.simulation_tick,
            snapshot.players,
            snapshot.entities,
            snapshot.entity_journal_phases,
            snapshot.world_chunk_journal_watermark,
            snapshot.world_time,
            snapshot.daylight_cycle_enabled,
            snapshot.players_sleeping_percentage,
            snapshot.keep_inventory,
            snapshot.world_flush_plan,
        ),
        None => {
            let (entities, entity_journal_phases) = sessions.persisted_entity_save_snapshot();
            (
                sessions.simulation_tick(),
                sessions.persisted_player_states(),
                entities,
                entity_journal_phases,
                sessions.world_chunk_journal_watermark(),
                sessions.world_time(),
                sessions.daylight_cycle_enabled(),
                sessions.players_sleeping_percentage(),
                sessions.keep_inventory(),
                None,
            )
        }
    };

    let mut world_flush_clean = false;
    let mut attempt = 0usize;
    loop {
        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::SaveAllFlush,
            "save-all dirty flush plan",
            Instant::now(),
            world.lock().await,
        );
        let storage_before = storage.stats();
        let flush_plan = if let Some(plan) = world_flush_plan.take() {
            plan
        } else {
            match storage.plan_dirty_flush_at_tick(simulation_tick) {
                Ok(plan) => plan,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush plan failed: {err}"));
                    break;
                }
            }
        };
        let flushable_before = storage.has_flushable_dirty_chunks();
        drop(storage);
        report.timings.flush_plan_us = report
            .timings
            .flush_plan_us
            .saturating_add(elapsed_us(started));

        let planned_chunks = flush_plan.chunk_count();
        let flush_started = Instant::now();
        let (remaining_dirty, has_flushable_dirty) = if flush_plan.is_empty() {
            info!(
                attempt,
                flushed = 0usize,
                planned = 0usize,
                flush_us = elapsed_us(flush_started),
                chunk_cache_len = storage_before.chunk_cache_len,
                chunk_cache_capacity = storage_before.chunk_cache_capacity,
                region_cache_len = storage_before.region_cache_len,
                region_cache_capacity = storage_before.region_cache_capacity,
                dirty_before = storage_before.dirty_chunks,
                dirty_after = storage_before.dirty_chunks,
                %context,
                "world storage save pressure"
            );
            (storage_before.dirty_chunks, flushable_before)
        } else {
            let started = Instant::now();
            let commit = match crate::dirty_flush::write_dirty_flush_blocking(flush_plan).await {
                Ok(commit) => commit,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush write failed: {err}"));
                    report.timings.flush_write_us = report
                        .timings
                        .flush_write_us
                        .saturating_add(elapsed_us(started));
                    break;
                }
            };
            report.timings.flush_write_us = report
                .timings
                .flush_write_us
                .saturating_add(elapsed_us(started));

            let started = Instant::now();
            let install = {
                let mut storage = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::SaveAllFlush,
                    "save-all dirty flush install",
                    Instant::now(),
                    world.lock().await,
                );
                if barrier_snapshot {
                    storage.install_dirty_flush_snapshot(commit)
                } else {
                    storage.install_dirty_flush(commit)
                }
            };
            let install = match install {
                Ok(install) => install,
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush install failed: {err}"));
                    report.timings.flush_commit_us = report
                        .timings
                        .flush_commit_us
                        .saturating_add(elapsed_us(started));
                    break;
                }
            };
            let synced =
                match crate::dirty_flush::sync_dirty_flush_install_blocking_typed(install).await {
                    Ok(synced) => synced,
                    Err(err) => {
                        report
                            .errors
                            .push(format!("dirty chunks: flush sync failed: {err}"));
                        report.timings.flush_commit_us = report
                            .timings
                            .flush_commit_us
                            .saturating_add(elapsed_us(started));
                        break;
                    }
                };
            let mut storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::SaveAllFlush,
                "save-all dirty flush finalize",
                Instant::now(),
                world.lock().await,
            );
            let finalized = storage.finalize_dirty_flush(synced);
            let flushed = if barrier_snapshot {
                finalized.installed_chunks()
            } else {
                finalized.cleaned_chunks()
            };
            report.chunks_flushed = report.chunks_flushed.saturating_add(flushed);
            let storage_after = storage.stats();
            let has_flushable_dirty = storage.has_flushable_dirty_chunks();
            info!(
                attempt,
                flushed,
                planned = planned_chunks,
                flush_us = elapsed_us(flush_started),
                chunk_cache_len = storage_after.chunk_cache_len,
                chunk_cache_capacity = storage_after.chunk_cache_capacity,
                region_cache_len = storage_after.region_cache_len,
                region_cache_capacity = storage_after.region_cache_capacity,
                dirty_before = storage_before.dirty_chunks,
                dirty_after = storage_after.dirty_chunks,
                %context,
                "world storage save pressure"
            );
            report.timings.flush_commit_us = report
                .timings
                .flush_commit_us
                .saturating_add(elapsed_us(started));
            (storage_after.dirty_chunks, has_flushable_dirty)
        };

        if remaining_dirty == 0 {
            world_flush_clean = true;
            break;
        }
        if !require_clean_dirty_flush {
            break;
        }
        if !has_flushable_dirty {
            report.errors.push(format!(
                "dirty chunks: final flush found {remaining_dirty} journal-pending chunks after producer drain"
            ));
            break;
        }
        info!(
            attempt,
            dirty = remaining_dirty,
            %context,
            "final dirty flush retrying changed chunks"
        );
    }

    if world_flush_clean && let Some(watermark) = world_chunk_journal_watermark {
        let checkpoint = sessions.world_chunk_journal().map(|journal| {
            tokio::task::spawn_blocking(move || journal.checkpoint_through(watermark))
        });
        if let Some(checkpoint) = checkpoint {
            match checkpoint.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    sessions.report_world_chunk_journal_failure();
                    report
                        .errors
                        .push(format!("world chunk journal checkpoint failed: {error}"));
                }
                Err(error) => {
                    sessions.report_world_chunk_journal_failure();
                    report.errors.push(format!(
                        "world chunk journal checkpoint worker failed: {error}"
                    ));
                }
            }
        }
    }

    let started = Instant::now();
    let (players_saved, acknowledged_players, player_errors) =
        save_player_states_blocking(root.clone(), Arc::clone(&config.items), players).await;
    report.players_saved = players_saved;
    report.errors.extend(player_errors);
    sessions.acknowledge_saved_player_states(&acknowledged_players);
    report.timings.players_us = elapsed_us(started);

    let started = Instant::now();
    let entity_count = entities.records.len();
    match save_entities_blocking(root.clone(), Arc::clone(&config.items), entities).await {
        Ok(()) => {
            report.entities_saved = entity_count;
            if let Err(error) = sessions.clear_recovered_entity_commits(&entity_journal_phases) {
                report
                    .errors
                    .push(format!("entity journal checkpoint failed: {error:?}"));
            }
        }
        Err(err) => report.errors.push(format!("entities: save failed: {err}")),
    }
    report.timings.entities_us = elapsed_us(started);

    let started = Instant::now();
    let metadata = play::persistence::WorldPersistedMetadata {
        world_time,
        daylight_cycle_enabled,
        players_sleeping_percentage,
        keep_inventory,
        world_identity: play::persistence::world_identity(&root),
    };
    match save_world_metadata_blocking(root.clone(), metadata).await {
        Ok(()) => report.world_metadata_saved = true,
        Err(err) => report
            .errors
            .push(format!("world metadata: save failed: {err}")),
    }
    report.timings.metadata_us = elapsed_us(started);
    report.timings.total_us = elapsed_us(total_started);

    report
}

async fn save_player_states_blocking(
    root: std::path::PathBuf,
    items: Arc<ItemRegistry>,
    players: Vec<(
        uuid::Uuid,
        play::persistence::PlayerPersistedState,
        Option<u64>,
    )>,
) -> (usize, Vec<(uuid::Uuid, u64)>, Vec<String>) {
    match tokio::task::spawn_blocking(move || {
        let mut saved = 0usize;
        let mut acknowledged = Vec::new();
        let mut errors = Vec::new();
        for (uuid, player, disconnected_generation) in players {
            match play::persistence::save_player_state(&root, uuid, &items, &player) {
                Ok(()) => {
                    saved += 1;
                    if let Some(generation) = disconnected_generation {
                        acknowledged.push((uuid, generation));
                    }
                }
                Err(err) => errors.push(format!("player {uuid}: save failed: {err}")),
            }
        }
        (saved, acknowledged, errors)
    })
    .await
    {
        Ok(result) => result,
        Err(err) => (
            0,
            Vec::new(),
            vec![format!("players: save worker failed: {err}")],
        ),
    }
}

async fn save_entities_blocking(
    root: std::path::PathBuf,
    items: Arc<ItemRegistry>,
    entities: play::persistence::PersistedEntityCheckpoint,
) -> Result<(), String> {
    crate::blocking::spawn_result_blocking(move || {
        play::persistence::save_persisted_entity_records(&root, &items, &entities)
    })
    .await
}

async fn save_world_metadata_blocking(
    root: std::path::PathBuf,
    metadata: play::persistence::WorldPersistedMetadata,
) -> Result<(), String> {
    crate::blocking::spawn_result_blocking(move || {
        play::persistence::save_world_metadata(&root, &metadata)
    })
    .await
}

async fn serve_then_final_save(bound: BoundServer) -> std::io::Result<()> {
    let save = bound.save_handle();
    let serve_result = bound.serve().await;
    finish_serve_with_final_save(serve_result, save.save_all_after_drain()).await
}

async fn finish_serve_with_final_save<S>(
    serve_result: std::io::Result<()>,
    save: S,
) -> std::io::Result<()>
where
    S: Future<Output = SaveAllReport>,
{
    let serve_error = match serve_result {
        Ok(()) => None,
        Err(error) if is_uncertain_runtime_serve_error(&error) => return Err(error),
        Err(error) => Some(error),
    };
    let report = save.await;
    log_save_report("server run final save", &report);
    if let Some(error) = serve_error {
        Err(error)
    } else if report.is_ok() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "final save failed with {} error(s)",
            report.errors.len()
        )))
    }
}

/// Convenience for the binary: bind, drain, then perform one final save.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    bind(config).await?.serve_and_save().await
}

#[cfg(test)]
#[path = "server_collision_tests.rs"]
mod server_collision_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_script::{
        LuaHostConfig, ScriptEventKind, ScriptGameMode, ScriptPlayerContext, ScriptPlayerId,
        script_boundary_pair, start_lua_host,
    };
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::num::NonZeroUsize;
    use std::sync::atomic::AtomicUsize;
    use std::task::Poll;
    use tokio::sync::mpsc;

    type StateSpec<'a> = (u32, bool, &'a [(&'a str, &'a str)]);

    fn canonical_entity_types() -> Arc<EntityTypeRegistry> {
        Arc::new(mc_data::entity_types::solaris_required_entity_types())
    }

    #[test]
    fn connection_task_limit_scales_with_players_and_stays_bounded() {
        assert_eq!(connection_task_limit(0), MIN_CONNECTION_TASKS);
        assert_eq!(connection_task_limit(8), MIN_CONNECTION_TASKS);
        assert_eq!(connection_task_limit(20), 56);
        assert_eq!(connection_task_limit(128), 272);
        assert_eq!(connection_task_limit(512), MAX_CONNECTION_TASKS);
        assert_eq!(connection_task_limit(u32::MAX), MAX_CONNECTION_TASKS);
    }

    #[test]
    fn pre_auth_connection_limit_is_smaller_and_bounded() {
        assert_eq!(pre_auth_connection_limit(0), MIN_PRE_AUTH_CONNECTIONS);
        assert_eq!(pre_auth_connection_limit(8), MIN_PRE_AUTH_CONNECTIONS);
        assert_eq!(pre_auth_connection_limit(20), 28);
        assert_eq!(pre_auth_connection_limit(128), MAX_PRE_AUTH_CONNECTIONS);
        assert_eq!(
            pre_auth_connection_limit(u32::MAX),
            MAX_PRE_AUTH_CONNECTIONS
        );
        for max_players in [0, 1, 8, 20, 128, 512, u32::MAX] {
            assert!(pre_auth_connection_limit(max_players) <= connection_task_limit(max_players));
        }
    }

    #[test]
    fn script_sink_routes_known_player_commands_with_exact_payload() {
        let (boundary, mut endpoint) =
            script_boundary_pair(NonZeroUsize::new(2).unwrap(), NonZeroUsize::new(1).unwrap());
        let manifest = mc_script::ScriptPluginManifest::new(
            "greetings",
            "Greetings",
            "0.1.0",
            mc_script::SCRIPT_API_VERSION,
        )
        .declare_player_command_root("hello")
        .validate()
        .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();
        let sink = ScriptEventSink::new(boundary);

        assert_eq!(
            sink.enqueue_player_command_with_operator(7, "Alex", "missing arg", false),
            mc_script::PlayerCommandAdmission::NotOwned
        );
        assert_eq!(
            sink.enqueue_player_command_with_operator(7, "Alex", "hello one  two ", false),
            mc_script::PlayerCommandAdmission::Enqueued
        );
        let event = endpoint.recv_event_blocking().unwrap();
        assert_eq!(event.target_plugin_id(), Some("greetings"));
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerCommand {
                player_id,
                username,
                root,
                arguments,
                ..
            } if *player_id == ScriptPlayerId::new(7)
                && username == "Alex"
                && root == "hello"
                && arguments == "one  two "
        ));
    }

    #[test]
    fn script_sink_reports_queue_full_as_dropped_and_closed_as_unavailable() {
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
        let manifest = mc_script::ScriptPluginManifest::new(
            "greetings",
            "Greetings",
            "0.1.0",
            mc_script::SCRIPT_API_VERSION,
        )
        .declare_player_command_root("hello")
        .validate()
        .unwrap();
        endpoint.register_player_commands(&manifest).unwrap();
        let sink = ScriptEventSink::new(boundary);
        sink.enqueue_event(ScriptEvent::server_started());

        assert_eq!(
            sink.enqueue_player_command_with_operator(7, "Alex", "hello full", false),
            mc_script::PlayerCommandAdmission::Dropped
        );

        drop(endpoint);
        assert_eq!(
            sink.enqueue_player_command_with_operator(7, "Alex", "hello closed", false),
            mc_script::PlayerCommandAdmission::NotOwned
        );
        assert!(sink.player_command_roots().is_empty());
    }

    #[tokio::test]
    async fn committed_script_event_worker_waits_for_exact_queue_capacity_notification() {
        let one = NonZeroUsize::new(1).unwrap();
        let (boundary, mut endpoint) = script_boundary_pair(one, one);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let sink = ScriptEventSink::new(boundary);
        let sessions = play::SessionRegistry::new();
        let receiver = sessions.install_script_commit_event_outbox();
        sessions
            .try_enqueue_script_commit_event_for_test(
                play::ScriptCommitDelivery::Required,
                ScriptEvent::try_player_died_with_context(
                    ScriptPlayerId::new(7),
                    ScriptPlayerContext::new(
                        "123e4567-e89b-12d3-a456-426614174000",
                        "Alex",
                        false,
                        1.5,
                        64.0,
                        -2.5,
                    ),
                    "minecraft:overworld",
                    ScriptGameMode::Survival,
                )
                .unwrap(),
            )
            .unwrap();
        sessions.close_script_commit_event_outbox();
        let worker = tokio::spawn(forward_committed_script_events(receiver, sink));

        assert!(matches!(
            endpoint.recv_event().await.unwrap().kind(),
            ScriptEventKind::ServerStarted
        ));
        assert!(matches!(
            endpoint.recv_event().await.unwrap().kind(),
            ScriptEventKind::PlayerDied { player_id, .. }
                if *player_id == ScriptPlayerId::new(7)
        ));
        worker.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_required_script_sink_times_out_and_fails_remaining_backlog() {
        let one = NonZeroUsize::new(1).unwrap();
        let (boundary, _endpoint) = script_boundary_pair(one, one);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let sink = ScriptEventSink::new(boundary);
        let sessions = play::SessionRegistry::new();
        let receiver = sessions.install_script_commit_event_outbox();
        for tick in 1..=2 {
            sessions
                .try_enqueue_script_commit_event_for_test(
                    play::ScriptCommitDelivery::Required,
                    ScriptEvent::server_tick(tick),
                )
                .unwrap();
        }
        sessions.close_script_commit_event_outbox();
        let mut failure = sessions.subscribe_script_commit_event_failure();
        let worker = tokio::spawn(forward_committed_script_events(receiver, sink));
        tokio::task::yield_now().await;

        tokio::time::advance(SCRIPT_COMMIT_FORWARD_TIMEOUT + Duration::from_millis(1)).await;

        assert!(matches!(
            worker.await.unwrap(),
            Err(ScriptCommitForwardError::RequiredTimeout { timeout })
                if timeout == SCRIPT_COMMIT_FORWARD_TIMEOUT
        ));
        assert!(*failure.borrow_and_update());
        let snapshot = sessions.script_commit_event_outbox_snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.dequeued, 1);
        assert_eq!(snapshot.abandoned_on_receiver_drop, 1);
        assert_eq!(snapshot.required_abandoned_on_receiver_drop, 1);
    }

    #[tokio::test]
    async fn required_committed_script_event_overflow_requests_shutdown() {
        let sessions = play::SessionRegistry::new();
        let _receiver = sessions.install_script_commit_event_outbox();
        let capacity = sessions.script_commit_event_outbox_snapshot().capacity;
        let shutdown = ShutdownHandle::default();
        let watcher = tokio::spawn(watch_script_commit_event_failure(
            sessions.subscribe_script_commit_event_failure(),
            shutdown.clone(),
        ));

        for tick in 0..capacity {
            sessions
                .try_enqueue_script_commit_event_for_test(
                    play::ScriptCommitDelivery::Required,
                    ScriptEvent::server_tick(tick as u64),
                )
                .unwrap();
        }
        assert!(
            sessions
                .try_enqueue_script_commit_event_for_test(
                    play::ScriptCommitDelivery::Required,
                    ScriptEvent::server_stopping("required overflow"),
                )
                .is_err()
        );

        tokio::time::timeout(Duration::from_secs(1), shutdown.wait_requested())
            .await
            .expect("required outbox failure did not request shutdown");
        let snapshot = sessions.script_commit_event_outbox_snapshot();
        assert_eq!(snapshot.depth, capacity);
        assert_eq!(snapshot.max_depth, capacity);
        assert_eq!(snapshot.required_overflow, 1);
        watcher.await.unwrap();
    }

    #[tokio::test]
    async fn best_effort_committed_script_event_drop_is_counted_without_failure() {
        let one = NonZeroUsize::new(1).unwrap();
        let (boundary, _endpoint) = script_boundary_pair(one, one);
        boundary
            .try_enqueue_event(ScriptEvent::server_started())
            .unwrap();
        let sink = ScriptEventSink::new(boundary);
        let sessions = play::SessionRegistry::new();
        let receiver = sessions.install_script_commit_event_outbox();
        sessions
            .try_enqueue_script_commit_event_for_test(
                play::ScriptCommitDelivery::BestEffort,
                ScriptEvent::server_tick(1),
            )
            .unwrap();
        sessions.close_script_commit_event_outbox();

        forward_committed_script_events(receiver, sink)
            .await
            .unwrap();

        let snapshot = sessions.script_commit_event_outbox_snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.best_effort_sink_dropped, 1);
        assert_eq!(snapshot.required_overflow, 0);
        assert_eq!(snapshot.required_closed, 0);
        assert!(!*sessions.subscribe_script_commit_event_failure().borrow());
    }

    #[tokio::test]
    async fn shutdown_wait_wakes_when_shutdown_is_requested() {
        let shutdown = ShutdownHandle::default();
        let mut waiter = Box::pin(shutdown.wait_requested());

        std::future::poll_fn(|context| match waiter.as_mut().poll(context) {
            Poll::Pending => Poll::Ready(()),
            Poll::Ready(()) => panic!("shutdown wait completed before the request"),
        })
        .await;
        shutdown.request();

        tokio::time::timeout(Duration::from_secs(1), waiter.as_mut())
            .await
            .expect("shutdown waiter did not wake");
    }

    #[tokio::test]
    async fn shutdown_wait_observes_request_made_before_registration() {
        let shutdown = ShutdownHandle::default();
        shutdown.request();

        tokio::time::timeout(Duration::from_secs(1), shutdown.wait_requested())
            .await
            .expect("pre-requested shutdown wait did not complete");
    }

    #[tokio::test]
    async fn script_spawn_rejects_unknown_registry_identifier_without_queueing() {
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "test".to_owned(),
            max_players: 1,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks: Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            world: None,
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(LootTables::default()),
            block_light: None,
            items: Arc::new(ItemRegistry::default()),
            item_facts: Arc::new(ItemFactsTable::default()),
            block_facts: Arc::new(BlockFactsTable::default()),
            entity_types: canonical_entity_types(),
            biome_spawns: Arc::new(BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        };
        let (simulation, _owner) = play::simulation_channel();
        assert_eq!(
            resolve_script_entity_type(&config, "minecraft:missing"),
            None
        );
        assert_eq!(simulation.snapshot().depth, 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn console_time_set_waits_for_server_owned_simulation_turn() {
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "test".to_owned(),
            max_players: 1,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks: Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            world: None,
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(LootTables::default()),
            block_light: None,
            items: Arc::new(ItemRegistry::default()),
            item_facts: Arc::new(ItemFactsTable::default()),
            block_facts: Arc::new(BlockFactsTable::default()),
            entity_types: canonical_entity_types(),
            biome_spawns: Arc::new(BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        };
        let sessions = play::SessionRegistry::new();
        let chunk_pipeline_resources = ChunkPipelineResources::with_limits(1, 1);
        let (simulation, mut owner) = play::simulation_channel();
        let mut command = Box::pin(execute_console_command(
            "time set night",
            "test save",
            "test stop",
            &config,
            &sessions,
            None,
            &simulation,
            &chunk_pipeline_resources,
        ));

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(command.as_mut(), cx).is_pending(),
                "console time set must wait for its owner response"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(sessions.world_time(), 0);
        assert_eq!(simulation.snapshot().depth, 1);

        assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
        assert!(!command.await);
        assert_eq!(sessions.world_time(), 13_000);
    }

    #[test]
    fn stopped_entity_ticker_requests_server_shutdown() {
        let shutdown = ShutdownHandle::default();

        let error =
            handle_entity_ticker_exit(&shutdown, Ok(())).expect_err("unexpected stop fails");

        assert!(shutdown.is_requested());
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    }

    #[test]
    fn runtime_metrics_policy_normalizes_log_interval() {
        let policy = RuntimeMetricsPolicy {
            log_interval_ticks: 0,
            slow_tick_ms: 0,
        }
        .normalized();

        assert_eq!(policy.log_interval_ticks, 1);
        assert_eq!(policy.slow_tick_ms, 0);
    }

    #[test]
    fn simulation_command_window_marks_off_tick_scope() {
        let mut window = SimulationCommandTelemetryWindow::default();

        window.record_off_tick(41, 2);
        let telemetry = window.finish_tick(1, 0);

        assert_eq!(telemetry.elapsed_us, 42);
        assert_eq!(telemetry.processed, 2);
        assert_eq!(
            telemetry.scope,
            SimulationCommandTelemetryScope::SincePreviousTickBoundary
        );
        assert_eq!(telemetry.scope.as_str(), "since_previous_tick_boundary");
    }

    #[test]
    fn simulation_command_gate_bounds_off_tick_work_between_ticks() {
        let mut gate = SimulationCommandGate::default();

        assert!(gate.accepts_off_tick_batch());
        gate.record_off_tick_batch();
        assert!(!gate.accepts_off_tick_batch());
        gate.record_tick_boundary();
        assert!(gate.accepts_off_tick_batch());
    }

    #[test]
    fn extension_outbound_custom_payload_obeys_channel_allowlist() {
        let (boundary, _endpoint) = mc_extension::boundary_pair(
            std::num::NonZeroUsize::new(4).unwrap(),
            std::num::NonZeroUsize::new(4).unwrap(),
        );
        let extension = ExtensionEventSink::new(
            boundary,
            CustomPayloadPolicy::new(64, ["solaris:allowed".to_owned()]),
        );
        assert!(
            validate_extension_custom_payload_command(
                &extension,
                PlayerId::new(91),
                "solaris:denied".to_owned(),
                bytes::Bytes::from_static(b"denied"),
            )
            .is_none()
        );

        let (channel, payload) = validate_extension_custom_payload_command(
            &extension,
            PlayerId::new(91),
            "solaris:allowed".to_owned(),
            bytes::Bytes::from_static(b"allowed"),
        )
        .expect("allowlisted payload");
        assert_eq!(channel.as_str(), "solaris:allowed");
        assert_eq!(payload, b"allowed");
    }

    #[tokio::test]
    async fn extension_command_task_stops_without_draining_after_shutdown() {
        let (boundary, endpoint) = mc_extension::boundary_pair(
            std::num::NonZeroUsize::new(4).unwrap(),
            std::num::NonZeroUsize::new(4).unwrap(),
        );
        let extension = ExtensionEventSink::new(
            boundary,
            CustomPayloadPolicy::new(64, ["solaris:allowed".to_owned()]),
        );
        endpoint
            .try_submit_command(ExtensionOutboundCommand::DisconnectPlayer {
                player_id: PlayerId::new(91),
                reason: "queued before shutdown".to_owned(),
            })
            .unwrap();
        let shutdown = ShutdownHandle::default();
        shutdown.request();

        run_extension_commands(
            extension.clone(),
            Arc::new(play::SessionRegistry::new()),
            shutdown,
        )
        .await;

        assert!(matches!(
            extension.try_recv_command(),
            Ok(ExtensionOutboundCommand::DisconnectPlayer { player_id, .. })
                if player_id == PlayerId::new(91)
        ));
    }

    #[tokio::test]
    async fn script_command_task_drains_buffered_command_before_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = Arc::new(save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(ItemRegistry::default()),
            canonical_entity_types(),
        ));
        let (boundary, endpoint) =
            script_boundary_pair(NonZeroUsize::new(4).unwrap(), NonZeroUsize::new(4).unwrap());
        let scripts = ScriptEventSink::new(boundary);
        endpoint
            .try_submit_command(ScriptCommand::BroadcastChatMessage {
                message: "accepted before shutdown".to_owned(),
            })
            .unwrap();
        drop(endpoint);
        let shutdown = config.shutdown.clone();
        shutdown.request();
        let (simulation, _owner) = play::simulation_channel();

        run_script_commands(ScriptCommandTask {
            scripts: scripts.clone(),
            zones: PluginZoneAdapter::new(scripts.clone()),
            storage: None,
            config,
            sessions: Arc::new(play::SessionRegistry::new()),
            runtime_control: None,
            simulation,
            chunk_pipeline_resources: ChunkPipelineResources::with_limits(1, 1),
            shutdown,
        })
        .await;

        assert!(
            scripts.recv_command().await.is_none(),
            "buffered script command must be consumed before the shutdown fence"
        );
    }

    #[tokio::test]
    async fn script_stop_drains_later_commands_from_the_same_host_batch() {
        let plugins = tempfile::tempdir().unwrap();
        let plugin = plugins.path().join("stop-batch");
        std::fs::create_dir(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.toml"),
            r#"id = "stop-batch"
name = "Stop Batch"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "server.stopping"]
console_commands = ["stop"]
"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("main.lua"),
            r#"function on_server_started(_event)
    solaris.run_console("stop")
end

function on_server_stopping(_event)
    solaris.broadcast("accepted after stop")
end
"#,
        )
        .unwrap();

        let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
        assert_eq!(host.loaded_plugins(), 1);
        let shutdown_boundary = boundary.clone();
        let scripts = ScriptEventSink::new(boundary);
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = Arc::new(save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(ItemRegistry::default()),
            canonical_entity_types(),
        ));
        let sessions = Arc::new(play::SessionRegistry::new());
        let shutdown = config.shutdown.clone();
        let (simulation, _owner) = play::simulation_channel();

        scripts.enqueue_event(ScriptEvent::server_started());
        let command_task = tokio::spawn(run_script_commands(ScriptCommandTask {
            scripts: scripts.clone(),
            zones: PluginZoneAdapter::new(scripts.clone()),
            storage: None,
            config,
            sessions,
            runtime_control: None,
            simulation,
            chunk_pipeline_resources: ChunkPipelineResources::with_limits(1, 1),
            shutdown: shutdown.clone(),
        }));

        shutdown.wait_requested().await;
        scripts.enqueue_event(ScriptEvent::server_stopping("server stopping"));
        shutdown_boundary.close_event_admission();
        tokio::task::spawn_blocking(move || host.join())
            .await
            .unwrap()
            .unwrap();
        command_task.await.unwrap();

        let leftover = scripts.recv_command().await;
        assert!(
            leftover.is_none(),
            "script stop must drain commands emitted by the accepted stopping event"
        );
        drop(scripts);
    }

    #[test]
    fn loaded_block_tick_hint_only_wakes_for_due_loaded_chunk() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let scheduled_ticks = world.scheduled_tick_view();
        let cpos = mc_world::ChunkPos { x: 2, z: 3 };
        let pos = mc_world::BlockPos {
            x: 2 * 16 + 1,
            y: 2,
            z: 3 * 16 + 1,
        };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(cpos, mc_world::BlockStateId(0), biome),
            )
            .unwrap();
        world
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                pos,
                mc_data::Identifier::parse("minecraft:wheat").unwrap(),
                10,
                0,
            ))
            .unwrap();

        assert!(!loaded_block_tick_due(&scheduled_ticks, &[(2, 3)], 9));
        assert!(!loaded_block_tick_due(&scheduled_ticks, &[(0, 0)], 10));
        assert!(loaded_block_tick_due(&scheduled_ticks, &[(2, 3)], 10));
    }

    #[test]
    fn loaded_fluid_tick_hint_only_wakes_for_due_loaded_chunk() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let scheduled_ticks = world.scheduled_tick_view();
        let cpos = mc_world::ChunkPos { x: 2, z: 3 };
        let pos = mc_world::BlockPos {
            x: 2 * 16 + 1,
            y: 2,
            z: 3 * 16 + 1,
        };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(
                cpos,
                mc_world::Chunk::empty(cpos, mc_world::BlockStateId(0), biome),
            )
            .unwrap();
        world
            .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
                pos,
                mc_data::Identifier::parse("minecraft:water").unwrap(),
                10,
                0,
            ))
            .unwrap();

        assert!(!loaded_fluid_tick_due(&scheduled_ticks, &[(2, 3)], 9));
        assert!(!loaded_fluid_tick_due(&scheduled_ticks, &[(0, 0)], 10));
        assert!(loaded_fluid_tick_due(&scheduled_ticks, &[(2, 3)], 10));
    }

    #[test]
    fn runtime_metrics_logging_respects_interval_and_slow_budget() {
        let policy = RuntimeMetricsPolicy {
            log_interval_ticks: 5,
            slow_tick_ms: 50,
        };

        let mut gate = RuntimeMetricsLogGate::default();
        assert!(gate.should_log(10, 1, policy));
        assert!(gate.should_log(11, 50_000, policy));
        assert!(!gate.should_log(12, 50_001, policy));
        assert!(gate.should_log(15, 50_001, policy));
        assert!(!gate.should_log(16, 49_999, policy));
        assert!(gate.should_log(17, 50_000, policy));
    }

    #[test]
    fn entity_physics_uses_shared_cpu_worker_capacity() {
        let resources = ChunkPipelineResources::with_limits(1, 3);

        assert_eq!(entity_physics_worker_count(&resources, 0), 0);
        assert_eq!(entity_physics_worker_count(&resources, 2), 1);
        assert_eq!(entity_physics_worker_count(&resources, 512), 2);

        resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false);
        assert_eq!(entity_physics_worker_count(&resources, 768), 2);
    }

    #[test]
    fn entity_physics_batches_small_jobs_before_parallelizing() {
        let resources = ChunkPipelineResources::with_limits(1, 16);

        assert_eq!(entity_physics_worker_count(&resources, 7), 1);
        assert_eq!(entity_physics_worker_count(&resources, 8), 1);
        assert_eq!(entity_physics_worker_count(&resources, 24), 1);
        assert_eq!(entity_physics_worker_count(&resources, 64), 1);
        assert_eq!(entity_physics_worker_count(&resources, 256), 1);
        assert_eq!(entity_physics_worker_count(&resources, 257), 2);
        assert_eq!(entity_physics_worker_count(&resources, 768), 3);
    }

    #[tokio::test]
    async fn entity_physics_keeps_a_common_herd_batch_inline() {
        let resources = ChunkPipelineResources::with_limits(1, 16);
        let snapshot = Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::new(),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
        });
        let inputs = (0..198)
            .map(|id| EntityPhysicsInput {
                query: play::EntityPhysicsQuery {
                    id: mc_entity::EntityId(id),
                    position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
                    velocity: mc_entity::Vec3::ZERO,
                    aabb: mc_physics::Aabb::COW,
                    on_ground: false,
                    fall_distance: 0.0,
                    kind: play::EntityPhysicsKind::Default,
                },
                snapshot: Arc::clone(&snapshot),
                complete_samples: false,
            })
            .collect();

        let steps = step_entity_physics_inputs(resources.clone(), inputs).await;

        assert_eq!(steps.len(), 198);
        assert_eq!(resources.metrics().snapshot().max_cpu_active, 0);
    }

    #[tokio::test]
    async fn entity_physics_does_not_wait_for_busy_chunk_workers() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let _busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Default,
        };
        let inputs = vec![EntityPhysicsInput {
            query,
            snapshot: Arc::new(EntityPhysicsSnapshot {
                chunks: HashMap::new(),
                materials: Arc::new(BlockMaterialIds::new(0, None, None)),
                blocks: None,
            }),
            complete_samples: false,
        }];

        let steps = tokio::time::timeout(
            Duration::from_secs(1),
            step_entity_physics_inputs(resources, inputs),
        )
        .await
        .expect("entity physics waited for a chunk worker");

        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, query.id);
        assert_eq!(steps[0].position, query.position);
    }

    #[tokio::test]
    async fn large_entity_physics_waits_for_cpu_push_without_blocking_owner_work() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
        let snapshot = Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::new(),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
        });
        let inputs = (0..257)
            .map(|id| EntityPhysicsInput {
                query: play::EntityPhysicsQuery {
                    id: mc_entity::EntityId(id),
                    position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
                    velocity: mc_entity::Vec3::ZERO,
                    aabb: mc_physics::Aabb::COW,
                    on_ground: false,
                    fall_distance: 0.0,
                    kind: play::EntityPhysicsKind::Default,
                },
                snapshot: Arc::clone(&snapshot),
                complete_samples: false,
            })
            .collect();
        let mut physics = std::pin::pin!(step_entity_physics_inputs(resources.clone(), inputs,));

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(physics.as_mut(), cx).is_pending(),
                "large physics batch ran inline while shared CPU was busy"
            );
            std::task::Poll::Ready(())
        })
        .await;

        let (owner_work_tx, owner_work_rx) = tokio::sync::oneshot::channel();
        owner_work_tx.send(()).unwrap();
        tokio::select! {
            biased;
            steps = &mut physics => panic!(
                "large physics completed before CPU release: {} steps",
                steps.len()
            ),
            result = owner_work_rx => result.unwrap(),
        }

        drop(busy_worker);
        let steps = physics.await;
        assert_eq!(steps.len(), 257);
        assert_eq!(resources.metrics().snapshot().max_cpu_active, 1);
    }

    #[tokio::test]
    async fn background_entity_physics_leaves_simulation_owner_responsive() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
        let snapshot = Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::new(),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
        });
        let queries = (0..257)
            .map(|id| play::EntityPhysicsQuery {
                id: mc_entity::EntityId(id),
                position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
                velocity: mc_entity::Vec3::ZERO,
                aabb: mc_physics::Aabb::COW,
                on_ground: false,
                fall_distance: 0.0,
                kind: play::EntityPhysicsKind::Default,
            })
            .collect::<Vec<_>>();
        let inputs = queries
            .iter()
            .copied()
            .map(|query| EntityPhysicsInput {
                query,
                snapshot: Arc::clone(&snapshot),
                complete_samples: false,
            })
            .collect();
        let mut physics = spawn_entity_physics_job(9, queries, resources, inputs);
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(std::pin::Pin::new(&mut physics), cx).is_pending(),
                "background physics completed while CPU admission was occupied"
            );
            std::task::Poll::Ready(())
        })
        .await;

        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let barrier = tokio::spawn(async move { simulation.save_barrier(false).await });
        assert!(owner.wait_for_command().await);
        assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
        assert!(barrier.await.unwrap().is_ok());

        drop(busy_worker);
        let completed = physics.await.unwrap();
        assert_eq!(completed.tick, 9);
        assert_eq!(completed.expected.len(), 257);
        assert_eq!(completed.steps.len(), 257);
    }

    #[tokio::test]
    async fn background_scheduled_blocks_leave_simulation_owner_responsive() {
        let reports = [
            report("minecraft:air", &[], &[(0, true, &[])]),
            report(
                "minecraft:stone_button",
                &[
                    ("face", &["wall"]),
                    ("facing", &["east"]),
                    ("powered", &["false", "true"]),
                ],
                &[
                    (
                        1,
                        true,
                        &[("face", "wall"), ("facing", "east"), ("powered", "false")],
                    ),
                    (
                        2,
                        false,
                        &[("face", "wall"), ("facing", "east"), ("powered", "true")],
                    ),
                ],
            ),
        ];
        let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut storage = WorldStorage::in_memory(Arc::clone(&blocks));
        storage
            .insert_generated_chunk(
                chunk_position,
                mc_world::Chunk::empty(
                    chunk_position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut positions = (1..=14)
            .flat_map(|z| (1..=14).map(move |x| mc_world::BlockPos { x, y: 64, z }))
            .collect::<Vec<_>>();
        positions.extend(
            (1..=6).flat_map(|z| (1..=10).map(move |x| mc_world::BlockPos { x, y: 65, z })),
        );
        assert_eq!(positions.len(), 256);
        for position in positions {
            storage
                .set_block_at(position, mc_world::BlockStateId(2))
                .unwrap();
            storage
                .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                    position,
                    Identifier::parse("minecraft:stone_button").unwrap(),
                    9,
                    0,
                ))
                .unwrap();
        }
        let world_read = storage.read_view();
        let world_mutation = storage.mutation_view();
        let world = Arc::new(Mutex::new(storage));
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("dimensions/minecraft/overworld/region")).unwrap();
        let mut config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::new(ItemRegistry::default()),
            canonical_entity_types(),
        );
        config.world = Some(Arc::clone(&world));
        let config = Arc::new(config);
        let sessions = Arc::new(play::SessionRegistry::new());
        sessions.register_loaded_for_server_test("ScheduledWorker", (0, 0));

        let resources = ChunkPipelineResources::with_limits(1, 1);
        let admission = sessions
            .try_begin_scheduled_block_ticks()
            .expect("first scheduled block admission succeeds");
        let duplicate = play::run_scheduled_block_ticks_background(
            &config,
            &sessions,
            play::SimulationWorldAccess {
                read: Some(&world_read),
                mutation: Some(&world_mutation),
                cpu: Some(&resources),
                light: config.block_light.as_ref(),
            },
            None,
            9,
            256,
        )
        .await;
        assert_eq!(duplicate.drained, 0);
        assert_eq!(duplicate.applied, 0);
        drop(admission);

        let busy_worker = resources.acquire_cpu().await.expect("reserve CPU worker");
        let block_tick = spawn_scheduled_block_tick_job(
            9,
            256,
            Arc::clone(&config),
            Arc::clone(&sessions),
            Some(world_read.clone()),
            Some(world_mutation.clone()),
            None,
            resources.clone(),
        );

        let (simulation, mut owner) = play::simulation_channel();
        let mut barrier = tokio::spawn(async move { simulation.save_barrier(false).await });
        let mut waiting = Box::pin(await_scheduled_block_tick_job_with_commands(
            block_tick,
            &mut owner,
            &config,
            &sessions,
            Some(&world_read),
            Some(&world_mutation),
            &resources,
        ));
        let barrier_result = tokio::select! {
            biased;
            result = &mut waiting => panic!(
                "scheduled block batch completed before CPU release: {:?}",
                result.0.map(|completed| completed.report)
            ),
            result = &mut barrier => result.unwrap(),
        };
        assert!(barrier_result.is_ok());

        drop(busy_worker);
        let (completed, commands) = waiting.await;
        let completed = completed.unwrap();
        eprintln!(
            "scheduled block background batch: drained={} applied={} elapsed_us={}",
            completed.report.drained, completed.report.applied, completed.elapsed_us
        );
        assert_eq!(commands.report.processed, 1);
        assert_eq!(commands.report.remaining_depth, 0);
        assert_eq!(completed.tick, 9);
        assert_eq!(completed.report.drained, 256);
        assert_eq!(completed.report.applied, 256);
        assert_eq!(
            world.lock().await.get_cached_block(position),
            Some(mc_world::BlockStateId(1))
        );
    }

    #[test]
    fn entity_physics_snapshot_becomes_stale_after_world_edit() {
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let mut world = WorldStorage::in_memory(blocks);
        let chunk = mc_world::ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                chunk,
                mc_world::Chunk::empty(
                    chunk,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let world_read = world.read_view();
        let captured = world_read.snapshot_chunks(&[chunk]);
        let snapshot = EntityPhysicsSnapshot {
            chunks: HashMap::from([(chunk, captured.chunk(chunk))]),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
        };

        assert!(entity_physics_snapshot_is_current(&world_read, &snapshot));
        world
            .set_block_at(
                mc_world::BlockPos { x: 1, y: 64, z: 1 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
        assert!(!entity_physics_snapshot_is_current(&world_read, &snapshot));
    }

    #[test]
    fn arrow_physics_samples_chunk_boundary_and_wires_block_hit_fact() {
        let reports = mc_data::blocks::solaris_required_blocks_report();
        let air = state_id(&reports, "minecraft:air", &[]);
        let stone = state_id(&reports, "minecraft:stone", &[]);
        let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let materials = material_ids(&blocks, &facts);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::in_memory(blocks);
        for chunk_x in [0, 1] {
            let position = mc_world::ChunkPos { x: chunk_x, z: 0 };
            world
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(position, mc_world::BlockStateId(air), biome.clone()),
                )
                .unwrap();
        }
        world
            .set_block_at(
                mc_world::BlockPos { x: 16, y: 64, z: 8 },
                mc_world::BlockStateId(stone),
            )
            .unwrap();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(71),
            position: mc_entity::Vec3::new(15.5, 64.25, 8.5),
            velocity: mc_entity::Vec3::new(1.0, 0.0, 0.0),
            aabb: mc_physics::Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::ArrowProjectile {
                revision: None,
                embedded_block: None,
            },
        };
        let input = sample_entity_physics_input(query, &mut world, &materials);
        assert!(input.complete_samples);
        assert!(
            input
                .snapshot
                .chunks
                .contains_key(&mc_world::ChunkPos { x: 0, z: 0 })
        );
        assert!(
            input
                .snapshot
                .chunks
                .contains_key(&mc_world::ChunkPos { x: 1, z: 0 })
        );
        let snapshot = Arc::clone(&input.snapshot);
        let step = step_sampled_entity(input);

        let facts = arrow_physics_facts_from_steps(1, &[query], &snapshot, &[step]);

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].arrow_id, query.id);
        let block_hit = facts[0].block_hit.expect("arrow endpoint hits stone");
        assert_eq!(block_hit.block_state, mc_world::BlockStateId(stone));
        assert_eq!(
            block_hit.block_position,
            mc_entity::projectile_26_1_2::BlockPosition::new(16, 64, 8)
        );
        assert_eq!(block_hit.location, step.position);
    }

    #[test]
    fn arrow_endpoint_sampler_uses_exact_block_collision_shape() {
        let reports = mc_data::blocks::solaris_required_blocks_report();
        let air = state_id(&reports, "minecraft:air", &[]);
        let slab = state_id(
            &reports,
            "minecraft:oak_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        );
        let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let materials = material_ids(&blocks, &facts);
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            position,
            mc_world::BlockStateId(air),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(slab));
        let mut world = WorldStorage::in_memory(blocks);
        world.insert_generated_chunk(position, chunk).unwrap();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(72),
            position: mc_entity::Vec3::new(8.5, 64.5, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::ArrowProjectile {
                revision: None,
                embedded_block: Some(mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8)),
            },
        };
        let input = sample_entity_physics_input(query, &mut world, &materials);
        let snapshot = Arc::clone(&input.snapshot);
        let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(&snapshot));

        assert_eq!(
            collision_block_touching_arrow_endpoint(
                &sampler,
                mc_entity::Vec3::new(8.5, 64.5, 8.5),
                query.aabb,
            ),
            Some((
                mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8),
                slab
            ))
        );
        assert_eq!(
            collision_block_touching_arrow_endpoint(
                &sampler,
                mc_entity::Vec3::new(8.5, 64.500_000_002, 8.5),
                query.aabb,
            ),
            None
        );
        let embedded = play::EntityPhysicsStep {
            id: query.id,
            position: mc_entity::Vec3::new(8.5, 64.500_000_002, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        };
        let fact = arrow_physics_facts_from_steps(4, &[query], &snapshot, &[embedded])[0];
        assert!(fact.embedded_in_block);
        assert_eq!(fact.current_block_state, mc_world::BlockStateId(slab));
        assert!(!fact.should_fall);
        assert!(fact.block_hit.is_none());
    }

    #[test]
    fn arrow_environment_sampler_propagates_water_and_support_loss() {
        let reports = mc_data::blocks::solaris_required_blocks_report();
        let air = state_id(&reports, "minecraft:air", &[]);
        let water = state_id(&reports, "minecraft:water", &[]);
        let blocks = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let materials = material_ids(&blocks, &facts);
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            position,
            mc_world::BlockStateId(air),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let _ = chunk.set_block(8, 64, 8, mc_world::BlockStateId(water));
        let mut world = WorldStorage::in_memory(blocks);
        world.insert_generated_chunk(position, chunk).unwrap();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(74),
            position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::ArrowProjectile {
                revision: None,
                embedded_block: Some(mc_entity::projectile_26_1_2::BlockPosition::new(8, 64, 8)),
            },
        };
        let input = sample_entity_physics_input(query, &mut world, &materials);
        let snapshot = Arc::clone(&input.snapshot);
        let step = step_sampled_entity(input);

        let fact = arrow_physics_facts_from_steps(9, &[query], &snapshot, &[step])[0];

        assert!(fact.in_water);
        assert!(fact.in_water_or_rain);
        assert!(!fact.embedded_in_block);
        assert_eq!(fact.current_block_state, mc_world::BlockStateId(water));
        assert!(fact.should_fall);
        for component in [
            fact.fall_velocity_scale.x,
            fact.fall_velocity_scale.y,
            fact.fall_velocity_scale.z,
        ] {
            assert!((0.0..0.2).contains(&component));
        }
    }

    #[test]
    fn stale_arrow_snapshot_fact_is_rejected_after_world_mutation() {
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let mut world = WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk = mc_world::ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                chunk,
                mc_world::Chunk::empty(
                    chunk,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        world
            .set_block_at(
                mc_world::BlockPos { x: 8, y: 64, z: 8 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
        let world_read = world.read_view();
        let captured = world_read.snapshot_chunks(&[chunk]);
        let snapshot = Arc::new(EntityPhysicsSnapshot {
            chunks: HashMap::from([(chunk, captured.chunk(chunk))]),
            materials: Arc::new(BlockMaterialIds::new(0, None, None)),
            blocks: None,
        });
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(73),
            position: mc_entity::Vec3::new(8.5, 64.25, 7.5),
            velocity: mc_entity::Vec3::new(0.0, 0.0, 1.0),
            aabb: mc_physics::Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::ArrowProjectile {
                revision: None,
                embedded_block: None,
            },
        };
        let step = play::EntityPhysicsStep {
            id: query.id,
            position: mc_entity::Vec3::new(8.5, 64.25, 7.75),
            velocity: mc_entity::Vec3::ZERO,
            on_ground: true,
            horizontal_collision: false,
        };
        assert!(
            arrow_physics_facts_from_steps(1, &[query], &snapshot, &[step])[0]
                .block_hit
                .is_some()
        );

        world
            .set_block_at(
                mc_world::BlockPos { x: 8, y: 64, z: 8 },
                mc_world::BlockStateId(0),
            )
            .unwrap();

        assert!(!entity_physics_snapshot_is_current(&world_read, &snapshot));
    }

    #[tokio::test]
    async fn entity_physics_sampling_does_not_wait_for_world_writer() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            tmp.path()
                .join("dimensions")
                .join("minecraft")
                .join("overworld")
                .join("region"),
        )
        .unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        let world = Arc::clone(config.world.as_ref().unwrap());
        let world_read = {
            let mut storage = world.lock().await;
            let cpos = mc_world::ChunkPos { x: 0, z: 0 };
            storage
                .insert_generated_chunk(
                    cpos,
                    mc_world::Chunk::empty(
                        cpos,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage.read_view()
        };
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Default,
        };
        let queries = [query];
        let resources = ChunkPipelineResources::with_limits(1, 16);
        let world_writer = world.lock().await;
        let inputs = prepare_entity_physics_inputs(&config, Some(&world_read), &queries);
        let mut physics = Box::pin(step_entity_physics_inputs(resources, inputs));

        std::future::poll_fn(|cx| match std::future::Future::poll(physics.as_mut(), cx) {
            std::task::Poll::Ready(steps) => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].id, query.id);
                std::task::Poll::Ready(())
            }
            std::task::Poll::Pending => {
                panic!("entity physics sampling waited for the world writer")
            }
        })
        .await;

        drop(world_writer);
    }

    #[test]
    fn runtime_tick_attribution_includes_sheep_grazing() {
        let sample = RuntimeTickSample {
            tick_us: 1_000,
            world_time_us: 0,
            sheep_grazing_us: 123,
            animal_breeding_us: 0,
            hostile_attacks_us: 0,
            entity_goals_us: 0,
            entity_physics_us: 0,
            entity_dispatch_us: 0,
            campfire_tick_us: 0,
            inhabited_time_us: 0,
            entity_save_us: 0,
            random_tick_us: 0,
            block_tick_us: 0,
            fluid_tick_us: 0,
        };

        assert_eq!(runtime_attributed_tick_us(&sample, 7, 11), 141);
    }

    #[test]
    fn runtime_work_input_uses_the_exact_pushed_percentile_window() {
        let mut window = RuntimeTickMetricsWindow::with_capacity(4);
        for _ in 0..3 {
            window.record(RuntimeTickSample {
                tick_us: 10_000,
                world_time_us: 10,
                sheep_grazing_us: 10,
                animal_breeding_us: 10,
                hostile_attacks_us: 10,
                entity_goals_us: 1_000,
                entity_physics_us: 1_000,
                entity_dispatch_us: 1_000,
                campfire_tick_us: 10,
                inhabited_time_us: 10,
                entity_save_us: 10,
                random_tick_us: 500,
                block_tick_us: 100,
                fluid_tick_us: 100,
            });
        }
        let spike = RuntimeTickSample {
            tick_us: 90_000,
            world_time_us: 10,
            sheep_grazing_us: 10,
            animal_breeding_us: 10,
            hostile_attacks_us: 10,
            entity_goals_us: 40_000,
            entity_physics_us: 20_000,
            entity_dispatch_us: 10_000,
            campfire_tick_us: 10,
            inhabited_time_us: 10,
            entity_save_us: 10,
            random_tick_us: 500,
            block_tick_us: 100,
            fluid_tick_us: 100,
        };
        window.record(spike);
        let percentiles = window.snapshot().expect("computed window");

        let input = runtime_work_input(&percentiles, false);

        assert_eq!(input.tick_p95_us, spike.tick_us);
        assert_eq!(input.entity_goals_p95_us, spike.entity_goals_us);
        assert_eq!(input.entity_physics_p95_us, spike.entity_physics_us);
        assert_eq!(input.entity_dispatch_p95_us, spike.entity_dispatch_us);
        assert_eq!(input.random_tick_p95_us, 500);
    }

    #[test]
    fn runtime_control_tick_observe_applies_memory_pressure_snapshot() {
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            },
        );
        let control = crate::RuntimeControlHandle::new_with_memory_pressure(
            crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy {
                    memory_pressure_percent: 50,
                    scale_down_after_ticks: 1,
                    ..crate::AutoscalePolicy::default()
                },
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 8,
                    chunk_send_rate: 16,
                    chunk_load_rate: 32,
                    chunk_generate_rate: 16,
                },
            },
            memory_pressure,
        );
        let input = runtime_control_tick_input(49_001);
        assert_eq!(input.tick_ms, 50);
        assert_eq!(input.memory_used_mb, 0);
        assert_eq!(input.memory_limit_mb, 0);

        let resources = ChunkPipelineResources::with_limits(1, 8);
        let sessions = play::SessionRegistry::new();
        let shutdown = ShutdownHandle::default();
        let decision =
            observe_runtime_control_tick(&control, &resources, &sessions, &shutdown, 49_001)
                .unwrap();
        assert_eq!(decision.pressure, Some(crate::AutoscalePressure::Memory));
        assert_eq!(decision.action, crate::AutoscaleAction::ScaleDown);
        assert_eq!(resources.cpu_limit(), 4);
        assert_eq!(sessions.entity_owner_lane_count(), 4);
        assert_eq!(
            control.snapshot().last_decision.pressure,
            Some(crate::AutoscalePressure::Memory)
        );
        assert_eq!(
            control.memory_pressure_observation().sample,
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            }
        );
    }

    #[test]
    fn runtime_control_applies_only_capacity_changes_and_preserves_special_paths() {
        let resources = ChunkPipelineResources::with_limits(1, 8);
        let sessions = play::SessionRegistry::new_with_entity_owner_lanes(8);
        let limits = crate::RuntimeControlLimits {
            view_distance: 8,
            chunk_send_rate: 16,
            chunk_load_rate: 32,
            chunk_generate_rate: 16,
        };
        let decision = |action, pressure| crate::AutoscaleDecision {
            action,
            pressure,
            limits,
            reason: "test decision".to_string(),
        };

        apply_runtime_control_decision(
            &resources,
            &sessions,
            &decision(crate::AutoscaleAction::Hold, None),
            false,
        )
        .unwrap();
        assert_eq!(sessions.entity_owner_reconfiguration_calls(), 0);
        assert_eq!(sessions.prepared_chunk_shed_calls(), 0);

        apply_runtime_control_decision(
            &resources,
            &sessions,
            &decision(
                crate::AutoscaleAction::Hold,
                Some(crate::AutoscalePressure::Memory),
            ),
            false,
        )
        .unwrap();
        assert_eq!(sessions.entity_owner_reconfiguration_calls(), 0);
        assert_eq!(sessions.prepared_chunk_shed_calls(), 1);

        apply_runtime_control_decision(
            &resources,
            &sessions,
            &decision(crate::AutoscaleAction::ScaleUp, None),
            false,
        )
        .unwrap();
        assert_eq!(resources.cpu_limit(), 8);
        assert_eq!(sessions.entity_owner_reconfiguration_calls(), 0);

        apply_runtime_control_decision(
            &resources,
            &sessions,
            &decision(crate::AutoscaleAction::ScaleDown, None),
            true,
        )
        .unwrap();
        assert_eq!(resources.cpu_limit(), 1);
        assert_eq!(sessions.entity_owner_lane_count(), 1);
        assert_eq!(sessions.entity_owner_reconfiguration_calls(), 1);
    }

    #[test]
    fn runtime_telemetry_snapshot_exposes_memory_and_session_counts() {
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 384,
                limit_mb: 2_048,
            },
        );
        let runtime_control = crate::RuntimeControlHandle::new_with_memory_pressure(
            crate::RuntimeControlConfig {
                policy: crate::AutoscalePolicy::default(),
                initial_limits: crate::RuntimeControlLimits {
                    view_distance: 8,
                    chunk_send_rate: 16,
                    chunk_load_rate: 32,
                    chunk_generate_rate: 16,
                },
            },
            memory_pressure,
        );
        let (simulation, _simulation_owner) = play::simulation_channel();
        let telemetry = RuntimeTelemetryHandle {
            tick_metrics: RuntimeTickMetricsHandle::default(),
            sessions: Arc::new(play::SessionRegistry::new()),
            runtime_control: Some(runtime_control),
            simulation,
        };

        let snapshot = telemetry.snapshot();
        assert!(snapshot.tick_percentiles.is_none());
        assert_eq!(snapshot.active_sessions, 0);
        assert_eq!(snapshot.server_entities, 0);
        assert_eq!(snapshot.simulation_queue_capacity, 1024);
        assert_eq!(snapshot.simulation_queue_depth, 0);
        assert_eq!(snapshot.simulation_queue_max_depth, 0);
        assert_eq!(snapshot.simulation_commands_processed, 0);
        assert_eq!(snapshot.simulation_commands_rejected_full, 0);
        assert_eq!(snapshot.simulation_commands_rejected_world_busy, 0);
        assert_eq!(snapshot.simulation_commands_rejected_world_unavailable, 0);
        assert_eq!(snapshot.simulation_commands_rejected_world_mutation, 0);
        assert_eq!(snapshot.simulation_commands_rejected_stale_session, 0);
        assert_eq!(snapshot.simulation_block_edits_processed, 0);
        assert_eq!(snapshot.simulation_container_commits_processed, 0);
        assert_eq!(snapshot.simulation_block_entity_commits_processed, 0);
        assert_eq!(snapshot.memory_used_mb, 384);
        assert_eq!(snapshot.memory_limit_mb, 2_048);
        assert!(snapshot.memory_sample_available);
        assert_eq!(snapshot.memory_sample_failures, 0);
    }

    fn report(id: &str, props: &[(&str, &[&str])], states: &[StateSpec<'_>]) -> BlockReport {
        BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties: props
                .iter()
                .map(|(name, values)| {
                    (
                        (*name).to_string(),
                        values.iter().map(|value| (*value).to_string()).collect(),
                    )
                })
                .collect(),
            states: states
                .iter()
                .map(|(id, default, props)| BlockStateReport {
                    id: *id,
                    default: *default,
                    properties: props
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                        .collect::<BTreeMap<_, _>>(),
                })
                .collect(),
        }
    }

    fn state_id(blocks: &[BlockReport], block_name: &str, properties: &[(&str, &str)]) -> u32 {
        let block = blocks
            .iter()
            .find(|block| block.id.as_str() == block_name)
            .unwrap_or_else(|| panic!("missing block {block_name}"));
        block
            .states
            .iter()
            .find(|state| {
                properties.iter().all(|(name, value)| {
                    state.properties.get(*name).map(String::as_str) == Some(*value)
                })
            })
            .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"))
            .id
    }

    fn isolated_oak_fence_physics_world() -> (WorldStorage, BlockMaterialIds) {
        let reports = mc_data::blocks::solaris_required_blocks_report();
        let air = state_id(&reports, "minecraft:air", &[]);
        let fence = state_id(
            &reports,
            "minecraft:oak_fence",
            &[
                ("east", "false"),
                ("north", "false"),
                ("south", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ],
        );
        let registry = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let materials = material_ids(&registry, &facts);
        let mut storage = WorldStorage::in_memory(registry);
        let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            chunk_pos,
            mc_world::BlockStateId(air),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let _ = chunk.set_block(9, 64, 8, mc_world::BlockStateId(fence));
        storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
        (storage, materials)
    }

    #[test]
    fn broken_pipe_is_graceful_disconnect() {
        let err = ConnectionError::Io(std::io::Error::from(ErrorKind::BrokenPipe));
        assert!(is_client_disconnect(&err));
    }

    #[test]
    fn codec_error_is_not_graceful_disconnect() {
        let err = ConnectionError::UnexpectedPacketId {
            state: mc_protocol::State::Login,
            expected: 1,
            got: 2,
        };
        assert!(!is_client_disconnect(&err));
    }

    #[test]
    fn material_ids_treat_generated_vegetation_as_passable() {
        let reports = vec![
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
            report(
                "minecraft:water",
                &[("level", &["0", "1"])],
                &[(2, true, &[("level", "0")]), (8, false, &[("level", "1")])],
            ),
            report(
                "minecraft:lava",
                &[("level", &["0", "1"])],
                &[(3, true, &[("level", "0")]), (9, false, &[("level", "1")])],
            ),
            report("minecraft:short_grass", &[], &[(4, true, &[])]),
            report("minecraft:poppy", &[], &[(5, true, &[])]),
            report(
                "minecraft:sugar_cane",
                &[("age", &["0", "1"])],
                &[(6, true, &[("age", "0")]), (7, false, &[("age", "1")])],
            ),
        ];
        let registry = BlockRegistry::from_report(&reports).unwrap();
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let ids = material_ids(&registry, &facts);

        assert_eq!(ids.classify(1), BlockMaterial::Solid);
        assert_eq!(ids.classify(2), BlockMaterial::Water);
        assert_eq!(ids.classify(3), BlockMaterial::Lava);
        assert_eq!(ids.classify(4), BlockMaterial::Air);
        assert_eq!(ids.classify(5), BlockMaterial::Air);
        assert_eq!(ids.classify(6), BlockMaterial::Air);
        assert_eq!(ids.classify(7), BlockMaterial::Air);
        assert_eq!(ids.classify(8), BlockMaterial::Water);
        assert_eq!(ids.classify(9), BlockMaterial::Lava);
    }

    #[test]
    fn entity_physics_refuses_unloaded_boundary_samples() {
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(15.8, 64.0, 0.5),
            velocity: mc_entity::Vec3::new(1.0, 0.0, 0.0),
            aabb: mc_physics::Aabb::COW,
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Default,
        };
        let step = step_sampled_entity(EntityPhysicsInput {
            query,
            snapshot: Arc::new(EntityPhysicsSnapshot {
                chunks: HashMap::new(),
                materials: Arc::new(BlockMaterialIds::new(0, None, None)),
                blocks: None,
            }),
            complete_samples: false,
        });

        assert_eq!(step.position, query.position);
        assert_eq!(step.velocity, mc_entity::Vec3::ZERO);
    }

    #[test]
    fn living_entity_physics_pushes_horizontal_collision() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let materials = BlockMaterialIds::new(0, None, None);
        let mut storage = WorldStorage::in_memory(registry);
        let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            chunk_pos,
            mc_world::BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..mc_world::SECTION_DIM as u8 {
            for z in 0..mc_world::SECTION_DIM as u8 {
                let _ = chunk.set_block(x, 63, z, mc_world::BlockStateId(1));
            }
        }
        let _ = chunk.set_block(9, 64, 8, mc_world::BlockStateId(1));
        let _ = chunk.set_block(9, 65, 8, mc_world::BlockStateId(1));
        storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.5, 64.0, 8.5),
            velocity: mc_entity::Vec3::new(20.0, 0.0, 0.0),
            aabb: mc_physics::Aabb::COW,
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Living,
        };

        let step =
            step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

        assert!(step.horizontal_collision);
        assert_eq!(step.position, query.position);
        assert_eq!(step.velocity, mc_entity::Vec3::ZERO);
        assert!(step.on_ground);
    }

    #[test]
    fn living_entity_walks_across_farmland_at_its_collision_height() {
        let reports = [
            report("minecraft:air", &[], &[(0, true, &[])]),
            report(
                "minecraft:farmland",
                &[("moisture", &["0", "7"])],
                &[
                    (1, true, &[("moisture", "0")]),
                    (2, false, &[("moisture", "7")]),
                ],
            ),
        ];
        let registry = Arc::new(BlockRegistry::from_report(&reports).unwrap());
        let facts = BlockFactsTable::from_blocks_report(&reports);
        let materials = material_ids(&registry, &facts);
        let mut storage = WorldStorage::in_memory(registry);
        let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            chunk_pos,
            mc_world::BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..mc_world::SECTION_DIM as u8 {
            for z in 0..mc_world::SECTION_DIM as u8 {
                let _ = chunk.set_block(x, 64, z, mc_world::BlockStateId(2));
            }
        }
        storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
        let farmland_top = 64.0 + 15.0 / 16.0;
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.5, farmland_top, 8.5),
            velocity: mc_entity::Vec3::new(2.0, 0.0, 0.0),
            aabb: mc_physics::Aabb::COW,
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Living,
        };

        let step =
            step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

        assert!(step.position.x > query.position.x);
        assert_eq!(step.position.y, farmland_top);
        assert!(!step.horizontal_collision);
        assert!(step.on_ground);
    }

    #[test]
    fn sampled_entity_moves_through_empty_side_of_isolated_fence() {
        let (mut storage, materials) = isolated_oak_fence_physics_world();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.0, 64.0, 8.1),
            velocity: mc_entity::Vec3::new(20.0, 0.0, 0.0),
            aabb: mc_physics::Aabb {
                half_width: 0.2,
                height: 0.7,
            },
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Living,
        };

        let step =
            step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

        assert!(step.position.x > query.position.x);
        assert!(!step.horizontal_collision);
    }

    #[test]
    fn sampled_entity_collides_with_overheight_center_of_isolated_fence() {
        let (mut storage, materials) = isolated_oak_fence_physics_world();
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.0, 65.3, 8.5),
            velocity: mc_entity::Vec3::new(30.0, 0.0, 0.0),
            aabb: mc_physics::Aabb {
                half_width: 0.2,
                height: 0.3,
            },
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Living,
        };

        let step =
            step_sampled_entity(sample_entity_physics_input(query, &mut storage, &materials));

        assert!(step.horizontal_collision);
        assert_eq!(step.position.x, query.position.x);
    }

    struct FlatGenerator {
        air: mc_world::BlockStateId,
        ground: mc_world::BlockStateId,
        biome: Identifier,
    }

    impl mc_world::ChunkGenerator for FlatGenerator {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let mut chunk = mc_world::Chunk::empty(pos, self.air, self.biome.clone());
            for x in 0..mc_world::SECTION_DIM as u8 {
                for z in 0..mc_world::SECTION_DIM as u8 {
                    let _ = chunk.set_block(x, 63, z, self.ground);
                }
            }
            chunk
        }
    }

    #[test]
    fn entity_physics_uses_only_cached_chunks_for_sampling() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let facts = BlockFactsTable::default();
        let materials = material_ids(&registry, &facts);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory(Arc::clone(&registry)).with_generator(Arc::new(
            FlatGenerator {
                air: mc_world::BlockStateId(0),
                ground: mc_world::BlockStateId(1),
                biome,
            },
        ));
        let query = play::EntityPhysicsQuery {
            id: mc_entity::EntityId(42),
            position: mc_entity::Vec3::new(8.5, 66.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: false,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Default,
        };

        let input = sample_entity_physics_input(query, &mut storage, &materials);
        assert!(!input.complete_samples);

        storage
            .get_chunk(mc_world::ChunkPos { x: 0, z: 0 })
            .expect("generate spawn chunk")
            .expect("spawn chunk generated");
        let input = sample_entity_physics_input(query, &mut storage, &materials);
        assert!(input.complete_samples);
        let step = step_sampled_entity(input);

        assert!(step.position.y < query.position.y);
        assert!(step.velocity.y < 0.0);
    }

    #[test]
    fn entity_physics_fetches_each_cached_chunk_once_per_snapshot() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory(Arc::clone(&registry));
        storage
            .insert_generated_chunk(
                mc_world::ChunkPos { x: 0, z: 0 },
                mc_world::Chunk::empty(
                    mc_world::ChunkPos { x: 0, z: 0 },
                    mc_world::BlockStateId(0),
                    biome,
                ),
            )
            .unwrap();
        let queries = [8.5, 8.6, 24.5, 24.6].map(|x| play::EntityPhysicsQuery {
            id: mc_entity::EntityId(x as i32),
            position: mc_entity::Vec3::new(x, 64.0, 8.5),
            velocity: mc_entity::Vec3::ZERO,
            aabb: mc_physics::Aabb::COW,
            on_ground: true,
            fall_distance: 0.0,
            kind: play::EntityPhysicsKind::Default,
        });
        let plans = entity_physics_sample_plans(&queries);
        let fetches = std::cell::Cell::new(0);

        let chunks = entity_physics_chunk_snapshots(&plans, |cpos| {
            fetches.set(fetches.get() + 1);
            storage.cached_chunk_snapshot(cpos)
        });

        assert_eq!(fetches.get(), 2);
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks
                .get(&mc_world::ChunkPos { x: 0, z: 0 })
                .is_some_and(Option::is_some)
        );
        assert!(
            chunks
                .get(&mc_world::ChunkPos { x: 1, z: 0 })
                .is_some_and(Option::is_none)
        );
    }

    #[test]
    fn entity_physics_batch_sampling_shares_chunk_snapshots() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let facts = BlockFactsTable::default();
        let materials = material_ids(&registry, &facts);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut storage = WorldStorage::in_memory(Arc::clone(&registry)).with_generator(Arc::new(
            FlatGenerator {
                air: mc_world::BlockStateId(0),
                ground: mc_world::BlockStateId(1),
                biome,
            },
        ));
        storage
            .get_chunk(mc_world::ChunkPos { x: 0, z: 0 })
            .expect("generate spawn chunk")
            .expect("spawn chunk generated");
        let queries = (0..49)
            .map(|idx| play::EntityPhysicsQuery {
                id: mc_entity::EntityId(idx + 1),
                position: mc_entity::Vec3::new(
                    8.5 + f64::from(idx % 7) * 0.05,
                    66.0,
                    8.5 + f64::from(idx / 7) * 0.05,
                ),
                velocity: mc_entity::Vec3::ZERO,
                aabb: mc_physics::Aabb::COW,
                on_ground: false,
                fall_distance: 0.0,
                kind: play::EntityPhysicsKind::Default,
            })
            .collect::<Vec<_>>();

        let plans = entity_physics_sample_plans(&queries);
        let chunks =
            entity_physics_chunk_snapshots(&plans, |cpos| storage.cached_chunk_snapshot(cpos));
        let snapshot = Arc::new(EntityPhysicsSnapshot {
            chunks,
            materials: Arc::new(materials),
            blocks: Some(storage.registry_arc()),
        });
        let inputs = entity_physics_inputs_from_snapshot(plans, snapshot);

        assert_eq!(inputs.len(), queries.len());
        assert!(inputs.iter().all(|input| input.complete_samples));
        assert!(Arc::ptr_eq(&inputs[0].snapshot, &inputs[1].snapshot));
        assert_eq!(inputs[0].snapshot.chunks.len(), 1);
    }

    #[test]
    fn cached_material_ids_recovers_poisoned_mutex_state() {
        let cache = PHYSICS_MATERIAL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));

        let poisoned = std::panic::catch_unwind(|| {
            let _guard = cache.lock().unwrap();
            panic!("inject material cache poison");
        });
        assert!(poisoned.is_err());

        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "poisoned-material-cache-test".into(),
            max_players: 0,
            view_distance: 0,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: None,
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: canonical_entity_types(),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        };

        let first = cached_material_ids(&config);
        let second = cached_material_ids(&config);

        assert!(Arc::ptr_eq(&first, &second));
        assert!(cache.lock().is_ok());
    }

    #[tokio::test]
    async fn serve_shutdown_notification_requests_runtime_control_drain() {
        let shutdown = ShutdownHandle::default();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "runtime-drain-shutdown-test".into(),
            max_players: 0,
            view_distance: 0,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: None,
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: canonical_entity_types(),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy {
                chunk_worker_threads: 8,
                runtime_control: Some(crate::RuntimeControlConfig {
                    policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
                    initial_limits: crate::RuntimeControlLimits {
                        view_distance: 4,
                        chunk_send_rate: 8,
                        chunk_load_rate: 16,
                        chunk_generate_rate: 16,
                    },
                }),
                ..ChunkPipelinePolicy::default()
            },
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: shutdown.clone(),
        };

        let bound = bind(config).await.expect("bind");
        let resources = bound.chunk_pipeline_resources.clone();
        let runtime_control = bound
            .runtime_control_handle()
            .expect("runtime control enabled");
        let serve = tokio::spawn(bound.serve());

        shutdown.request();
        let serve_result = tokio::time::timeout(Duration::from_secs(2), serve)
            .await
            .expect("serve exits after shutdown")
            .expect("serve task joins");
        serve_result.expect("serve exits cleanly");

        assert!(runtime_control.snapshot().draining);
        assert_eq!(resources.cpu_limit(), 1);
    }

    #[tokio::test]
    async fn serve_shutdown_drains_without_starting_final_save() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        let shutdown = config.shutdown.clone();
        let bound = bind(config).await.expect("bind");
        let save = bound.save_handle();
        let serve = tokio::spawn(bound.serve());

        shutdown.request();
        tokio::time::timeout(Duration::from_secs(2), serve)
            .await
            .expect("serve drain exits after shutdown")
            .expect("serve task joins")
            .expect("serve drain succeeds");

        let metadata = tmp.path().join("solaris").join("world.dat");
        assert!(
            !metadata.exists(),
            "serve drain must not perform the final save"
        );

        let report = save.save_all_after_drain().await;
        assert!(report.is_ok(), "single final save failed: {report:?}");
        assert!(report.world_metadata_saved);
        assert!(metadata.exists());
    }

    #[tokio::test]
    async fn public_run_performs_final_save_after_successful_drain() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        config.shutdown.request();

        run(config).await.expect("public run drains and saves");

        assert!(tmp.path().join("solaris").join("world.dat").exists());
    }

    #[tokio::test]
    async fn run_bound_propagates_final_save_failure() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        let shutdown = config.shutdown.clone();
        let bound = bind(config).await.expect("bind");
        std::fs::create_dir(tmp.path().join("solaris/world.dat")).unwrap();
        shutdown.request();

        let error = serve_then_final_save(bound)
            .await
            .expect_err("final save failure reaches run caller");

        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(error.to_string().contains("final save failed"));
    }

    #[tokio::test]
    async fn serve_error_still_runs_final_save_without_masking_primary_error() {
        let save_called = Arc::new(AtomicBool::new(false));
        let save_called_by_future = Arc::clone(&save_called);
        let save = async move {
            save_called_by_future.store(true, Ordering::SeqCst);
            SaveAllReport {
                players_saved: 1,
                entities_saved: 2,
                chunks_flushed: 3,
                world_metadata_saved: true,
                timings: SaveAllTimings::default(),
                errors: vec!["final save failed".to_owned()],
            }
        };

        let error = finish_serve_with_final_save(
            Err(std::io::Error::new(
                ErrorKind::ConnectionAborted,
                "listener accept failed",
            )),
            save,
        )
        .await
        .expect_err("primary serve error remains visible");

        assert!(save_called.load(Ordering::SeqCst));
        assert_eq!(error.kind(), ErrorKind::ConnectionAborted);
        assert_eq!(error.to_string(), "listener accept failed");
    }

    #[tokio::test]
    async fn run_bound_drains_admitted_simulation_mutation_before_final_save() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        let shutdown = config.shutdown.clone();
        let bound = bind(config).await.expect("bind");
        let simulation = bound.simulation.clone();
        let mut mutation = Box::pin(simulation.set_world_time_server_owned(73));

        std::future::poll_fn(|context| {
            assert!(
                mutation.as_mut().poll(context).is_pending(),
                "mutation must remain in flight until the simulation owner starts"
            );
            std::task::Poll::Ready(())
        })
        .await;
        shutdown.request();

        serve_then_final_save(bound)
            .await
            .expect("drain and final save succeed");
        mutation
            .await
            .expect("admitted mutation completes during drain");
        let metadata = play::persistence::load_world_metadata(tmp.path())
            .unwrap()
            .expect("world metadata saved");
        assert!(
            metadata.world_time >= 73,
            "final save must include the admitted world-time mutation"
        );
    }

    #[tokio::test]
    async fn entity_owner_failure_is_push_published_and_typed_connection_panic_is_fatal() {
        let sessions = Arc::new(play::SessionRegistry::new());
        let mut failure = sessions.subscribe_entity_owner_failure();
        let reported = sessions
            .report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::WorkerPanicked);

        failure
            .changed()
            .await
            .expect("owner fatal watch remains open");
        assert_eq!(
            failure.borrow_and_update().as_ref().copied(),
            Some(reported)
        );

        let task_sessions = Arc::clone(&sessions);
        let join = tokio::spawn(async move {
            let _ = task_sessions.entity_owner_status_for_test();
        })
        .await
        .expect_err("owner fatal blocks the connection task through typed unwind");
        let error = connection_task_join_error(join);

        assert!(is_entity_owner_serve_error(&error));
        assert!(error.to_string().contains("WorkerPanicked"));
    }

    #[tokio::test]
    async fn unrelated_connection_task_panic_is_terminal_without_claiming_owner_uncertainty() {
        let join = tokio::spawn(async {
            panic!("injected connection task panic");
        })
        .await
        .expect_err("connection task panic produces a join error");

        let error = connection_task_join_error(join);

        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(!is_entity_owner_serve_error(&error));
        assert_eq!(error.to_string(), "connection task panicked");
    }

    #[tokio::test]
    async fn entity_owner_serve_error_skips_clean_final_save() {
        let save_called = Arc::new(AtomicBool::new(false));
        let save_called_by_future = Arc::clone(&save_called);
        let save = async move {
            save_called_by_future.store(true, Ordering::SeqCst);
            SaveAllReport {
                players_saved: 0,
                entities_saved: 0,
                chunks_flushed: 0,
                world_metadata_saved: false,
                timings: SaveAllTimings::default(),
                errors: Vec::new(),
            }
        };
        let primary = entity_owner_serve_error(mc_entity::RegionOwnerLaneError::OutcomeUnknown);

        let error = finish_serve_with_final_save(Err(primary), save)
            .await
            .expect_err("uncertain owner state must remain terminal");

        assert!(!save_called.load(Ordering::SeqCst));
        assert!(is_entity_owner_serve_error(&error));
        assert!(error.to_string().contains("OutcomeUnknown"));
    }

    #[tokio::test]
    async fn authoritative_runtime_poison_is_terminal_and_skips_clean_final_save() {
        let lock = Arc::new(std::sync::Mutex::new(()));
        let poisoned = Arc::clone(&lock);
        let _ = std::thread::spawn(move || {
            let _guard = poisoned.lock().unwrap();
            panic!("inject authoritative runtime poison");
        })
        .join();
        let before = crate::runtime_lock_poison_metrics_snapshot().authoritative_poison;
        let task_lock = Arc::clone(&lock);
        let join = tokio::spawn(async move {
            drop(crate::lock_policy::lock_authoritative_mutex(
                &task_lock,
                "test.runtime_authority",
            ));
        })
        .await
        .expect_err("poisoned runtime authority must unwind its task");
        let primary = runtime_task_join_error("connection", join);

        assert!(is_uncertain_runtime_serve_error(&primary));
        assert!(primary.to_string().contains("test.runtime_authority"));
        assert!(crate::runtime_lock_poison_metrics_snapshot().authoritative_poison > before);

        let save_called = Arc::new(AtomicBool::new(false));
        let save_called_by_future = Arc::clone(&save_called);
        let save = async move {
            save_called_by_future.store(true, Ordering::SeqCst);
            SaveAllReport {
                players_saved: 0,
                entities_saved: 0,
                chunks_flushed: 0,
                world_metadata_saved: false,
                timings: SaveAllTimings::default(),
                errors: Vec::new(),
            }
        };
        let error = finish_serve_with_final_save(Err(primary), save)
            .await
            .expect_err("poisoned runtime state must remain terminal");

        assert!(!save_called.load(Ordering::SeqCst));
        assert!(is_uncertain_runtime_serve_error(&error));
    }

    #[tokio::test]
    async fn unrelated_command_task_panic_is_a_terminal_drain_error() {
        let result = tokio::spawn(async {
            panic!("injected command task failure");
            #[allow(unreachable_code)]
            "test command"
        })
        .await;

        let error = log_command_task_exit(result, true).expect_err("join failure propagates");

        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(!is_entity_owner_serve_error(&error));
        assert_eq!(error.to_string(), "command task panicked");
    }

    #[tokio::test]
    async fn typed_owner_panic_from_command_or_entity_task_remains_owner_fatal() {
        let sessions = Arc::new(play::SessionRegistry::new());
        sessions
            .report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::OutcomeUnknown);

        let command_sessions = Arc::clone(&sessions);
        let command_result = tokio::spawn(async move {
            let _ = command_sessions.entity_owner_status_for_test();
            #[allow(unreachable_code)]
            "script command"
        })
        .await;
        let command_error =
            log_command_task_exit(command_result, false).expect_err("typed panic propagates");
        assert!(is_entity_owner_serve_error(&command_error));
        assert!(command_error.to_string().contains("OutcomeUnknown"));

        let entity_sessions = Arc::clone(&sessions);
        let entity_result = tokio::spawn(async move {
            let _ = entity_sessions.entity_owner_status_for_test();
        })
        .await;
        let shutdown = ShutdownHandle::default();
        let entity_error = handle_entity_ticker_exit(&shutdown, entity_result)
            .expect_err("typed panic propagates");
        assert!(is_entity_owner_serve_error(&entity_error));
        assert!(entity_error.to_string().contains("OutcomeUnknown"));
        assert!(shutdown.is_requested());
    }

    #[tokio::test]
    async fn periodic_save_worker_failure_is_a_drain_error() {
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let mut started = Some(started);
        let worker = crate::dirty_flush::DirtyFlushCoordinator::spawn(move || {
            let started = started.take().expect("one flush invocation is expected");
            async move {
                started.send(()).expect("test observes worker start");
                panic!("injected periodic save worker failure");
            }
        });
        worker.notifier().request();
        started_rx.await.expect("worker reports start");

        let error = drain_periodic_save_worker(Some(worker))
            .await
            .expect_err("periodic worker failure propagates");

        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(!is_uncertain_runtime_serve_error(&error));
        assert_eq!(error.to_string(), "periodic save task panicked");
    }

    #[tokio::test]
    async fn accept_failure_requests_shutdown_and_runtime_drain() {
        let shutdown = ShutdownHandle::default();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "accept-failure-drain-test".into(),
            max_players: 0,
            view_distance: 0,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: None,
            tags: Arc::new(TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::new(mc_data::items::ItemRegistry::from_report(&[])),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: canonical_entity_types(),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy {
                chunk_worker_threads: 8,
                runtime_control: Some(crate::RuntimeControlConfig {
                    policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
                    initial_limits: crate::RuntimeControlLimits {
                        view_distance: 4,
                        chunk_send_rate: 8,
                        chunk_load_rate: 16,
                        chunk_generate_rate: 16,
                    },
                }),
                ..ChunkPipelinePolicy::default()
            },
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: shutdown.clone(),
        };

        let bound = bind(config).await.expect("bind");
        let resources = bound.chunk_pipeline_resources.clone();
        let runtime_control = bound
            .runtime_control_handle()
            .expect("runtime control enabled");
        let error = handle_accept_failure(
            std::io::Error::new(ErrorKind::ConnectionAborted, "injected accept failure"),
            &shutdown,
            Some(&runtime_control),
            &resources,
            &bound.sessions,
        );

        assert_eq!(error.kind(), ErrorKind::ConnectionAborted);
        assert!(shutdown.is_requested());
        assert!(runtime_control.snapshot().draining);
        assert_eq!(resources.cpu_limit(), 1);
    }

    #[tokio::test]
    async fn entity_ticker_drain_waits_for_in_flight_tick() {
        let entered_tick = Arc::new(Notify::new());
        let release_tick = Arc::new(Notify::new());
        let task_entered = Arc::clone(&entered_tick);
        let task_release = Arc::clone(&release_tick);
        let ticker = tokio::spawn(async move {
            task_entered.notify_waiters();
            task_release.notified().await;
        });
        entered_tick.notified().await;

        let mut drain = std::pin::pin!(drain_entity_ticker(ticker));
        let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
        probe_tx.send(()).unwrap();
        tokio::select! {
            biased;
            result = &mut drain => panic!("entity ticker drain returned before the in-flight tick completed: {result:?}"),
            result = probe_rx => result.unwrap(),
        }

        release_tick.notify_waiters();
        drain.await.expect("entity ticker drains");
    }

    #[tokio::test]
    async fn entity_ticker_timeout_cancels_task_before_returning() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
        let ticker = tokio::spawn(async move {
            let _held_tx = held_tx;
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered_rx.await.unwrap();

        let error = drain_entity_ticker_with_timeout(ticker, Duration::ZERO)
            .await
            .expect_err("entity ticker timeout fails the drain");

        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(held_rx.await.is_err(), "ticker task must be dropped");
    }

    #[tokio::test]
    async fn late_owner_panic_during_connection_drain_remains_owner_fatal() {
        let sessions = Arc::new(play::SessionRegistry::new());
        sessions
            .report_entity_owner_failure_for_test(mc_entity::RegionOwnerLaneError::WorkerPanicked);
        let mut connections = tokio::task::JoinSet::new();
        connections.spawn(async move {
            let _ = sessions.entity_owner_status_for_test();
        });

        let error = drain_connections_with_timeout(&mut connections, Duration::from_secs(1))
            .await
            .expect_err("late owner panic must fail the drain");

        assert!(connections.is_empty());
        assert!(is_entity_owner_serve_error(&error));
        assert!(error.to_string().contains("WorkerPanicked"));
    }

    #[tokio::test]
    async fn connection_timeout_cancels_tasks_before_returning() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
        let mut connections = tokio::task::JoinSet::new();
        connections.spawn(async move {
            let _held_tx = held_tx;
            entered_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        entered_rx.await.unwrap();

        let error = drain_connections_with_timeout(&mut connections, Duration::ZERO)
            .await
            .expect_err("connection timeout fails the drain");

        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(connections.is_empty());
        assert!(held_rx.await.is_err(), "connection task must be dropped");
    }

    #[test]
    fn console_stop_requests_drain_and_shutdown() {
        let shutdown = ShutdownHandle::default();
        let runtime_control = RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 4,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 16,
            },
        });
        let resources = ChunkPipelineResources::with_limits(1, 4);
        let sessions = play::SessionRegistry::new();

        request_stop(&shutdown, Some(&runtime_control), &resources, &sessions);

        assert!(runtime_control.snapshot().draining);
        assert!(shutdown.is_requested());
    }

    #[tokio::test]
    async fn console_stop_requests_shutdown_without_early_save() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(ItemRegistry::default()),
            canonical_entity_types(),
        );
        let runtime_control = RuntimeControlHandle::new(crate::RuntimeControlConfig {
            policy: crate::AutoscalePolicy::for_profile(crate::AutoscaleProfile::Balanced),
            initial_limits: crate::RuntimeControlLimits {
                view_distance: 4,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 16,
            },
        });
        let resources = ChunkPipelineResources::with_limits(1, 4);
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let mut stop = std::pin::pin!(execute_console_command(
            "stop",
            "test save",
            "test stop",
            &config,
            &sessions,
            Some(&runtime_control),
            &simulation,
            &resources,
        ));

        let stopped = tokio::select! {
            biased;
            stopped = &mut stop => stopped,
            ready = owner.wait_for_command() => {
                assert!(ready, "simulation command channel remains open");
                assert_eq!(
                    owner
                        .process_tick_with_world(
                            &sessions,
                            config.world.as_ref(),
                            config.block_light.as_deref(),
                            1,
                        )
                        .processed,
                    1,
                );
                stop.await
            }
        };

        assert!(stopped);
        assert!(config.shutdown.is_requested());
        assert!(runtime_control.snapshot().draining);
        let metadata = tmp.path().join("solaris").join("world.dat");
        assert!(
            !metadata.exists(),
            "console stop must not save before the runtime drain"
        );

        owner.shutdown();
        let report = save_all_after_drain_with_context("test final save", &config, &sessions).await;
        assert!(report.is_ok(), "post-drain final save failed: {report:?}");
        assert!(metadata.exists());
    }

    #[tokio::test]
    async fn startup_dirty_flush_detects_dirty_disk_world_and_skips_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);

        assert_eq!(startup_dirty_flush_dirty_count(&config).await, None);

        {
            let world = config.world.as_ref().unwrap();
            let mut storage = world.lock().await;
            let cpos = mc_world::ChunkPos { x: 0, z: 0 };
            storage
                .insert_generated_chunk(
                    cpos,
                    mc_world::Chunk::empty(
                        cpos,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage
                .set_block_at(
                    mc_world::BlockPos { x: 1, y: 64, z: 1 },
                    mc_world::BlockStateId(1),
                )
                .unwrap();
        }

        assert_eq!(startup_dirty_flush_dirty_count(&config).await, Some(1));

        config.shutdown.request();
        assert_eq!(startup_dirty_flush_dirty_count(&config).await, None);
    }

    #[test]
    fn fenced_dirty_report_waits_for_the_exact_producer_wake() {
        assert_eq!(
            log_dirty_only_flush(
                "fenced dirty report test",
                Ok(DirtyOnlyFlushReport {
                    planned_chunks: 0,
                    flushed_chunks: 0,
                    remaining_dirty: 1,
                    immediately_flushable: false,
                }),
            ),
            crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer
        );
    }

    #[test]
    fn dirty_flush_failure_is_not_reported_as_complete() {
        assert_eq!(
            log_dirty_only_flush("dirty failure report test", Err("disk full".to_owned())),
            crate::dirty_flush::DirtyFlushCompletion::Failed
        );
    }

    #[tokio::test]
    async fn journal_fenced_dirty_flush_runs_once_and_allows_drain() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let world = Arc::new(Mutex::new(
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 1)
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let mut config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            items,
            canonical_entity_types(),
        );
        config.world = Some(Arc::clone(&world));
        let config = Arc::new(config);
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        {
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(
                        position,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            assert!(matches!(
                storage.stamp_cached_chunks_for_world_journal(7, &[position]),
                mc_world::JournalStampResult::Stamped(_)
            ));
        }
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            {
                let config = Arc::clone(&config);
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let config = Arc::clone(&config);
                    let dirty_calls = Arc::clone(&dirty_calls);
                    async move {
                        dirty_calls.fetch_add(1, Ordering::SeqCst);
                        log_dirty_only_flush(
                            "journal-fenced drain test",
                            flush_dirty_chunks_only(&config, 0).await,
                        )
                    }
                }
            },
            || async { panic!("journal fence must not request a full checkpoint") },
        );

        coordinator.notifier().request_dirty_flush();
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 1);
        assert_eq!(world.lock().await.dirty_count(), 1);
    }

    #[tokio::test]
    async fn exact_journal_fence_release_wakes_and_flushes_dirty_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let world = Arc::new(Mutex::new(
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 1)
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let mut config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            items,
            canonical_entity_types(),
        );
        config.world = Some(Arc::clone(&world));
        let config = Arc::new(config);
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        let mutation = {
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(
                        position,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            assert!(matches!(
                storage.stamp_cached_chunks_for_world_journal(8, &[position]),
                mc_world::JournalStampResult::Stamped(_)
            ));
            storage.mutation_view()
        };
        let (completed, mut completed_rx) = mpsc::channel(2);
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            {
                let config = Arc::clone(&config);
                move || {
                    let config = Arc::clone(&config);
                    let completed = completed.clone();
                    async move {
                        let result = log_dirty_only_flush(
                            "journal-fence release test",
                            flush_dirty_chunks_only(&config, 0).await,
                        );
                        completed.send(result).await.expect("test observes flush");
                        result
                    }
                }
            },
            || async { panic!("journal fence must not request a full checkpoint") },
        );
        let notifier = coordinator.notifier();
        world
            .lock()
            .await
            .set_dirty_high_water_notifier(Arc::new(move || {
                notifier.request_dirty_flush();
            }));

        coordinator.notifier().request_dirty_flush();
        assert_eq!(
            completed_rx.recv().await,
            Some(crate::dirty_flush::DirtyFlushCompletion::AwaitingProducer)
        );
        assert_eq!(
            mutation.clear_journal_pending_conditionally(8, &[position]),
            1
        );
        assert_eq!(
            completed_rx.recv().await,
            Some(crate::dirty_flush::DirtyFlushCompletion::Complete)
        );
        coordinator.drain().await;

        assert_eq!(world.lock().await.dirty_count(), 0);
    }

    #[tokio::test]
    async fn saturated_dirty_mutation_retries_after_failed_pressure_flush() {
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let world = Arc::new(Mutex::new(WorldStorage::in_memory_with_capacity(blocks, 1)));
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let (completed, mut completed_rx) = mpsc::channel(2);
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let call = dirty_calls.fetch_add(1, Ordering::SeqCst);
                    let completed = completed.clone();
                    async move {
                        completed.send(call).await.expect("test observes flush");
                        if call == 0 {
                            crate::dirty_flush::DirtyFlushCompletion::Failed
                        } else {
                            crate::dirty_flush::DirtyFlushCompletion::Complete
                        }
                    }
                }
            },
            || async { panic!("dirty pressure must not request a full checkpoint") },
        );
        let notifier = coordinator.notifier();
        world
            .lock()
            .await
            .set_dirty_high_water_notifier(Arc::new(move || {
                notifier.request_dirty_flush();
            }));
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        world
            .lock()
            .await
            .insert_generated_chunk(
                position,
                mc_world::Chunk::empty(
                    position,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert_eq!(completed_rx.recv().await, Some(0));

        world
            .lock()
            .await
            .set_block_at(
                mc_world::BlockPos { x: 1, y: 64, z: 1 },
                mc_world::BlockStateId(1),
            )
            .unwrap();
        assert_eq!(completed_rx.recv().await, Some(1));
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn startup_dirty_flush_drains_more_than_four_bounded_batches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let world = Arc::new(Mutex::new(
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 257)
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let mut config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
            entity_types,
        );
        config.world = Some(Arc::clone(&world));
        let config = Arc::new(config);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        {
            let mut storage = world.lock().await;
            for x in 0..257 {
                let position = mc_world::ChunkPos { x, z: 0 };
                storage
                    .insert_generated_chunk(
                        position,
                        mc_world::Chunk::empty(position, mc_world::BlockStateId(0), biome.clone()),
                    )
                    .unwrap();
            }
        }

        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            {
                let config = Arc::clone(&config);
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let config = Arc::clone(&config);
                    let dirty_calls = Arc::clone(&dirty_calls);
                    async move {
                        dirty_calls.fetch_add(1, Ordering::SeqCst);
                        log_dirty_only_flush(
                            "startup dirty-only flush test",
                            flush_dirty_chunks_only(&config, 0).await,
                        )
                    }
                }
            },
            || async { panic!("startup dirty-only path must not run a full checkpoint") },
        );
        let requests = coordinator.notifier();

        enqueue_startup_dirty_flush(&config, &requests).await;
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 5);
        assert_eq!(world.lock().await.stats().dirty_chunks, 0);
        assert!(
            play::persistence::load_world_metadata(tmp.path())
                .unwrap()
                .is_none(),
            "startup dirty-only flush must exclude full-checkpoint metadata"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn bind_prepares_spawn_chunk_without_holding_world_lock() {
        struct PausedGenerator {
            entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }

        impl mc_world::chunk::ChunkGenerator for PausedGenerator {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                if let Some(entered) = self.entered.lock().unwrap().take() {
                    let _ = entered.send(());
                }
                self.release.lock().unwrap().recv().unwrap();
                let mut chunk = mc_world::Chunk::empty(
                    pos,
                    mc_world::BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                );
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let generator = Arc::new(PausedGenerator {
            entered: std::sync::Mutex::new(Some(entered_tx)),
            release: std::sync::Mutex::new(release_rx),
        });
        let world = Arc::new(Mutex::new(
            WorldStorage::open(tmp.path(), Arc::clone(&blocks))
                .unwrap()
                .with_generator(generator),
        ));
        let mut config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);
        config.world = Some(Arc::clone(&world));

        let bind_task = tokio::spawn(async move { bind(config).await });
        tokio::time::timeout(Duration::from_secs(2), entered_rx)
            .await
            .expect("bind should start detached spawn generation")
            .expect("spawn generator should report entry");

        let world_available = match tokio::time::timeout(Duration::from_secs(1), world.lock()).await
        {
            Ok(storage) => {
                drop(storage);
                true
            }
            Err(_) => false,
        };
        release_tx.send(()).unwrap();
        let bound = bind_task.await.unwrap().unwrap();

        assert!(
            world_available,
            "spawn generation must not hold the shared world lock"
        );
        assert!(
            world
                .lock()
                .await
                .cached_chunk_snapshot(mc_world::ChunkPos { x: 0, z: 0 })
                .is_some(),
            "bind should commit the prepared spawn chunk"
        );
        drop(bound);
    }

    fn save_all_test_config(
        tmp: &std::path::Path,
        blocks: Arc<BlockRegistry>,
        items: Arc<mc_data::items::ItemRegistry>,
        entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
    ) -> ServerConfig {
        let world = Arc::new(Mutex::new(
            WorldStorage::open(tmp, Arc::clone(&blocks))
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "test".into(),
            max_players: 1,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: Some(world),
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items,
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types,
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), true),
            loader_manifest: None,
            shutdown: ShutdownHandle::default(),
        }
    }

    #[tokio::test]
    async fn bind_replays_and_acknowledges_pending_regional_entity_commit() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let entity_types = canonical_entity_types();
        let snapshot = mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(1_000_001),
            uuid: uuid::Uuid::from_u128(71),
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: mc_entity::Vec3::new(0.2, 0.0, 0.0),
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 14.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: mc_entity::GoalState::FollowPosition {
                target: mc_entity::Vec3::new(8.0, 64.0, -3.5),
                speed: 0.4,
            },
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState::baby()),
            retained: mc_entity::EntityRetainedState::default(),
        };
        let decision = mc_entity::RegionalCommitDecision::from_parts(
            mc_entity::RegionPhase(1),
            19,
            vec![snapshot.clone()],
            Vec::new(),
        )
        .unwrap();
        let (mut journal, pending) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(pending.is_empty());
        mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
        drop(journal);

        let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
        let mut bound = bind(config)
            .await
            .expect("bind with pending owner decision");
        let restored = bound.sessions.persisted_entity_save_snapshot().0.records;
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].snapshot, snapshot);
        let (_, pending) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0], decision);
        assert_eq!(pending[1].upserts(), std::slice::from_ref(&snapshot));
        assert!(pending[1].removed().is_empty());
        assert!(pending[1].phase() > decision.phase());
        assert!(pending[1].sequence_watermark() > decision.sequence_watermark());

        let report = {
            let mut save = std::pin::pin!(save_all_after_simulation_barrier(
                "recovered journal checkpoint test",
                &bound.config,
                &bound.sessions,
                &bound.simulation,
            ));
            let command_ready = tokio::select! {
                report = &mut save => panic!("save completed before owner snapshot: {report:?}"),
                ready = bound.simulation_owner.wait_for_command() => ready,
            };
            assert!(command_ready, "simulation command channel closed");
            assert_eq!(
                bound
                    .simulation_owner
                    .process_tick_with_world(
                        &bound.sessions,
                        bound.config.world.as_ref(),
                        bound.config.block_light.as_deref(),
                        1,
                    )
                    .processed,
                1
            );
            save.as_mut().await
        };
        assert!(report.is_ok(), "save failed: {:?}", report.errors);
        let (_, pending) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert_eq!(pending.len(), 2, "checkpoint cleanup stays memory-only");
        let checkpoint = play::persistence::load_persisted_entities(
            tmp.path(),
            bound.config.items.as_ref(),
            bound.config.entity_types.as_ref(),
        )
        .unwrap();
        let replayed = play::persistence::replay_regional_commit_decisions(checkpoint, &pending)
            .expect("saved owner snapshot filters checkpointed WAL records");
        assert_eq!(replayed.records.len(), 1);
        assert_eq!(replayed.records[0].snapshot, snapshot);

        drop(bound);
        let (_, pending) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert!(
            pending.is_empty(),
            "normal shutdown compacts checkpointed WAL"
        );
    }

    #[tokio::test]
    async fn bind_keeps_recovered_entity_removal_until_full_snapshot_save() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let entity_types = canonical_entity_types();
        let snapshot = mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(1_000_001),
            uuid: uuid::Uuid::from_u128(72),
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 14.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: mc_entity::GoalState::Idle,
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState::adult()),
            retained: mc_entity::EntityRetainedState::default(),
        };
        play::persistence::save_persisted_entities(
            tmp.path(),
            items.as_ref(),
            std::slice::from_ref(&snapshot),
        )
        .unwrap();
        let decision = mc_entity::RegionalCommitDecision::from_parts(
            mc_entity::RegionPhase(1),
            19,
            Vec::new(),
            vec![snapshot.id],
        )
        .unwrap();
        let (mut journal, _) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
        drop(journal);

        let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
        let bound = bind(config)
            .await
            .expect("bind with pending entity removal");
        assert!(
            bound
                .sessions
                .persisted_entity_save_snapshot()
                .0
                .records
                .is_empty()
        );
        let (_, pending) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        assert_eq!(pending, vec![decision]);
    }

    #[tokio::test]
    async fn bind_rejects_duplicate_final_entity_uuid_before_restore() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let entity_types = canonical_entity_types();
        let duplicate_uuid = uuid::Uuid::from_u128(73);
        let persisted = mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(1_000_001),
            uuid: duplicate_uuid,
            type_id: 30,
            type_name: "minecraft:cow".into(),
            position: mc_entity::Vec3::new(4.5, 64.0, -3.5),
            rotation: mc_entity::Rotation::ZERO,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: true,
            item_stack: None,
            experience_value: None,
            block_state: None,
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 14.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: mc_entity::GoalState::Idle,
            vehicle: None,
            animal: Some(mc_entity::AnimalBreedingState::adult()),
            retained: mc_entity::EntityRetainedState::default(),
        };
        play::persistence::save_persisted_entities(
            tmp.path(),
            items.as_ref(),
            std::slice::from_ref(&persisted),
        )
        .unwrap();
        let duplicate = mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(1_000_002),
            ..persisted
        };
        let decision = mc_entity::RegionalCommitDecision::from_parts(
            mc_entity::RegionPhase(1),
            19,
            vec![duplicate],
            Vec::new(),
        )
        .unwrap();
        let (mut journal, _) =
            play::persistence::FileRegionalDecisionJournal::open(tmp.path()).unwrap();
        mc_entity::RegionalDecisionJournal::record_commit(&mut journal, &decision).unwrap();
        drop(journal);
        let journal_path = tmp.path().join("solaris/entity-owner-journal.json");
        let journal_before_bind = std::fs::read(&journal_path).unwrap();

        let error = match bind(save_all_test_config(
            tmp.path(),
            blocks,
            items,
            entity_types,
        ))
        .await
        {
            Ok(_) => panic!("bind accepted duplicate final entity UUID"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert!(error.to_string().contains("duplicate restored entity UUID"));
        assert_eq!(std::fs::read(journal_path).unwrap(), journal_before_bind);
    }

    #[tokio::test]
    async fn active_save_acquires_coordinator_before_owner_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let save_coordinator = config.shutdown.save_coordinator();
        let coordinator = save_coordinator.lock().await;
        let mut save = Box::pin(save_all_after_simulation_barrier(
            "save coordinator ordering test",
            &config,
            &sessions,
            &simulation,
        ));

        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(save.as_mut(), cx).is_pending(),
                "active save must wait for the occupied coordinator"
            );
            std::task::Poll::Ready(())
        })
        .await;

        assert_eq!(
            owner.process_tick(&sessions, 1).processed,
            0,
            "a queued save must not capture its owner snapshot before it owns the coordinator"
        );
        drop(save);
        drop(coordinator);
    }

    #[tokio::test]
    async fn save_coordinator_does_not_serialize_unrelated_servers() {
        let first = ShutdownHandle::default();
        let second = ShutdownHandle::default();
        let first_coordinator = first.save_coordinator();
        let first_guard = first_coordinator.lock().await;
        let second_coordinator = second.save_coordinator();
        let mut second_guard = Box::pin(second_coordinator.lock());

        std::future::poll_fn(|context| match second_guard.as_mut().poll(context) {
            std::task::Poll::Ready(guard) => {
                drop(guard);
                std::task::Poll::Ready(())
            }
            std::task::Poll::Pending => {
                panic!("an unrelated server save coordinator must not wait")
            }
        })
        .await;
        drop(first_guard);
    }

    #[tokio::test]
    async fn active_save_uses_the_ordered_owner_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ]));
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(
            tmp.path(),
            blocks,
            Arc::clone(&items),
            Arc::clone(&entity_types),
        );
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let mut save = std::pin::pin!(save_all_after_simulation_barrier(
            "ordered save test",
            &config,
            &sessions,
            &simulation,
        ));
        let command_ready = tokio::select! {
            report = &mut save => panic!("active save completed before owner snapshot: {report:?}"),
            ready = owner.wait_for_command() => ready,
        };
        assert!(command_ready, "simulation command channel closed");

        assert_eq!(
            owner
                .process_tick_with_world(
                    &sessions,
                    config.world.as_ref(),
                    config.block_light.as_deref(),
                    1,
                )
                .processed,
            1
        );
        sessions.restore_persisted_entities(play::persistence::PersistedEntityCheckpoint::new(
            0,
            vec![play::persistence::PersistedEntityRecord {
                snapshot: mc_entity::EntitySnapshot {
                    id: mc_entity::EntityId(1_000_001),
                    uuid: uuid::Uuid::from_u128(1),
                    type_id: 71,
                    type_name: "minecraft:item".into(),
                    position: mc_entity::Vec3::new(0.5, 64.0, 0.5),
                    rotation: mc_entity::Rotation::ZERO,
                    velocity: mc_entity::Vec3::ZERO,
                    on_ground: true,
                    item_stack: Some(mc_entity::EntityItemStack::new(1, 1)),
                    experience_value: None,
                    block_state: None,
                    lifecycle: mc_entity::EntityLifecycle::Alive,
                    health: 20.0,
                    attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                    goal: mc_entity::GoalState::Idle,
                    vehicle: None,
                    animal: None,
                    retained: mc_entity::EntityRetainedState::default(),
                },
                age: 0,
                pickup_delay: 0,
            }],
        ));
        let report = save.await;

        assert!(report.is_ok(), "save errors: {:?}", report.errors);
        assert_eq!(report.entities_saved, 0);
        assert_eq!(sessions.persisted_entity_records().len(), 1);
        let saved =
            play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();
        assert!(saved.records.is_empty());
    }

    #[tokio::test]
    async fn active_save_world_flush_matches_the_owner_barrier() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(tmp.path(), Arc::clone(&blocks), items, entity_types);
        let world = config.world.as_ref().unwrap();
        let cpos = mc_world::ChunkPos { x: 0, z: 0 };
        let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        {
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    cpos,
                    mc_world::Chunk::empty(
                        cpos,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage
                .set_block_at(pos, mc_world::BlockStateId(1))
                .unwrap();
        }
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let mut save = std::pin::pin!(save_all_after_simulation_barrier(
            "world barrier save test",
            &config,
            &sessions,
            &simulation,
        ));
        let command_ready = tokio::select! {
            report = &mut save => panic!("save completed before owner barrier: {report:?}"),
            ready = owner.wait_for_command() => ready,
        };
        assert!(command_ready);
        assert_eq!(
            owner
                .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
                .processed,
            1
        );

        world
            .lock()
            .await
            .set_block_at(pos, mc_world::BlockStateId(0))
            .unwrap();
        let save_report = save.await;
        assert!(save_report.is_ok(), "save errors: {:?}", save_report.errors);
        assert_eq!(save_report.chunks_flushed, 1);

        let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
        assert_eq!(
            reopened.get_block(pos).unwrap(),
            Some(mc_world::BlockStateId(1)),
            "disk state must match the world at the owner barrier"
        );
        assert_eq!(
            world.lock().await.get_cached_block(pos),
            Some(mc_world::BlockStateId(0)),
            "post-barrier mutation must remain live and dirty"
        );
    }

    #[tokio::test]
    async fn active_save_waits_for_the_exact_world_journal_fence() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            items,
            canonical_entity_types(),
        );
        let world = config.world.as_ref().unwrap();
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        let mutation = {
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(
                        position,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            assert!(matches!(
                storage.stamp_cached_chunks_for_world_journal(41, &[position]),
                mc_world::JournalStampResult::Stamped(_)
            ));
            storage.mutation_view()
        };
        let dirty_tail_progress = config.shutdown.clone();
        world
            .lock()
            .await
            .set_dirty_high_water_notifier(Arc::new(move || {
                dirty_tail_progress.mark_dirty_tail_progress();
            }));

        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let mut save = std::pin::pin!(save_all_after_simulation_barrier(
            "journal-fenced save test",
            &config,
            &sessions,
            &simulation,
        ));
        let command_ready = tokio::select! {
            report = &mut save => panic!("save completed before owner barrier: {report:?}"),
            ready = owner.wait_for_command() => ready,
        };
        assert!(command_ready);
        assert_eq!(
            owner
                .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
                .processed,
            1
        );

        assert_eq!(
            mutation.clear_journal_pending_conditionally(40, &[position]),
            0,
            "a different journal decision must not release the save"
        );
        std::future::poll_fn(|context| {
            assert!(
                std::future::Future::poll(save.as_mut(), context).is_pending(),
                "save acknowledged a journal-fenced dirty chunk"
            );
            std::task::Poll::Ready(())
        })
        .await;

        assert_eq!(
            mutation.clear_journal_pending_conditionally(41, &[position]),
            1
        );
        let command_ready = tokio::select! {
            report = &mut save => panic!("save completed before the replacement barrier: {report:?}"),
            ready = owner.wait_for_command() => ready,
        };
        assert!(
            command_ready,
            "fence release must request a new owner barrier"
        );
        assert_eq!(
            owner
                .process_tick_with_world(&sessions, config.world.as_ref(), None, 2)
                .processed,
            1
        );
        let report = save.await;
        assert!(report.is_ok(), "save errors: {:?}", report.errors);
        assert_eq!(report.chunks_flushed, 1);
        assert_eq!(world.lock().await.dirty_count(), 0);
    }

    #[tokio::test]
    async fn final_save_rejects_an_orphaned_world_journal_fence() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let config = save_all_test_config(
            tmp.path(),
            blocks,
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        );
        let world = config.world.as_ref().unwrap();
        let position = mc_world::ChunkPos { x: 0, z: 0 };
        {
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    position,
                    mc_world::Chunk::empty(
                        position,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            assert!(matches!(
                storage.stamp_cached_chunks_for_world_journal(52, &[position]),
                mc_world::JournalStampResult::Stamped(_)
            ));
        }

        let sessions = play::SessionRegistry::new();
        let report =
            save_all_after_drain_with_context("orphaned journal fence test", &config, &sessions)
                .await;

        assert!(!report.is_ok());
        assert!(
            report
                .errors
                .iter()
                .any(|error| { error.contains("journal-pending chunks after producer drain") })
        );
        assert_eq!(world.lock().await.dirty_count(), 1);
    }

    #[tokio::test]
    async fn last_session_event_enqueues_periodic_checkpoint() {
        let sessions = play::SessionRegistry::new();
        let observed = sessions.session_empty_generation();
        let (flush_started, mut flush_started_receiver) = mpsc::channel(1);
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            || async { panic!("disconnect must not request the dirty-only action") },
            move || {
                let flush_started = flush_started.clone();
                async move {
                    flush_started
                        .send(())
                        .await
                        .expect("test observes full checkpoint request");
                }
            },
        );
        let save_requests = coordinator.notifier();
        let mut request = Box::pin(wait_for_session_empty_save_request(
            &sessions,
            observed,
            Some(&save_requests),
            41,
        ));
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(request.as_mut(), cx).is_pending(),
                "checkpoint request must wait for the last session event"
            );
            std::task::Poll::Ready(())
        })
        .await;

        sessions.mark_session_empty_for_test();
        assert_eq!(request.await, observed + 1);
        assert_eq!(flush_started_receiver.recv().await, Some(()));
        coordinator.drain().await;
    }

    #[tokio::test]
    async fn runtime_dirty_high_water_drains_tail_across_bounded_actions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[report("minecraft:air", &[], &[(0, true, &[])])]).unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let world = Arc::new(Mutex::new(
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&blocks), 65)
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let mut config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            items,
            canonical_entity_types(),
        );
        config.world = Some(Arc::clone(&world));
        let config = Arc::new(config);
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = crate::dirty_flush::DirtyFlushCoordinator::spawn_actions(
            {
                let config = Arc::clone(&config);
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let config = Arc::clone(&config);
                    let dirty_calls = Arc::clone(&dirty_calls);
                    async move {
                        dirty_calls.fetch_add(1, Ordering::SeqCst);
                        log_dirty_only_flush(
                            "runtime dirty-only flush test",
                            flush_dirty_chunks_only(&config, 41).await,
                        )
                    }
                }
            },
            || async { panic!("dirty high water must not run a full checkpoint") },
        );
        let dirty_flush = coordinator.notifier();
        world
            .lock()
            .await
            .set_dirty_high_water_notifier(Arc::new(move || {
                dirty_flush.request_dirty_flush();
            }));
        let biome = Identifier::parse("minecraft:plains").unwrap();
        {
            let mut storage = world.lock().await;
            for x in 0..=DIRTY_ONLY_FLUSH_MAX_CHUNKS as i32 {
                let position = mc_world::ChunkPos { x, z: 0 };
                storage
                    .insert_generated_chunk(
                        position,
                        mc_world::Chunk::empty(position, mc_world::BlockStateId(0), biome.clone()),
                    )
                    .unwrap();
            }
        }

        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
        assert_eq!(world.lock().await.stats().dirty_chunks, 0);
        assert!(
            play::persistence::load_world_metadata(tmp.path())
                .unwrap()
                .is_none(),
            "runtime dirty-only flush must exclude full-checkpoint metadata"
        );
    }

    #[tokio::test]
    async fn periodic_checkpoint_persists_ordered_owner_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ]));
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
            Arc::clone(&entity_types),
        );
        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        {
            let mut world = config.world.as_ref().unwrap().lock().await;
            let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
            world
                .insert_generated_chunk(
                    chunk_pos,
                    mc_world::Chunk::empty(
                        chunk_pos,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            world
                .set_block_at(position, mc_world::BlockStateId(1))
                .unwrap();
        }
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(73);
        let (simulation, mut owner) = play::simulation_channel();
        let mut retained = mc_entity::EntityRetainedState::default();
        retained.item_pickup_ready_tick = Some(13);
        assert_eq!(
            owner.restore_persisted_entities(
                &sessions,
                play::persistence::PersistedEntityCheckpoint::new(
                    11,
                    vec![play::persistence::PersistedEntityRecord {
                        snapshot: mc_entity::EntitySnapshot {
                            id: mc_entity::EntityId(1_000_003),
                            uuid: uuid::Uuid::from_u128(3),
                            type_id: 71,
                            type_name: "minecraft:item".into(),
                            position: mc_entity::Vec3::new(1.5, 64.0, 2.5),
                            rotation: mc_entity::Rotation::ZERO,
                            velocity: mc_entity::Vec3::ZERO,
                            on_ground: true,
                            item_stack: Some(mc_entity::EntityItemStack::new(1, 3)),
                            experience_value: None,
                            block_state: None,
                            lifecycle: mc_entity::EntityLifecycle::Alive,
                            health: 20.0,
                            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                            goal: mc_entity::GoalState::Idle,
                            vehicle: None,
                            animal: None,
                            retained,
                        },
                        age: 11,
                        pickup_delay: 2,
                    },]
                ),
            ),
            1
        );

        let shutdown = ShutdownHandle::default();
        let mut save = std::pin::pin!(save_periodic_checkpoint(
            &config,
            &sessions,
            &simulation,
            &shutdown,
        ));
        let command_ready = tokio::select! {
            report = &mut save => {
                panic!("periodic checkpoint completed before owner snapshot: {report:?}")
            }
            ready = owner.wait_for_command() => ready,
        };
        assert!(command_ready, "simulation command channel closed");
        assert_eq!(
            owner
                .process_tick_with_world(&sessions, config.world.as_ref(), None, 9)
                .processed,
            1
        );

        let report = save.await.expect("checkpoint is not superseded");

        assert!(report.is_ok(), "checkpoint errors: {:?}", report.errors);
        assert_eq!(report.entities_saved, 1);
        assert_eq!(report.chunks_flushed, 1);
        assert!(report.world_metadata_saved);
        let saved = play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types)
            .unwrap()
            .records;
        assert_eq!(saved.len(), 1);
        assert_eq!(
            saved[0].item_stack,
            Some(mc_entity::EntityItemStack::new(1, 3))
        );
        assert_eq!(saved[0].age, 11);
        assert_eq!(saved[0].pickup_delay, 2);
        let metadata = play::persistence::load_world_metadata(tmp.path())
            .unwrap()
            .unwrap();
        assert_eq!(metadata.world_time, 73);
        let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
        assert_eq!(
            reopened.get_block(position).unwrap(),
            Some(mc_world::BlockStateId(1))
        );
    }

    #[tokio::test]
    async fn periodic_checkpoint_is_superseded_by_shutdown_before_owner_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let config = save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            items,
            canonical_entity_types(),
        );
        let shutdown = ShutdownHandle::default();
        let sessions = play::SessionRegistry::new();
        let (simulation, mut owner) = play::simulation_channel();
        let mut save = std::pin::pin!(save_periodic_checkpoint(
            &config,
            &sessions,
            &simulation,
            &shutdown,
        ));

        let command_ready = tokio::select! {
            report = &mut save => {
                panic!("periodic checkpoint completed before owner snapshot: {report:?}")
            }
            ready = owner.wait_for_command() => ready,
        };
        assert!(command_ready, "simulation command channel closed");

        shutdown.request();
        assert!(save.await.is_none());
        owner.shutdown();
    }

    #[tokio::test]
    async fn save_all_reports_zero_entities_when_entity_write_fails() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(tmp.path(), blocks, items, entity_types);
        let sessions = play::SessionRegistry::new();
        sessions.restore_persisted_entities(play::persistence::PersistedEntityCheckpoint::new(
            0,
            vec![play::persistence::PersistedEntityRecord {
                snapshot: mc_entity::EntitySnapshot {
                    id: mc_entity::EntityId(1_000_004),
                    uuid: uuid::Uuid::from_u128(4),
                    type_id: 71,
                    type_name: "minecraft:item".into(),
                    position: mc_entity::Vec3::new(0.5, 64.0, 0.5),
                    rotation: mc_entity::Rotation::ZERO,
                    velocity: mc_entity::Vec3::ZERO,
                    on_ground: true,
                    item_stack: Some(mc_entity::EntityItemStack::new(99, 1)),
                    experience_value: None,
                    block_state: None,
                    lifecycle: mc_entity::EntityLifecycle::Alive,
                    health: 20.0,
                    attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                    goal: mc_entity::GoalState::Idle,
                    vehicle: None,
                    animal: None,
                    retained: mc_entity::EntityRetainedState::default(),
                },
                age: 0,
                pickup_delay: 0,
            }],
        ));

        let report = save_all(&config, &sessions).await;

        assert!(!report.is_ok());
        assert_eq!(report.entities_saved, 0);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("entities: save failed"))
        );
    }

    #[tokio::test]
    async fn save_all_writes_entities_and_world_metadata_to_real_storage() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ]));
        let entity_types = canonical_entity_types();
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(42);
        let mut retained = mc_entity::EntityRetainedState::default();
        retained.item_pickup_ready_tick = Some(15);
        let checkpoint = play::persistence::PersistedEntityCheckpoint::new(
            12,
            vec![play::persistence::PersistedEntityRecord {
                snapshot: mc_entity::EntitySnapshot {
                    id: mc_entity::EntityId(1_000_001),
                    uuid: uuid::Uuid::from_u128(1),
                    type_id: 71,
                    type_name: "minecraft:item".into(),
                    position: mc_entity::Vec3::new(1.0, 2.0, 3.0),
                    rotation: mc_entity::Rotation::ZERO,
                    velocity: mc_entity::Vec3::ZERO,
                    on_ground: true,
                    item_stack: Some(mc_entity::EntityItemStack::new(1, 2)),
                    experience_value: None,
                    block_state: None,
                    lifecycle: mc_entity::EntityLifecycle::Alive,
                    health: 20.0,
                    attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                    goal: mc_entity::GoalState::Idle,
                    vehicle: None,
                    animal: None,
                    retained,
                },
                age: 12,
                pickup_delay: 3,
            }],
        );
        assert_eq!(sessions.restore_persisted_entities(checkpoint), 1);
        let config = save_all_test_config(
            tmp.path(),
            blocks,
            Arc::clone(&items),
            Arc::clone(&entity_types),
        );

        let report = save_all(&config, &sessions).await;

        assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
        assert_eq!(report.entities_saved, 1);
        assert!(report.world_metadata_saved);
        let entities =
            play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types)
                .unwrap()
                .records;
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0].item_stack,
            Some(mc_entity::EntityItemStack::new(1, 2))
        );
        assert_eq!(entities[0].age, 12);
        assert_eq!(entities[0].pickup_delay, 3);
        let metadata = play::persistence::load_world_metadata(tmp.path())
            .unwrap()
            .unwrap();
        assert_eq!(metadata.world_time, 42);
    }

    #[tokio::test]
    async fn save_all_then_bind_restores_world_time_and_item_entities() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ]));
        let entity_types = canonical_entity_types();
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(99);
        sessions.set_daylight_cycle_enabled(false);
        sessions.set_players_sleeping_percentage(50);
        let mut retained = mc_entity::EntityRetainedState::default();
        retained.item_pickup_ready_tick = Some(12);
        let checkpoint = play::persistence::PersistedEntityCheckpoint::new(
            8,
            vec![play::persistence::PersistedEntityRecord {
                snapshot: mc_entity::EntitySnapshot {
                    id: mc_entity::EntityId(1_000_002),
                    uuid: uuid::Uuid::from_u128(2),
                    type_id: 71,
                    type_name: "minecraft:item".into(),
                    position: mc_entity::Vec3::new(4.0, 5.0, 6.0),
                    rotation: mc_entity::Rotation::ZERO,
                    velocity: mc_entity::Vec3::ZERO,
                    on_ground: true,
                    item_stack: Some(mc_entity::EntityItemStack::new(1, 5)),
                    experience_value: None,
                    block_state: None,
                    lifecycle: mc_entity::EntityLifecycle::Alive,
                    health: 20.0,
                    attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
                    goal: mc_entity::GoalState::Idle,
                    vehicle: None,
                    animal: None,
                    retained,
                },
                age: 8,
                pickup_delay: 4,
            }],
        );
        assert_eq!(sessions.restore_persisted_entities(checkpoint), 1);
        let save_config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
            Arc::clone(&entity_types),
        );

        let report = save_all(&save_config, &sessions).await;
        assert!(report.is_ok(), "save-all errors: {:?}", report.errors);

        let bound = bind(save_all_test_config(
            tmp.path(),
            blocks,
            items,
            entity_types,
        ))
        .await
        .unwrap();
        assert_eq!(bound.sessions.world_time(), 99);
        assert!(!bound.sessions.daylight_cycle_enabled());
        assert_eq!(bound.sessions.players_sleeping_percentage(), 50);
        let records = bound.sessions.persisted_entity_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, mc_entity::EntityId(1_000_002));
        assert_eq!(
            records[0].item_stack,
            Some(mc_entity::EntityItemStack::new(1, 5))
        );
        assert_eq!(records[0].age, 8);
        assert_eq!(records[0].pickup_delay, 4);
    }

    #[tokio::test]
    async fn bind_replays_world_chunk_journal_and_clean_save_checkpoints_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let mut chunk = mc_world::Chunk::empty(
            chunk_position,
            mc_world::BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk
            .set_block(1, 64, 1, mc_world::BlockStateId(1))
            .unwrap();
        chunk
            .extras
            .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));

        let (journal, pending) = play::world_journal::WorldChunkJournal::open(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        assert_eq!(
            journal.record_snapshots(12, vec![Arc::new(chunk)]).unwrap(),
            1
        );
        drop(journal);

        let bound = bind(save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
            Arc::clone(&entity_types),
        ))
        .await
        .unwrap();
        assert_eq!(
            bound
                .config
                .world
                .as_ref()
                .unwrap()
                .lock()
                .await
                .get_cached_block(position),
            Some(mc_world::BlockStateId(1))
        );
        assert_eq!(bound.sessions.world_chunk_journal_watermark(), Some(1));

        let report = save_all(&bound.config, &bound.sessions).await;
        assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
        assert_eq!(bound.sessions.world_chunk_journal_watermark(), None);
        drop(bound);

        let (_, pending) =
            play::world_journal::WorldChunkJournal::open(tmp.path(), blocks, items).unwrap();
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn bind_skips_pending_world_journal_image_at_disk_lsn() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();

        let mut image_a =
            mc_world::Chunk::empty(chunk_position, mc_world::BlockStateId(0), biome.clone());
        image_a
            .set_block(1, 64, 1, mc_world::BlockStateId(1))
            .unwrap();
        image_a
            .extras
            .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));
        let (journal, pending) = play::world_journal::WorldChunkJournal::open(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
        )
        .unwrap();
        assert!(pending.is_empty());
        assert_eq!(
            journal
                .record_snapshots(12, vec![Arc::new(image_a)])
                .unwrap(),
            1
        );
        drop(journal);

        let mut image_b = mc_world::Chunk::empty(chunk_position, mc_world::BlockStateId(0), biome);
        image_b
            .set_block(1, 64, 1, mc_world::BlockStateId(1))
            .unwrap();
        image_b
            .set_block(1, 64, 1, mc_world::BlockStateId(0))
            .unwrap();
        image_b
            .extras
            .push(("SolarisJournalLsn".to_owned(), mc_nbt::Tag::Long(1)));
        image_b.mark_dirty();
        let mut storage = WorldStorage::open(tmp.path(), Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        storage
            .commit_chunk_snapshot(chunk_position, image_b)
            .unwrap();
        assert_eq!(storage.flush_dirty().unwrap(), 1);
        drop(storage);

        let bound = bind(save_all_test_config(
            tmp.path(),
            blocks,
            items,
            entity_types,
        ))
        .await
        .unwrap();
        let storage = bound.config.world.as_ref().unwrap().lock().await;
        assert_eq!(
            storage.get_cached_block(position),
            Some(mc_world::BlockStateId(0)),
            "disk image B at the matching LSN must win over pending journal image A"
        );
        assert_eq!(
            storage
                .cached_chunk_snapshot(chunk_position)
                .unwrap()
                .world_journal_lsn(),
            1
        );
    }

    #[tokio::test]
    async fn failed_world_flush_keeps_world_chunk_journal_pending() {
        let tmp = tempfile::tempdir().unwrap();
        let region_root = tmp.path().join("region");
        std::fs::create_dir_all(&region_root).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report("minecraft:stone", &[], &[(1, true, &[])]),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            Arc::clone(&items),
            entity_types,
        );
        let chunk_position = mc_world::ChunkPos { x: 0, z: 0 };
        let snapshot = {
            let mut storage = config.world.as_ref().unwrap().lock().await;
            storage
                .insert_generated_chunk(
                    chunk_position,
                    mc_world::Chunk::empty(
                        chunk_position,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage
                .set_block_at(
                    mc_world::BlockPos { x: 1, y: 64, z: 1 },
                    mc_world::BlockStateId(1),
                )
                .unwrap();
            storage.cached_chunk_snapshot(chunk_position).unwrap()
        };
        let sessions = play::SessionRegistry::new();
        let (journal, pending) =
            play::world_journal::WorldChunkJournal::open(tmp.path(), blocks, items).unwrap();
        assert!(pending.is_empty());
        assert_eq!(journal.record_snapshots(1, vec![snapshot]).unwrap(), 1);
        sessions.install_world_chunk_journal(journal);
        std::fs::remove_dir(&region_root).unwrap();
        std::fs::write(&region_root, b"blocks region writes").unwrap();

        let report = save_all(&config, &sessions).await;

        assert!(!report.is_ok());
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("dirty chunks:")),
            "save errors: {:?}",
            report.errors
        );
        assert_eq!(sessions.world_chunk_journal_watermark(), Some(1));
    }

    #[tokio::test]
    async fn bind_rejects_corrupt_world_metadata_without_overwriting_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let metadata_path = tmp.path().join("solaris/world.dat");
        std::fs::create_dir_all(metadata_path.parent().unwrap()).unwrap();
        let corrupt = b"not gzip nbt";
        std::fs::write(&metadata_path, corrupt).unwrap();

        let err = match bind(save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        ))
        .await
        {
            Ok(_) => panic!("bind accepted corrupt world metadata"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("world metadata load failed"));
        assert_eq!(std::fs::read(metadata_path).unwrap(), corrupt);
    }

    #[tokio::test]
    async fn bind_rejects_corrupt_entities_without_overwriting_them() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let entities_path = tmp.path().join("solaris/entities.dat");
        std::fs::create_dir_all(entities_path.parent().unwrap()).unwrap();
        let corrupt = b"not gzip nbt";
        std::fs::write(&entities_path, corrupt).unwrap();

        let err = match bind(save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        ))
        .await
        {
            Ok(_) => panic!("bind accepted corrupt persisted entities"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("persisted entity load failed"));
        assert_eq!(std::fs::read(entities_path).unwrap(), corrupt);
    }

    #[tokio::test]
    async fn bind_rejects_world_metadata_from_another_world() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        play::persistence::save_world_metadata(
            tmp.path(),
            &play::persistence::WorldPersistedMetadata {
                world_time: 77,
                daylight_cycle_enabled: true,
                players_sleeping_percentage: 100,
                keep_inventory: false,
                world_identity: "different-world".into(),
            },
        )
        .unwrap();

        let err = match bind(save_all_test_config(
            tmp.path(),
            Arc::new(BlockRegistry::from_report(&[]).unwrap()),
            Arc::new(mc_data::items::ItemRegistry::default()),
            canonical_entity_types(),
        ))
        .await
        {
            Ok(_) => panic!("bind accepted metadata from another world"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("world metadata identity mismatch"));
    }

    #[tokio::test]
    async fn save_all_persists_scheduled_fluid_ticks_as_remaining_restart_delay() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(
            BlockRegistry::from_report(&[
                report("minecraft:air", &[], &[(0, true, &[])]),
                report(
                    "minecraft:water",
                    &[("level", &["0", "1"])],
                    &[(1, true, &[("level", "0")]), (2, false, &[("level", "1")])],
                ),
            ])
            .unwrap(),
        );
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[]));
        let entity_types = canonical_entity_types();
        let sessions = play::SessionRegistry::new();
        sessions.advance_world_time(100);
        let config = save_all_test_config(
            tmp.path(),
            Arc::clone(&blocks),
            items,
            Arc::clone(&entity_types),
        );
        let cpos = mc_world::ChunkPos { x: 0, z: 0 };
        let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let water = Identifier::parse("minecraft:water").unwrap();
        {
            let world = config.world.as_ref().unwrap();
            let mut storage = world.lock().await;
            storage
                .insert_generated_chunk(
                    cpos,
                    mc_world::Chunk::empty(
                        cpos,
                        mc_world::BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage
                .set_block_at(pos, mc_world::BlockStateId(1))
                .unwrap();
            assert!(
                storage
                    .schedule_fluid_tick(mc_world::ScheduledFluidTick::new(
                        pos,
                        water.clone(),
                        112,
                        0,
                    ))
                    .unwrap()
            );
        }

        let report = save_all(&config, &sessions).await;

        assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
        drop(config);

        let mut reopened = WorldStorage::open(tmp.path(), blocks).unwrap();
        let ticks = reopened.scheduled_fluid_ticks(cpos).unwrap().unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].pos, pos);
        assert_eq!(ticks[0].fluid, water);
        assert_eq!(
            ticks[0].trigger_tick, 12,
            "fresh runtimes must reload persisted fluid ticks as remaining delay"
        );
    }

    #[test]
    fn public_bind_detection_is_conservative() {
        assert!(!is_public_bind("127.0.0.1:25565".parse().unwrap()));
        assert!(!is_public_bind("10.0.0.1:25565".parse().unwrap()));
        assert!(!is_public_bind("192.168.1.5:25565".parse().unwrap()));
        assert!(!is_public_bind("[::ffff:127.0.0.1]:25565".parse().unwrap()));
        assert!(is_public_bind("0.0.0.0:25565".parse().unwrap()));
        assert!(is_public_bind("8.8.8.8:25565".parse().unwrap()));
    }

    #[test]
    fn public_security_rejects_offline_auth() {
        let err = validate_public_security_config(
            "0.0.0.0:25565".parse().unwrap(),
            &CommandPermissionConfig::new(Vec::<String>::new(), false),
        )
        .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("offline-mode"));
    }

    #[test]
    fn public_security_rejects_local_dev_operator_fallback() {
        let permissions = CommandPermissionConfig::new(Vec::<String>::new(), true)
            .with_login_access(login::LoginAccessConfig::normalized(
                true,
                false,
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
            ));
        let err = validate_public_security_config("8.8.8.8:25565".parse().unwrap(), &permissions)
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("allow_local_dev_operators"));
    }

    #[test]
    fn public_security_allows_online_mode() {
        let permissions = CommandPermissionConfig::new(["Notch"], false).with_login_access(
            login::LoginAccessConfig::normalized(
                true,
                false,
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
            ),
        );
        validate_public_security_config("8.8.8.8:25565".parse().unwrap(), &permissions).unwrap();
    }

    #[test]
    fn private_security_allows_local_offline_dev() {
        validate_public_security_config(
            "127.0.0.1:25565".parse().unwrap(),
            &CommandPermissionConfig::new(Vec::<String>::new(), true),
        )
        .unwrap();
    }

    #[test]
    fn local_dev_operator_fallback_requires_loopback_peer() {
        let profile = login::LoggedInProfile {
            uuid: uuid::Uuid::nil(),
            name: "LanPlayer".into(),
        };
        let fallback = CommandPermissionConfig::new(Vec::<String>::new(), true);

        assert_eq!(
            fallback.permissions_for(&profile, "127.0.0.1:40000".parse().unwrap()),
            play::commands::CommandPermissions::from_op(true)
        );
        assert_eq!(
            fallback.permissions_for(&profile, "[::ffff:127.0.0.1]:40000".parse().unwrap()),
            play::commands::CommandPermissions::from_op(true)
        );
        assert_eq!(
            fallback.permissions_for(&profile, "192.168.1.20:40000".parse().unwrap()),
            play::commands::CommandPermissions::from_op(false)
        );

        let explicit = CommandPermissionConfig::new(["  LanPlayer  ", "  "], true);
        assert_eq!(
            explicit.permissions_for(&profile, "192.168.1.20:40000".parse().unwrap()),
            play::commands::CommandPermissions::from_op(true)
        );
    }
}
