//! TCP listener, accept loop, and server supervision.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::io::ErrorKind;
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
use tokio::sync::{Mutex, Notify, Semaphore};
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

mod checkpoint;
mod entity_physics;
mod entity_ticker;
mod natural_spawn_ticker;
mod operator_control;
mod runtime_control;

pub(super) use checkpoint::*;
use entity_physics::*;

pub use operator_control::{OperatorControlHandle, OperatorWeather};

use runtime_control::apply_runtime_control_decision;

type PhysicsMaterialCache = HashMap<
    (usize, usize),
    (
        std::sync::Weak<BlockRegistry>,
        std::sync::Weak<BlockFactsTable>,
        Arc<BlockMaterialIds>,
    ),
>;

static PHYSICS_MATERIAL_CACHE: OnceLock<std::sync::Mutex<PhysicsMaterialCache>> = OnceLock::new();
type CollisionDirectLookupCache = HashMap<usize, (std::sync::Weak<BlockRegistry>, bool)>;

static COLLISION_DIRECT_LOOKUP_CACHE: OnceLock<std::sync::Mutex<CollisionDirectLookupCache>> =
    OnceLock::new();
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
    /// Live operator identities (lowercase name or UUID). Shared with the
    /// console so grants and revocations apply without a server restart.
    operators: Arc<arc_swap::ArcSwap<BTreeSet<String>>>,
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
            operators: Arc::new(arc_swap::ArcSwap::from_pointee(normalize_identities(
                operators,
            ))),
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

    /// Re-resolve operator authority for an already connected player.
    ///
    /// The loopback developer fallback uses the actual connection peer,
    /// not the login-time operator bit, which may have since been revoked.
    #[must_use]
    pub(crate) fn live_permissions_for(
        &self,
        name: &str,
        uuid: &str,
        peer: SocketAddr,
    ) -> play::commands::CommandPermissions {
        self.live_permissions_for_normalized(
            &name.to_ascii_lowercase(),
            &uuid.to_ascii_lowercase(),
            peer,
        )
    }

    /// The caller holds normalized identifiers derived from the verified profile.
    /// Movement observations can reuse them without allocating on every packet.
    pub(crate) fn live_permissions_for_normalized(
        &self,
        name: &str,
        uuid: &str,
        peer: SocketAddr,
    ) -> play::commands::CommandPermissions {
        let operators = self.operators.load();
        let listed = operators.contains(name) || operators.contains(uuid);
        play::commands::CommandPermissions::from_op(
            listed
                || (operators.is_empty()
                    && self.allow_local_dev_operators
                    && is_loopback_peer(peer)),
        )
    }

    #[must_use]
    fn is_operator(&self, profile: &login::LoggedInProfile, peer: SocketAddr) -> bool {
        let operators = self.operators.load();
        if operators.is_empty() && self.allow_local_dev_operators && is_loopback_peer(peer) {
            return true;
        }
        operators.contains(&profile.name.to_ascii_lowercase())
            || operators.contains(&profile.uuid.to_string().to_ascii_lowercase())
    }

    pub(crate) fn login_access(&self) -> &login::LoginAccessConfig {
        &self.login_access
    }

    /// Shared live operator identities for runtime console mutation.
    pub(crate) fn operator_identities(&self) -> Arc<arc_swap::ArcSwap<BTreeSet<String>>> {
        Arc::clone(&self.operators)
    }

    /// Shared live whitelist identities for runtime console mutation.
    pub(crate) fn whitelist_identities(&self) -> Arc<arc_swap::ArcSwap<BTreeSet<String>>> {
        Arc::clone(&self.login_access.whitelist)
    }
}

fn normalize_identities<I, S>(entries: I) -> BTreeSet<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    entries
        .into_iter()
        .map(Into::into)
        .filter_map(|entry| {
            let normalized = entry.trim().to_ascii_lowercase();
            (!normalized.is_empty()).then_some(normalized)
        })
        .collect()
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

    pub(crate) fn save_coordinator(&self) -> Arc<Mutex<()>> {
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
/// Player-list (tab menu) header/footer text from the `[tab_list]` server
/// config section. Both default to empty, which is the vanilla default
/// (no header/footer lines shown); the Play handler only sends the
/// `ClientboundTabList` packet when at least one side is non-empty.
#[derive(Clone, Default)]
pub struct TabListConfig {
    pub header: String,
    pub footer: String,
}

#[derive(Clone)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    pub motd: String,
    pub max_players: u32,
    pub tab_list: TabListConfig,
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
    pub fn seed_spawn_entities(
        &self,
        entities: Vec<mc_entity::SpawnEntity>,
    ) -> LoadBenchSeedReport {
        let seeded = self.sessions.seed_load_bench_spawn_entities(entities);
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
    pub fn seed_natural_entities(
        &self,
        entities: Vec<mc_entity::SpawnEntity>,
    ) -> LoadBenchSeedReport {
        let seeded = self.sessions.seed_load_bench_natural_entities(entities);
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
    profile_world: Option<WorldHandle>,
    profile_read: Option<mc_world::WorldReadView>,
    profile_resources: ChunkPipelineResources,
    profile_blocks: Arc<BlockRegistry>,
}

mod resource_snapshot;

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
pub(crate) struct ScriptEventSink {
    boundary: ScriptBoundary,
}

impl ScriptEventSink {
    pub(crate) fn new(boundary: ScriptBoundary) -> Self {
        Self { boundary }
    }

    pub(crate) fn boundary(&self) -> &ScriptBoundary {
        &self.boundary
    }

    pub(crate) fn enqueue_custom_payload(
        &self,
        player_id: mc_script::ScriptPlayerId,
        phase: mc_script::ScriptProtocolPhase,
        channel: &str,
        payload: Vec<u8>,
    ) {
        if let Err(error) = self
            .boundary
            .try_enqueue_custom_payload(player_id, phase, channel, payload)
        {
            debug!(channel, ?phase, ?error, "script custom payload rejected");
        }
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

    pub(crate) fn plugin_is_active(&self, plugin_id: &str) -> bool {
        self.boundary.plugin_is_active(plugin_id)
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

struct ScriptCommitWorkers {
    worker: Option<tokio::task::JoinHandle<Result<(), ScriptCommitForwardError>>>,
    failure_watcher: Option<tokio::task::JoinHandle<()>>,
}

async fn forward_committed_script_events(
    mut events: mc_script::ScriptCommitEventReceiver,
    scripts: ScriptEventSink,
) -> Result<(), ScriptCommitForwardError> {
    while let Some(event) = events.recv().await {
        match tokio::time::timeout(
            SCRIPT_COMMIT_FORWARD_TIMEOUT,
            scripts.enqueue_required_event(event),
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
    Ok(())
}

async fn watch_script_commit_event_failure(
    monitor: Arc<mc_script::ScriptCommitEventMonitor>,
    shutdown: ShutdownHandle,
) {
    tokio::select! {
        () = monitor.wait_for_failure() => {
            warn!("required committed script event delivery failed; requesting shutdown");
            shutdown.request();
        }
        () = shutdown.notified() => {}
    }
}

fn spawn_script_commit_workers(
    scripts: Option<ScriptEventSink>,
    sessions: &play::SessionRegistry,
    shutdown: &ShutdownHandle,
) -> ScriptCommitWorkers {
    let Some(scripts) = scripts else {
        return ScriptCommitWorkers {
            worker: None,
            failure_watcher: None,
        };
    };
    sessions.install_precommit_boundary(scripts.boundary().clone());
    let events = sessions.install_script_commit_event_outbox();
    let failure = sessions.script_commit_event_monitor();
    let failure_shutdown = shutdown.clone();
    ScriptCommitWorkers {
        worker: Some(tokio::spawn(forward_committed_script_events(
            events, scripts,
        ))),
        failure_watcher: Some(tokio::spawn(watch_script_commit_event_failure(
            failure,
            failure_shutdown,
        ))),
    }
}

async fn drain_script_commit_runtime(
    sessions: &play::SessionRegistry,
    scripts: Option<&ScriptEventSink>,
    workers: ScriptCommitWorkers,
) -> (std::io::Result<()>, std::io::Result<()>) {
    sessions.close_script_commit_event_outbox();
    let mut script_commit_event_drain_result = match workers.worker {
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
    if let Some(watcher) = workers.failure_watcher {
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
    let server_stopping_event_result = if let Some(scripts) = scripts {
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
    (
        script_commit_event_drain_result,
        server_stopping_event_result,
    )
}

async fn entity_world_context(
    config: &ServerConfig,
) -> (
    Option<std::path::PathBuf>,
    Option<mc_world::ScheduledTickView>,
) {
    let Some(world) = config.world.as_ref() else {
        return (None, None);
    };
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
}

fn spawn_periodic_save_coordinator(
    enabled: bool,
    entity_config: &Arc<ServerConfig>,
    entity_sessions: &Arc<play::SessionRegistry>,
    simulation: &play::SimulationHandle,
    shutdown: &ShutdownHandle,
) -> (
    Option<crate::dirty_flush::DirtyFlushNotifier>,
    Option<crate::dirty_flush::DirtyFlushCoordinator>,
) {
    if !enabled {
        return (None, None);
    }
    let periodic_config = Arc::clone(entity_config);
    let periodic_sessions = Arc::clone(entity_sessions);
    let periodic_simulation = simulation.clone();
    let periodic_shutdown = shutdown.clone();
    let dirty_config = Arc::clone(entity_config);
    let dirty_sessions = Arc::clone(entity_sessions);
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
                    save_periodic_checkpoint(&config, &sessions, &simulation, &shutdown).await
                else {
                    return;
                };
                log_save_report("periodic checkpoint", &report);
            }
        },
    );
    (Some(worker.notifier()), Some(worker))
}

async fn install_dirty_high_water_notifier(
    config: &ServerConfig,
    dirty_flush: Option<&crate::dirty_flush::DirtyFlushNotifier>,
) {
    let (Some(world), Some(dirty_flush)) = (config.world.as_ref(), dirty_flush) else {
        return;
    };
    let dirty_flush = dirty_flush.clone();
    let dirty_tail_progress = config.shutdown.clone();
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

/// Read-only operator-facing session facts for the optional dashboard.
#[derive(Clone)]
pub struct OperatorFactsHandle {
    sessions: Arc<play::SessionRegistry>,
}

impl OperatorFactsHandle {
    /// Bounded point-in-time online player names and truncation flag.
    #[must_use]
    pub fn online_player_names(&self, limit: usize) -> (Vec<String>, bool) {
        self.sessions.online_player_names(limit)
    }

    /// Server entity counts by tracked category.
    #[must_use]
    pub fn entity_category_counts(&self) -> std::collections::BTreeMap<String, u64> {
        self.sessions.entity_category_counts()
    }

    /// Most recently completed save report, when one exists.
    #[must_use]
    pub fn last_save_report(&self) -> Option<crate::operator_metrics::RetainedSaveReport> {
        self.sessions.retained_save_report()
    }

    /// Most recently surfaced cumulative natural-spawn report, when one exists.
    #[must_use]
    pub fn natural_spawn_report(
        &self,
    ) -> Option<crate::operator_metrics::RetainedNaturalSpawnReport> {
        self.sessions.retained_natural_spawn_report()
    }
}

/// The live sessions of whichever bound server has published them.
///
/// The caller that must resolve a stable player identity to the session that
/// identity holds right now - a component host, which needs its lookup before
/// the server that owns the sessions exists - creates this handle first, keeps
/// a clone of it, and publishes a bound server into it with
/// [`BoundServer::register_player_sessions`]. Queries then answer from that
/// server's own session registry, which is the authority that also answers
/// `list-online-players`, so the two can never disagree.
///
/// The handle keeps no session table of its own: it never invents a session and
/// never re-derives one from anything but the published registry.
///
/// * An unpublished handle, and a handle whose published registry is no longer
///   alive, answers nobody.
/// * After a re-bind, it answers the server published last, not the one it
///   answered before.
/// * A player who is not connected right now is answered as absent, never with
///   the session they used to hold, including in the window between a
///   connection ending and the registry tearing its session down.
/// * Two connected players cannot share an identity: the registry refuses a
///   second session for a uuid or name it already holds, so one identity never
///   resolves to a choice of sessions.
#[derive(Clone, Debug)]
pub struct PlayerSessionsHandle {
    published: Arc<arc_swap::ArcSwap<Option<std::sync::Weak<play::SessionRegistry>>>>,
}

impl PlayerSessionsHandle {
    /// An empty handle: it answers nobody until a server publishes into it.
    #[must_use]
    pub fn new() -> Self {
        Self {
            published: Arc::new(arc_swap::ArcSwap::from_pointee(None)),
        }
    }

    /// The session `player` holds right now, or `None` when that player is not
    /// connected or no server publishes live sessions through this handle.
    ///
    /// `player` is the stable identity a plugin addresses - the player uuid the
    /// server hands out in its player contexts, not a username and not a
    /// session id.
    #[must_use]
    pub fn session_of(&self, player: &str) -> Option<u64> {
        let published = self.published.load();
        let sessions = match published.as_ref() {
            Some(sessions) => sessions.upgrade()?,
            None => return None,
        };
        sessions.script_session_of_identity(player)
    }

    /// Answer from `sessions` from now on, replacing whichever server was
    /// published before it.
    fn publish(&self, sessions: &Arc<play::SessionRegistry>) {
        self.published
            .store(Arc::new(Some(Arc::downgrade(sessions))));
    }
}

impl Default for PlayerSessionsHandle {
    fn default() -> Self {
        Self::new()
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

fn spawn_admitted_connection(
    connections: &mut tokio::task::JoinSet<()>,
    socket: tokio::net::TcpStream,
    peer: SocketAddr,
    services: &ConnectionServices,
    connection_permits: &Arc<Semaphore>,
    pre_auth_admission: &PreAuthAdmission,
) {
    let Some(pre_auth_permit) = pre_auth_admission.try_acquire(peer.ip()) else {
        debug!(%peer, "pre-auth admission rejected connection");
        return;
    };
    debug!(%peer, "accepted connection");
    let connection_permit = Arc::clone(connection_permits)
        .try_acquire_owned()
        .expect("accept branch is enabled only while a connection permit exists");
    let services = services.clone();
    connections.spawn(async move {
        let _connection_permit = connection_permit;
        if let Err(err) = Box::pin(crate::resource_profile::measure_future(
            crate::resource_profile::CpuStage::Network,
            handle_connection(socket, peer, services, pre_auth_permit),
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

fn configured_entity_type_id(config: &ServerConfig, name: &str) -> Option<i32> {
    Identifier::parse(name)
        .ok()
        .and_then(|id| config.entity_types.id_of(&id))
        .and_then(|id| i32::try_from(id).ok())
}

fn villager_population_ids(
    config: &ServerConfig,
) -> Option<(
    mc_entity::villager_population_26_1_2::VillagerFoodItemIds,
    i32,
    i32,
)> {
    let item_id = |name: &str| {
        Identifier::parse(name)
            .ok()
            .and_then(|id| config.items.id_of(&id))
    };
    match (
        item_id("minecraft:bread"),
        item_id("minecraft:potato"),
        item_id("minecraft:carrot"),
        item_id("minecraft:beetroot"),
        configured_entity_type_id(config, "minecraft:villager"),
        configured_entity_type_id(config, "minecraft:item"),
    ) {
        (Some(bread), Some(potato), Some(carrot), Some(beetroot), Some(villager), Some(item)) => {
            Some((
                mc_entity::villager_population_26_1_2::VillagerFoodItemIds {
                    bread,
                    potato,
                    carrot,
                    beetroot,
                },
                villager,
                item,
            ))
        }
        _ => None,
    }
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
    pub fn operator_facts_handle(&self) -> OperatorFactsHandle {
        OperatorFactsHandle {
            sessions: Arc::clone(&self.sessions),
        }
    }

    /// Publish this server's live sessions through `handle` from now on,
    /// replacing whichever server `handle` answered for before it.
    ///
    /// The handle is created before this server exists - the component host
    /// that reads it is started before the network binds - so the composition
    /// root publishes the bound server's registry here. A command a plugin
    /// addresses to a stable player identity is then answered with the session
    /// that identity holds right now, read from the same registry that answers
    /// `list-online-players`.
    pub fn register_player_sessions(&self, handle: &PlayerSessionsHandle) {
        handle.publish(&self.sessions);
    }
    #[must_use]
    pub fn runtime_telemetry_handle(&self) -> RuntimeTelemetryHandle {
        RuntimeTelemetryHandle {
            tick_metrics: self.runtime_tick_metrics.clone(),
            sessions: Arc::clone(&self.sessions),
            runtime_control: self.runtime_control.clone(),
            simulation: self.simulation.clone(),
            profile_world: self.config.world.clone(),
            profile_read: self.connection_world.read.clone(),
            profile_resources: self.chunk_pipeline_resources.clone(),
            profile_blocks: Arc::clone(&self.config.blocks),
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
        let physics_warm_started = Instant::now();
        let prewarmed_physics_states = mc_entity::warm_physics_caches();
        let physics_warm_us = physics_warm_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        info!(
            addr = %self.local_addr()?,
            registries = self.config.data.registry_count(),
            entries = self.config.data.entry_count(),
            pathing_states = prewarmed_entity_pathing_states.get(),
            physics_states = prewarmed_physics_states.get(),
            physics_warm_us,
            "Solaris is listening"
        );
        let config = self.config;
        let online_authentication = self.online_authentication;
        let chunk_geometry = self.chunk_geometry;
        let connection_world = self.connection_world;
        let chunk_pipeline_resources = self.chunk_pipeline_resources;
        let runtime_control = self.runtime_control;
        let runtime_control_signals = runtime_control
            .as_ref()
            .and_then(RuntimeControlHandle::take_signal_receiver);
        let runtime_tick_metrics = self.runtime_tick_metrics;
        let sessions = self.sessions;
        let mut entity_owner_failure = sessions.subscribe_entity_owner_failure();
        let simulation = self.simulation;
        let simulation_owner = self.simulation_owner;
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
        let script_commit_workers =
            spawn_script_commit_workers(scripts.clone(), &sessions, &shutdown);
        let mut connections = tokio::task::JoinSet::new();
        let (entity_world_root, entity_scheduled_ticks) = entity_world_context(&config).await;
        let entity_world_read = connection_world.read.clone();
        let entity_world_mutation = connection_world.mutation.clone();
        let entity_pathing_materials = entity_world_read
            .as_ref()
            .map(|_| cached_material_ids(&config));
        let entity_world_journal_failure = sessions.subscribe_world_chunk_journal_failure();
        let (periodic_save_requests, periodic_save_worker) = spawn_periodic_save_coordinator(
            entity_world_root.is_some(),
            &config,
            &sessions,
            &simulation,
            &shutdown,
        );
        install_dirty_high_water_notifier(&config, periodic_save_requests.as_ref()).await;
        if let Some(requests) = periodic_save_requests.as_ref() {
            enqueue_startup_checkpoint(&config, requests).await;
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
            scripts: scripts.clone(),
            script_storage: script_storage.clone(),
            script_zones: script_zones.clone(),
        };
        let (entity_shutdown, entity_shutdown_requested) = tokio::sync::oneshot::channel();
        let mut entity_ticker = tokio::spawn(crate::resource_profile::measure_future(
            crate::resource_profile::CpuStage::Simulation,
            entity_ticker::run_entity_ticker(entity_ticker::EntityTickerContext {
                prewarmed_entity_pathing_states,
                entity_world_journal_failure,
                entity_shutdown_requested,
                simulation_owner,
                entity_config: Arc::clone(&config),
                entity_sessions: Arc::clone(&sessions),
                entity_chunk_pipeline_resources: chunk_pipeline_resources.clone(),
                entity_world_read,
                entity_world_mutation,
                entity_scheduled_ticks,
                periodic_save_requests: periodic_save_requests.clone(),
                entity_runtime_control: runtime_control.clone(),
                entity_runtime_control_signals: runtime_control_signals,
                entity_tick_metrics: runtime_tick_metrics,
                entity_pathing_materials,
                entity_scripts: scripts.clone(),
                entity_script_zones: script_zones.clone(),
            }),
        ));
        let RuntimeCommandTasks {
            mut command_tasks,
            runtime_control_signal_watcher,
        } = spawn_runtime_command_tasks(RuntimeCommandTaskContext {
            config: Arc::clone(&config),
            sessions: Arc::clone(&sessions),
            runtime_control: runtime_control.clone(),
            simulation: simulation.clone(),
            scripts: scripts.clone(),
            script_storage,
            script_zones: script_zones.clone(),
            shutdown: shutdown.clone(),
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
                    spawn_admitted_connection(
                        &mut connections,
                        socket,
                        peer,
                        &connection_services,
                        &connection_permits,
                        &pre_auth_admission,
                    );
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
                    // Shutdown is already decided: do not request a runtime-control
                    // drain here. Draining collapses entity-owner lanes to one
                    // while the ticker and connection tasks are still draining,
                    // so in-flight owner calls fail Closed and panic. Quiesce
                    // below without touching lane topology.
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
        let (script_commit_event_drain_result, server_stopping_event_result) =
            drain_script_commit_runtime(&sessions, scripts.as_ref(), script_commit_workers).await;
        while let Some(result) = command_tasks.join_next().await {
            if let Err(error) = log_command_task_exit(result, true)
                && command_drain_error.is_none()
            {
                command_drain_error = Some(error);
            }
        }
        if let Some(error) = accept_error
            .or(entity_owner_error)
            .or(connection_task_error)
            .or(command_drain_error)
        {
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

struct RuntimeCommandTaskContext {
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    runtime_control: Option<RuntimeControlHandle>,
    simulation: play::SimulationHandle,
    scripts: Option<ScriptEventSink>,
    script_storage: Option<PluginStorageHandle>,
    script_zones: Option<PluginZoneAdapter>,
    shutdown: ShutdownHandle,
}

struct RuntimeCommandTasks {
    command_tasks: tokio::task::JoinSet<&'static str>,
    runtime_control_signal_watcher: Option<tokio::task::JoinHandle<()>>,
}

fn spawn_runtime_command_tasks(context: RuntimeCommandTaskContext) -> RuntimeCommandTasks {
    let RuntimeCommandTaskContext {
        config,
        sessions,
        runtime_control,
        simulation,
        scripts,
        script_storage,
        script_zones,
        shutdown,
    } = context;
    let runtime_control_signal_watcher = runtime_control.as_ref().map(|control| {
        let control = control.clone();
        let sessions = Arc::clone(&sessions);
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            forward_slow_client_sheds_to_runtime_control(sessions, control, shutdown).await;
        })
    });
    let mut command_tasks = tokio::task::JoinSet::new();
    if let Some(script_commands) = scripts {
        let script_config = Arc::clone(&config);
        let script_sessions = Arc::clone(&sessions);
        let script_simulation = simulation.clone();
        let script_shutdown = shutdown.clone();
        let script_zones =
            script_zones.expect("script boundary and zone adapter are created together");
        command_tasks.spawn(async move {
            run_script_commands(ScriptCommandTask {
                scripts: script_commands,
                storage: script_storage,
                config: script_config,
                sessions: script_sessions,
                simulation: script_simulation,
                shutdown: script_shutdown,
                zones: script_zones,
            })
            .await;
            "script command"
        });
    }
    RuntimeCommandTasks {
        command_tasks,
        runtime_control_signal_watcher,
    }
}

struct ScriptCommandTask {
    scripts: ScriptEventSink,
    storage: Option<PluginStorageHandle>,
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
    simulation: play::SimulationHandle,
    shutdown: ShutdownHandle,
    zones: PluginZoneAdapter,
}

async fn run_script_commands(task: ScriptCommandTask) {
    let ScriptCommandTask {
        scripts,
        storage,
        config,
        sessions,
        simulation,
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
                    ScriptRouter::context(&config, &sessions, &simulation, &shutdown),
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
    bind_internal(config, None).await
}

/// Bind with the bounded server-side script API enabled.
pub async fn bind_with_scripts(
    config: ServerConfig,
    boundary: ScriptBoundary,
) -> std::io::Result<BoundServer> {
    bind_internal(config, Some(ScriptEventSink::new(boundary))).await
}

async fn bind_internal(
    mut config: ServerConfig,
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
    let journal_writer = entity_world_root
        .as_deref()
        .map(play::world_journal::JournalWriter::open)
        .transpose()?;
    let (sessions, pending_entity_commits) = if let Some(root) = entity_world_root.as_deref() {
        let (journal, pending) = play::persistence::FileRegionalDecisionJournal::open(
            root,
            Arc::clone(journal_writer.as_ref().expect("persistent world journal")),
        )
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
        (
            Arc::new(
                play::SessionRegistry::try_new_with_entity_owner_journal(
                    chunk_pipeline_resources.cpu_capacity(),
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
                    chunk_pipeline_resources.cpu_capacity(),
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
    if let (Some(root), Some(world)) = (entity_world_root.as_deref(), config.world.as_ref()) {
        let (journal, pending) = play::world_journal::WorldChunkJournal::open(
            root,
            Arc::clone(&config.blocks),
            Arc::clone(&config.items),
            Arc::clone(journal_writer.as_ref().expect("persistent world journal")),
        )
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
        world.lock().await.set_journal_barrier({
            let writer = Arc::clone(journal_writer.as_ref().expect("persistent world journal"));
            Arc::new(move || writer.flush().map_err(std::io::Error::other))
        });
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
    // The simulation channel exists before the settlement world because a
    // structure portion commits as one server-owned command on it.
    let (simulation, mut simulation_owner) =
        play::simulation_channel_with_explosion_seed(config.random_tick.seed as i64);
    if let Some(scripts) = scripts.as_ref() {
        simulation.install_precommit_boundary(scripts.boundary().clone());
        sessions.install_damage_precommit_handle(simulation.clone());
    }
    // A deployed package that declares the settlement features and ships an
    // authored catalog owns the profile for this world. Validation happens
    // before the storage actor starts, so a catalog violation fails startup
    // loudly instead of degrading to an empty runtime.
    let settlement: Option<(
        Arc<crate::script::storage::SettlementRuntime>,
        Arc<dyn crate::script::storage::SettlementWorld>,
    )> = match (scripts.as_ref(), config.world.as_ref()) {
        (Some(scripts), Some(world)) => {
            let deployment = crate::settlement::discover_settlement_deployment(
                scripts.boundary().deployed_packages(),
                config.blocks.as_ref(),
            )
            .map_err(|error| std::io::Error::new(ErrorKind::InvalidData, error))?;
            match deployment {
                Some(deployment) => {
                    let root = entity_world_root.as_deref().ok_or_else(|| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            "the settlement profile requires a persistent world directory",
                        )
                    })?;
                    let world_identity = crate::settlement::settlement_world_identity(root);
                    // Grounding samples the exact generator the world generates
                    // from; a world without one cannot place sites honestly.
                    let ground = world.lock().await.generator().ok_or_else(|| {
                        std::io::Error::new(
                            ErrorKind::InvalidData,
                            "the settlement profile requires the world's terrain generator",
                        )
                    })?;
                    // The world's own generator answers the village sites:
                    // the same object the terrain generates from, so a listed
                    // village is the village the world will place.
                    let village_sites = Some(Arc::new(
                        crate::script::storage::GeneratorVillageSites::new(Arc::clone(&ground)),
                    )
                        as Arc<dyn crate::script::storage::VillageSiteGround>);
                    let runtime = Arc::new(deployment.runtime(
                        config.random_tick.seed as i64,
                        &world_identity,
                        [0, 0],
                        ground,
                        village_sites,
                    ));
                    let read = world.lock().await.read_view();
                    info!(
                        plugin = deployment.plugin_id(),
                        profile_revision = deployment.profile_revision(),
                        blueprints = runtime.catalog().len(),
                        "settlement blueprint catalog validated",
                    );
                    let adapter = crate::settlement::LiveSettlementWorld::new(
                        read,
                        Arc::clone(&config.blocks),
                        Arc::clone(&config.tags),
                        script_zones.clone(),
                        simulation.clone(),
                    );
                    Some((runtime, Arc::new(adapter)))
                }
                None => None,
            }
        }
        _ => None,
    };
    let inventory_storage = if let Some(root) = entity_world_root.as_deref()
        && (scripts.is_some()
            || sessions
                .world_chunk_journal()
                .is_some_and(|journal| journal.has_inventory_decisions()))
    {
        let mut storage =
            crate::script::storage::PluginStorage::open(root).map_err(plugin_storage_bind_error)?;
        let mut inventory = crate::script::storage::world_inventory::InventoryRuntime::new(
            Some(root),
            &config.shutdown,
            Arc::clone(&sessions),
            Arc::clone(&config.items),
            Arc::clone(&config.item_facts),
        );
        if let Some((runtime, world)) = settlement.as_ref() {
            inventory = inventory
                .with_settlement_runtime(Arc::clone(runtime))
                .with_settlement_world(Arc::clone(world));
        }
        if let Some(world) = config.world.as_ref() {
            // Resident work, routes and combat read the same live world storage
            // the settlement profile uses; without it every physical step fails
            // closed as `unsupported`.
            let read = world.lock().await.read_view();
            inventory = inventory.with_resident_world(Arc::new(
                crate::play::resident_work::LiveResidentWorld::new(
                    read,
                    Arc::clone(&config.blocks),
                    script_zones.clone(),
                    simulation.clone(),
                    Arc::clone(&config.items),
                    Arc::clone(&config.item_facts),
                ),
            ));
        }
        inventory
            .recover(&mut storage)
            .map_err(plugin_storage_bind_error)?;
        Some((storage, inventory))
    } else {
        None
    };
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
                    sessions.restore_weather(metadata.weather);
                    sessions.set_players_sleeping_percentage(metadata.players_sleeping_percentage);
                    sessions.set_keep_inventory(metadata.keep_inventory);
                    info!(
                        world_time = metadata.world_time,
                        daylight_cycle_enabled = metadata.daylight_cycle_enabled,
                        weather = ?metadata.weather,
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
    if let Some((_, inventory)) = inventory_storage.as_ref() {
        inventory.recover_pending_treatments().await?;
    }
    let script_storage =
        scripts
            .as_ref()
            .zip(inventory_storage)
            .map(|(scripts, (storage, inventory))| {
                PluginStorageHandle::start(
                    storage,
                    inventory,
                    scripts.clone(),
                    config.shutdown.clone(),
                )
            });
    if let Some(world_read) = connection_world.read.as_ref() {
        simulation_owner.configure_player_movement_authority(
            world_read.clone(),
            Arc::clone(&config.blocks),
            Arc::clone(&config.block_facts),
        );
    }
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
    sessions.declare_manifest_view_kinds(config.loader_manifest.as_deref());
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
    if !command_permissions.login_access().online_mode {
        warn!(
            "offline-mode public server: player names and operator identities are not authenticated"
        );
    }
    Ok(())
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
#[path = "server_player_sessions_tests.rs"]
mod server_player_sessions_tests;

#[cfg(test)]
#[path = "server/tests.rs"]
pub(crate) mod tests;
