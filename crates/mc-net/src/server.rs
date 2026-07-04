//! TCP listener, accept loop, and the per-connection state-machine entry
//! point.

use std::collections::{BTreeSet, HashMap};
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::BytesMut;
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
use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::{BlockPos, BlockRegistry, MAX_Y, MIN_Y, WorldStorage};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, Semaphore};
use tracing::{debug, info, warn};

use crate::chunk_pipeline::ChunkPipelineResources;
use crate::connection::{PRE_PLAY_READ_TIMEOUT, read_packet_with_timeout};
use crate::error::ConnectionError;
use crate::{ChunkPipelinePolicy, RuntimeControlHandle, RuntimeControlInput};
use crate::{configuration, login, play, status};

static SAVE_COORDINATOR: OnceLock<Mutex<()>> = OnceLock::new();
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
                .map(Into::into)
                .map(|entry| entry.to_ascii_lowercase())
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
    ) -> play::commands::CommandPermissions {
        play::commands::CommandPermissions::from_op(self.is_operator(profile))
    }

    #[must_use]
    fn is_operator(&self, profile: &login::LoggedInProfile) -> bool {
        if self.operators.is_empty() && self.allow_local_dev_operators {
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

#[derive(Clone, Default)]
pub struct ShutdownHandle {
    requested: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeMetricsPolicy {
    pub log_interval_ticks: u64,
    pub slow_tick_ms: u64,
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

    pub(crate) async fn notified(&self) {
        if self.requested.load(Ordering::SeqCst) {
            return;
        }
        self.notify.notified().await;
    }
}

/// Shared, mutably-accessible handle to the world.
///
/// `WorldStorage::get_chunk` is `&mut self` — it touches an internal
/// LRU on every call — so we wrap it in a tokio Mutex. The mutex is
/// async because chunk reads will eventually await disk I/O (M3.f's
/// region cache + worker pool).
pub type WorldHandle = Arc<Mutex<WorldStorage>>;

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
    chunk_pipeline_resources: ChunkPipelineResources,
    runtime_control: Option<RuntimeControlHandle>,
    sessions: Arc<play::SessionRegistry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboundPressureSnapshot {
    pub visibility_command_drops: u64,
    pub reliable_command_retries: u64,
    pub reliable_command_retries_in_flight: u64,
    pub max_reliable_command_retries_in_flight: u64,
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
            visibility_command_drops: pressure.visibility_command_drops,
            reliable_command_retries: pressure.reliable_command_retries,
            reliable_command_retries_in_flight: pressure.reliable_command_retries_in_flight,
            max_reliable_command_retries_in_flight: pressure.max_reliable_command_retries_in_flight,
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
}

impl SaveHandle {
    pub async fn save_all(&self) -> SaveAllReport {
        save_all(&self.config, &self.sessions).await
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
        }
    }

    #[must_use]
    pub fn chunk_pipeline_metrics(&self) -> crate::ChunkPipelineResourceMetrics {
        self.chunk_pipeline_resources.metrics()
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

    /// Accept connections forever, spawning a per-connection task each
    /// time. An error inside a connection task is logged but does not
    /// stop the listener.
    pub async fn serve(self) -> std::io::Result<()> {
        info!(
            addr = %self.local_addr()?,
            registries = self.config.data.registry_count(),
            entries = self.config.data.entry_count(),
            "Solaris is listening"
        );
        let config = self.config;
        let chunk_pipeline_resources = self.chunk_pipeline_resources;
        let runtime_control = self.runtime_control;
        let sessions = self.sessions;
        let shutdown = config.shutdown.clone();
        let mut connections = tokio::task::JoinSet::new();
        let entity_world_root = if let Some(world) = config.world.as_ref() {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "entity world root",
                Instant::now(),
                world.lock().await,
            );
            storage.world_root().map(std::path::Path::to_path_buf)
        } else {
            None
        };
        let entity_sessions = Arc::clone(&sessions);
        let entity_config = Arc::clone(&config);
        let entity_runtime_control = runtime_control.clone();
        let entity_chunk_pipeline_resources = chunk_pipeline_resources.clone();
        let entity_cpu_permits =
            Arc::new(Semaphore::new(config.chunk_pipeline.entity_worker_threads));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(play::ENTITY_TICK_PERIOD);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let metrics_policy = RuntimeMetricsPolicy::default().normalized();
            let simulation_policy = entity_config.random_tick.normalized();
            let mut tick = 0_u64;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = entity_config.shutdown.notified() => {
                        info!("shutdown requested; entity ticker stopping");
                        break;
                    }
                }
                let tick_started = Instant::now();
                tick = tick.wrapping_add(1);

                let started = Instant::now();
                let world_time = entity_sessions.advance_world_time(1);
                let world_time_us = elapsed_us(started);

                let started = Instant::now();
                let queries = entity_sessions.tick_entities_and_collect_physics_queries(tick);
                let entity_goals_us = elapsed_us(started);

                let started = Instant::now();
                let steps =
                    entity_physics_steps(&entity_config, Arc::clone(&entity_cpu_permits), &queries)
                        .await;
                let entity_physics_us = elapsed_us(started);

                let started = Instant::now();
                entity_sessions.apply_entity_physics_and_dispatch(tick, &steps);
                let entity_dispatch_us = elapsed_us(started);

                let landed_falling_blocks = entity_sessions.landed_falling_blocks(&steps);
                if !landed_falling_blocks.is_empty() {
                    play::land_falling_blocks(
                        &entity_config,
                        &entity_sessions,
                        &landed_falling_blocks,
                    )
                    .await;
                }

                let started = Instant::now();
                let campfire_tick =
                    play::run_campfire_cooking_ticks(&entity_config, &entity_sessions).await;
                let campfire_tick_us = elapsed_us(started);

                let started = Instant::now();
                let mut entity_save_us = 0;
                if tick.is_multiple_of(simulation_policy.save_interval_ticks)
                    && entity_sessions.active_session_count() > 0
                    && let Some(root) = entity_world_root.as_deref()
                {
                    let _save_guard = SAVE_COORDINATOR.get_or_init(|| Mutex::new(())).lock().await;
                    let records = entity_sessions.persisted_entity_records();
                    if let Err(err) = save_entities_blocking(
                        root.to_path_buf(),
                        Arc::clone(&entity_config.items),
                        records,
                    )
                    .await
                    {
                        warn!(error = %err, "persisted entity save failed");
                    }
                    let metadata = play::persistence::WorldPersistedMetadata {
                        world_time,
                        world_identity: play::persistence::world_identity(root),
                    };
                    if let Err(err) =
                        save_world_metadata_blocking(root.to_path_buf(), metadata).await
                    {
                        warn!(error = %err, "world metadata save failed");
                    }
                    entity_save_us = elapsed_us(started);
                }

                let started = Instant::now();
                let random_tick =
                    play::run_random_ticks(&entity_config, &entity_sessions, tick).await;
                let random_tick_us = elapsed_us(started);

                let started = Instant::now();
                let fluid_tick =
                    play::run_scheduled_fluid_ticks(&entity_config, &entity_sessions, tick).await;
                let fluid_tick_us = elapsed_us(started);

                let tick_us = elapsed_us(tick_started);
                if let Some(control) = entity_runtime_control.as_ref() {
                    let resources = entity_chunk_pipeline_resources.metrics().snapshot();
                    control.observe(RuntimeControlInput {
                        tick_ms: tick_us.div_ceil(1_000),
                        queued_chunks: 0,
                        queue_capacity: 0,
                        active_workers: resources.active_cpu,
                        worker_capacity: entity_config.chunk_pipeline.chunk_worker_threads.max(1),
                        memory_used_mb: 0,
                        memory_limit_mb: 0,
                        first_chunk_ms: None,
                    });
                }
                if should_log_runtime_metrics(tick, tick_us, metrics_policy) {
                    let pressure = entity_sessions.pressure_snapshot();
                    let lock_pressure = crate::lock_metrics::snapshot();
                    if is_slow_tick(tick_us, metrics_policy) {
                        warn!(
                            tick,
                            world_time,
                            tick_us,
                            world_time_us,
                            entity_goals_us,
                            entity_physics_us,
                            entity_dispatch_us,
                            campfire_tick_us,
                            entity_save_us,
                            random_tick_us,
                            fluid_tick_us,
                            entity_queries = queries.len(),
                            entity_steps = steps.len(),
                            campfire_persisted = campfire_tick.persisted,
                            campfire_completed = campfire_tick.completed,
                            campfire_dropped = campfire_tick.dropped,
                            random_sampled = random_tick.sampled,
                            random_eligible = random_tick.eligible,
                            random_applied = random_tick.applied,
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
                            visibility_command_drops = pressure.visibility_command_drops,
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
                    } else {
                        debug!(
                            tick,
                            world_time,
                            tick_us,
                            world_time_us,
                            entity_goals_us,
                            entity_physics_us,
                            entity_dispatch_us,
                            campfire_tick_us,
                            entity_save_us,
                            random_tick_us,
                            fluid_tick_us,
                            entity_queries = queries.len(),
                            entity_steps = steps.len(),
                            campfire_persisted = campfire_tick.persisted,
                            campfire_completed = campfire_tick.completed,
                            campfire_dropped = campfire_tick.dropped,
                            random_sampled = random_tick.sampled,
                            random_eligible = random_tick.eligible,
                            random_applied = random_tick.applied,
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
                            visibility_command_drops = pressure.visibility_command_drops,
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
        });
        tokio::spawn(run_console_commands(
            Arc::clone(&config),
            Arc::clone(&sessions),
        ));
        loop {
            tokio::select! {
                result = self.listener.accept() => {
                    let (socket, peer) = result?;
                    debug!(%peer, "accepted connection");
                    let config = config.clone();
                    let chunk_pipeline_resources = chunk_pipeline_resources.clone();
                    let runtime_control = runtime_control.clone();
                    let sessions = Arc::clone(&sessions);
                    connections.spawn(async move {
                        if let Err(err) =
                            handle_connection(socket, &config, sessions, chunk_pipeline_resources, runtime_control).await
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
                        warn!(error = %err, "connection task join failed");
                    }
                }
                () = shutdown.notified() => {
                    info!("shutdown requested; listener stopping");
                    break;
                }
            };
        }
        drain_connections(&mut connections).await;
        Ok(())
    }
}

async fn drain_connections(connections: &mut tokio::task::JoinSet<()>) {
    let started = Instant::now();
    while !connections.is_empty() {
        let Some(remaining) = CONNECTION_DRAIN_TIMEOUT.checked_sub(started.elapsed()) else {
            warn!(remaining = connections.len(), "connection drain timed out");
            return;
        };
        match tokio::time::timeout(remaining, connections.join_next()).await {
            Ok(Some(Ok(()))) => {}
            Ok(Some(Err(err))) => warn!(error = %err, "connection task join failed"),
            Ok(None) => return,
            Err(_) => {
                warn!(remaining = connections.len(), "connection drain timed out");
                return;
            }
        }
    }
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

fn should_log_runtime_metrics(tick: u64, tick_us: u64, policy: RuntimeMetricsPolicy) -> bool {
    tick.is_multiple_of(policy.log_interval_ticks) || is_slow_tick(tick_us, policy)
}

fn is_slow_tick(tick_us: u64, policy: RuntimeMetricsPolicy) -> bool {
    policy.slow_tick_ms > 0 && tick_us >= policy.slow_tick_ms.saturating_mul(1_000)
}

async fn run_console_commands(config: Arc<ServerConfig>, sessions: Arc<play::SessionRegistry>) {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    loop {
        let line = tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) => line,
                    Ok(None) => return,
                    Err(err) => {
                        warn!(error = %err, "console command input failed");
                        return;
                    }
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
        match play::commands::parse_admin_command(raw, play::commands::CommandPermissions::CONSOLE)
        {
            Ok(play::commands::AdminCommand::SaveAll) => {
                let report = save_all(&config, &sessions).await;
                log_save_report("console save-all", &report);
            }
            Ok(play::commands::AdminCommand::Stop) => {
                let report = save_all(&config, &sessions).await;
                log_save_report("console stop", &report);
                if report.is_ok() {
                    config.shutdown.request();
                    return;
                }
                warn!("console stop aborted because save-all failed");
            }
            Ok(play::commands::AdminCommand::TimeSet(time)) => {
                sessions.set_world_time(time);
                info!(time, "console set world time");
            }
            Ok(command) => {
                warn!(
                    ?command,
                    "console command requires a player source in this M35 slice"
                );
            }
            Err(error) => {
                warn!(
                    error = console_command_error(error),
                    "console command rejected"
                );
            }
        }
    }
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

async fn entity_physics_steps(
    config: &ServerConfig,
    cpu_permits: Arc<Semaphore>,
    queries: &[play::EntityPhysicsQuery],
) -> Vec<play::EntityPhysicsStep> {
    let Some(world) = config.world.as_ref() else {
        return Vec::new();
    };
    if queries.is_empty() {
        return Vec::new();
    }
    let materials = cached_material_ids(config);
    let inputs = {
        let mut storage = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::WorldStorage,
            "entity physics sampling",
            Instant::now(),
            world.lock().await,
        );
        queries
            .iter()
            .map(|query| sample_entity_physics_input(*query, &mut storage, &materials))
            .collect::<Vec<_>>()
    };

    let workers = config
        .chunk_pipeline
        .entity_worker_threads
        .max(1)
        .min(inputs.len());
    let batch_size = inputs.len().div_ceil(workers);
    let mut handles = Vec::with_capacity(workers);
    let mut inputs = inputs.into_iter();
    for _ in 0..workers {
        let batch = inputs.by_ref().take(batch_size).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }
        let permit = match Arc::clone(&cpu_permits).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return Vec::new(),
        };
        handles.push(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            batch
                .into_iter()
                .map(step_sampled_entity)
                .collect::<Vec<_>>()
        }));
    }

    let mut steps = Vec::with_capacity(queries.len());
    for handle in handles {
        match handle.await {
            Ok(mut batch) => steps.append(&mut batch),
            Err(err) if err.is_cancelled() => debug!("entity physics worker cancelled"),
            Err(err) => warn!(error = %err, "entity physics worker failed"),
        }
    }
    steps
}

struct EntityPhysicsInput {
    query: play::EntityPhysicsQuery,
    samples: HashMap<BlockPos, BlockMaterial>,
    complete_samples: bool,
}

struct SampledPhysicsWorld {
    samples: HashMap<BlockPos, BlockMaterial>,
}

impl BlockSampler for SampledPhysicsWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.samples
            .get(&BlockPos { x, y, z })
            .copied()
            .unwrap_or(BlockMaterial::Air)
    }
}

fn sample_entity_physics_input(
    query: play::EntityPhysicsQuery,
    storage: &mut WorldStorage,
    materials: &BlockMaterialIds,
) -> EntityPhysicsInput {
    let positions = entity_physics_sample_positions(query);
    let mut samples = HashMap::with_capacity(positions.len());
    let mut complete_samples = true;
    for pos in positions {
        let material = match storage.get_cached_block(pos) {
            Some(state) => materials.classify(state.0),
            None if (MIN_Y..MAX_Y).contains(&pos.y) => {
                complete_samples = false;
                BlockMaterial::Air
            }
            None => BlockMaterial::Air,
        };
        samples.insert(pos, material);
    }
    EntityPhysicsInput {
        query,
        samples,
        complete_samples,
    }
}

fn entity_physics_sample_positions(query: play::EntityPhysicsQuery) -> Vec<BlockPos> {
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

    let capacity = (max_x - min_x + 1).max(0) as usize
        * (max_y - min_y + 1).max(0) as usize
        * (max_z - min_z + 1).max(0) as usize;
    let mut positions = Vec::with_capacity(capacity);
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                positions.push(BlockPos { x, y, z });
            }
        }
    }
    positions
}

fn step_sampled_entity(input: EntityPhysicsInput) -> play::EntityPhysicsStep {
    if !input.complete_samples {
        return play::EntityPhysicsStep {
            id: input.query.id,
            position: input.query.position,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: input.query.on_ground,
        };
    }
    let sampler = SampledPhysicsWorld {
        samples: input.samples,
    };
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
    }
}

fn physics_config_for_query(query: play::EntityPhysicsQuery) -> PhysicsConfig {
    match query.kind {
        play::EntityPhysicsKind::Default => PhysicsConfig::default(),
        play::EntityPhysicsKind::ArrowProjectile => PhysicsConfig::arrow_projectile(),
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
    let mut cache = cache_lock.lock().unwrap_or_else(|poisoned| {
        warn!("physics material cache mutex was poisoned; recovering state");
        cache_lock.clear_poison();
        poisoned.into_inner()
    });
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

    BlockMaterialIds::new(
        state("minecraft:air").unwrap_or(0),
        state("minecraft:water"),
        state("minecraft:lava"),
    )
    .with_water_states(fluid_material_states(blocks, facts, FluidKind::Water))
    .with_lava_states(fluid_material_states(blocks, facts, FluidKind::Lava))
    .with_passable(passable)
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
    validate_public_security_config(config.bind_address, &config.command_permissions)?;
    let listener = TcpListener::bind(config.bind_address).await?;
    let chunk_pipeline_resources = ChunkPipelineResources::new(config.chunk_pipeline);
    let runtime_control = config
        .chunk_pipeline
        .runtime_control
        .map(RuntimeControlHandle::new);
    let sessions = Arc::new(play::SessionRegistry::new());
    play::configure_session_arrow_kill_rewards(&sessions, &config);
    if let Some(world) = config.world.as_ref() {
        let world_root = {
            let storage = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::WorldStorage,
                "bind world root",
                Instant::now(),
                world.lock().await,
            );
            storage.world_root().map(std::path::Path::to_path_buf)
        };
        if let Some(root) = world_root.as_deref() {
            match play::persistence::load_world_metadata(root) {
                Ok(Some(metadata)) => {
                    let expected = play::persistence::world_identity(root);
                    if !metadata.world_identity.is_empty() && metadata.world_identity != expected {
                        warn!(
                            stored = %metadata.world_identity,
                            expected = %expected,
                            "world metadata identity mismatch"
                        );
                    }
                    sessions.set_world_time(metadata.world_time);
                    info!(world_time = metadata.world_time, "loaded world metadata");
                }
                Ok(None) => {}
                Err(err) => warn!(error = %err, "world metadata load failed"),
            }
            match play::persistence::load_persisted_entities(
                root,
                &config.items,
                &config.entity_types,
            ) {
                Ok(entities) => {
                    let restored = sessions.restore_persisted_entities(entities);
                    if restored > 0 {
                        info!(restored, "loaded persisted entities");
                    }
                }
                Err(err) => warn!(error = %err, "persisted entity load failed"),
            }
        }
    }
    play::hydrate_persisted_campfire_cooking(&config, &sessions).await;
    Ok(BoundServer {
        listener,
        config: Arc::new(config),
        chunk_pipeline_resources,
        runtime_control,
        sessions,
    })
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
        Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "online-mode authentication is not implemented; public bind addresses are disabled",
        ))
    } else {
        Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "offline-mode Solaris authentication cannot be used on a public bind address",
        ))
    }
}

fn is_public_bind(addr: SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_private() && !ip.is_link_local(),
        std::net::IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unique_local(),
    }
}

pub async fn save_all(config: &ServerConfig, sessions: &play::SessionRegistry) -> SaveAllReport {
    let total_started = Instant::now();
    let queue_started = Instant::now();
    let _save_guard = SAVE_COORDINATOR.get_or_init(|| Mutex::new(())).lock().await;
    let mut report = SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        timings: SaveAllTimings {
            queued_us: elapsed_us(queue_started),
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

    let started = Instant::now();
    let storage = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::SaveAllFlush,
        "save-all dirty flush plan",
        Instant::now(),
        world.lock().await,
    );
    let storage_before = storage.stats();
    let flush_plan = match storage.plan_dirty_flush_at_tick(sessions.simulation_tick()) {
        Ok(plan) => Some(plan),
        Err(err) => {
            report
                .errors
                .push(format!("dirty chunks: flush plan failed: {err}"));
            None
        }
    };
    drop(storage);
    report.timings.flush_plan_us = elapsed_us(started);

    if let Some(flush_plan) = flush_plan {
        let planned_chunks = flush_plan.chunk_count();
        let flush_started = Instant::now();
        if flush_plan.is_empty() {
            info!(
                flushed = 0usize,
                planned = 0usize,
                flush_us = elapsed_us(flush_started),
                chunk_cache_len = storage_before.chunk_cache_len,
                chunk_cache_capacity = storage_before.chunk_cache_capacity,
                region_cache_len = storage_before.region_cache_len,
                region_cache_capacity = storage_before.region_cache_capacity,
                dirty_before = storage_before.dirty_chunks,
                dirty_after = storage_before.dirty_chunks,
                "world storage save pressure"
            );
        } else {
            let started = Instant::now();
            match crate::dirty_flush::write_dirty_flush_blocking(flush_plan).await {
                Ok(commit) => {
                    report.timings.flush_write_us = elapsed_us(started);

                    let started = Instant::now();
                    let mut storage = crate::lock_metrics::timed_guard(
                        crate::lock_metrics::LockMetricKind::SaveAllFlush,
                        "save-all dirty flush commit",
                        Instant::now(),
                        world.lock().await,
                    );
                    match storage.commit_dirty_flush(commit) {
                        Ok(flushed) => {
                            report.chunks_flushed = flushed;
                            let storage_after = storage.stats();
                            info!(
                                flushed,
                                planned = planned_chunks,
                                flush_us = elapsed_us(flush_started),
                                chunk_cache_len = storage_after.chunk_cache_len,
                                chunk_cache_capacity = storage_after.chunk_cache_capacity,
                                region_cache_len = storage_after.region_cache_len,
                                region_cache_capacity = storage_after.region_cache_capacity,
                                dirty_before = storage_before.dirty_chunks,
                                dirty_after = storage_after.dirty_chunks,
                                "world storage save pressure"
                            );
                        }
                        Err(err) => {
                            report
                                .errors
                                .push(format!("dirty chunks: flush commit failed: {err}"));
                        }
                    }
                    report.timings.flush_commit_us = elapsed_us(started);
                }
                Err(err) => {
                    report
                        .errors
                        .push(format!("dirty chunks: flush write failed: {err}"));
                    report.timings.flush_write_us = elapsed_us(started);
                }
            }
        }
    }

    let started = Instant::now();
    let players = sessions.persisted_player_states();
    let (players_saved, player_errors) =
        save_player_states_blocking(root.clone(), Arc::clone(&config.items), players).await;
    report.players_saved = players_saved;
    report.errors.extend(player_errors);
    report.timings.players_us = elapsed_us(started);

    let started = Instant::now();
    let entities = sessions.persisted_entity_records();
    report.entities_saved = entities.len();
    if let Err(err) =
        save_entities_blocking(root.clone(), Arc::clone(&config.items), entities).await
    {
        report.errors.push(format!("entities: save failed: {err}"));
    }
    report.timings.entities_us = elapsed_us(started);

    let started = Instant::now();
    let metadata = play::persistence::WorldPersistedMetadata {
        world_time: sessions.world_time(),
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
    players: Vec<(uuid::Uuid, play::persistence::PlayerPersistedState)>,
) -> (usize, Vec<String>) {
    match tokio::task::spawn_blocking(move || {
        let mut saved = 0usize;
        let mut errors = Vec::new();
        for (uuid, player) in players {
            match play::persistence::save_player_state(&root, uuid, &items, &player) {
                Ok(()) => saved += 1,
                Err(err) => errors.push(format!("player {uuid}: save failed: {err}")),
            }
        }
        (saved, errors)
    })
    .await
    {
        Ok(result) => result,
        Err(err) => (0, vec![format!("players: save worker failed: {err}")]),
    }
}

async fn save_entities_blocking(
    root: std::path::PathBuf,
    items: Arc<ItemRegistry>,
    entities: Vec<play::persistence::PersistedEntityRecord>,
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

/// Convenience for the binary: `bind` followed by `serve`.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    bind(config).await?.serve().await
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    config: &ServerConfig,
    sessions: Arc<play::SessionRegistry>,
    chunk_pipeline_resources: ChunkPipelineResources,
    runtime_control: Option<RuntimeControlHandle>,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets — same setting
    // vanilla uses.
    socket.set_nodelay(true)?;

    let (mut reader, mut writer) = socket.into_split();
    let mut buf = BytesMut::with_capacity(4096);
    let mut compression = Compression::Disabled;

    let handshake = read_packet_with_timeout::<Handshake, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Handshake,
        PRE_PLAY_READ_TIMEOUT,
    )
    .await?;
    debug!(
        protocol = handshake.protocol_version,
        address = %handshake.server_address,
        port = handshake.server_port,
        next = ?handshake.next_state,
        "handshake received"
    );

    match handshake.next_state {
        NextState::Status => status::handle(&mut reader, &mut writer, &mut buf, config).await,
        NextState::Login | NextState::Transfer => {
            let profile = login::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                config.chunk_pipeline.compression_threshold,
                &mut compression,
                config.chunk_pipeline.compression_level,
                config.command_permissions.login_access(),
            )
            .await?;
            let Some(profile) = profile else {
                return Ok(());
            };
            configuration::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                &config.data,
                &config.tags,
            )
            .await?;
            play::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                config,
                sessions,
                chunk_pipeline_resources,
                runtime_control,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use std::collections::BTreeMap;

    type StateSpec<'a> = (u32, bool, &'a [(&'a str, &'a str)]);

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
    fn runtime_metrics_logging_respects_interval_and_slow_budget() {
        let policy = RuntimeMetricsPolicy {
            log_interval_ticks: 5,
            slow_tick_ms: 50,
        };

        assert!(should_log_runtime_metrics(10, 1, policy));
        assert!(should_log_runtime_metrics(11, 50_000, policy));
        assert!(!should_log_runtime_metrics(11, 49_999, policy));
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

    #[test]
    fn broken_pipe_is_graceful_disconnect() {
        let err = ConnectionError::Io(std::io::Error::from(ErrorKind::BrokenPipe));
        assert!(is_client_disconnect(&err));
    }

    #[test]
    fn codec_error_is_not_graceful_disconnect() {
        let err = ConnectionError::UnexpectedPacketId {
            state: State::Login,
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
            kind: play::EntityPhysicsKind::Default,
        };
        let step = step_sampled_entity(EntityPhysicsInput {
            query,
            samples: HashMap::new(),
            complete_samples: false,
        });

        assert_eq!(step.position, query.position);
        assert_eq!(step.velocity, mc_entity::Vec3::ZERO);
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
            entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(&[])),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
            command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), false),
            shutdown: ShutdownHandle::default(),
        };

        let first = cached_material_ids(&config);
        let second = cached_material_ids(&config);

        assert!(Arc::ptr_eq(&first, &second));
        assert!(cache.lock().is_ok());
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
            shutdown: ShutdownHandle::default(),
        }
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
        let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(&[
            mc_data::entity_types::EntityTypeReport {
                id: Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 1,
            },
        ]));
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(42);
        sessions.restore_persisted_entities([play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_001),
                uuid: uuid::Uuid::from_u128(1),
                type_id: 1,
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
            },
            age: 12,
            pickup_delay: 3,
        }]);
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
            play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();
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
        let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(&[
            mc_data::entity_types::EntityTypeReport {
                id: Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 1,
            },
        ]));
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(99);
        sessions.restore_persisted_entities([play::persistence::PersistedEntityRecord {
            snapshot: mc_entity::EntitySnapshot {
                id: mc_entity::EntityId(1_000_002),
                uuid: uuid::Uuid::from_u128(2),
                type_id: 1,
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
            },
            age: 8,
            pickup_delay: 4,
        }]);
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
        let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(&[]));
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
    fn public_security_rejects_online_mode_until_auth_exists() {
        let permissions = CommandPermissionConfig::new(["Notch"], false).with_login_access(
            login::LoginAccessConfig::normalized(
                true,
                false,
                std::iter::empty::<&str>(),
                std::iter::empty::<&str>(),
            ),
        );
        let err = validate_public_security_config("8.8.8.8:25565".parse().unwrap(), &permissions)
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(err.to_string().contains("online-mode authentication"));
    }

    #[test]
    fn private_security_allows_local_offline_dev() {
        validate_public_security_config(
            "127.0.0.1:25565".parse().unwrap(),
            &CommandPermissionConfig::new(Vec::<String>::new(), true),
        )
        .unwrap();
    }
}
