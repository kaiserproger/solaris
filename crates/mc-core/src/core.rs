//! [`Core`]: public handle — `open`, `submit`, tick driver, subscribe.
//!
//! - [`Core::open`] builds registries, recovers the journal, loads the world.
//! - [`Core::submit`] is callable from I/O threads: bounded, push-woken, never
//!   blocks (`false` = queue full, input dropped and counted).
//! - The tick thread runs at 20 TPS and delivers each [`TickOutput`] to a
//!   bounded broadcast channel (slow consumers skip to latest via `Lagged`)
//!   plus per-connection reliable [`PlayerChange`](crate::boundary::PlayerChange)
//!   outboxes (bounded; overflow disconnects). The journal thread owns
//!   appends/fsyncs/checkpoints behind a bounded FIFO work queue fed by the
//!   tick thread, which blocks when the queue is full (backpressure) so
//!   accepted batches are never dropped; recovery converges every domain to
//!   the last complete batch.
//!
//! The tick thread owns [`TickState`] and there is no shared outbox store; the
//! journal thread owns its writer/checkpoint work and they communicate by
//! message passing. Remaining shared locks on the tick path are the plugin
//! precommit authorization/session boundaries (`build_precommit.rs`).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;
use tokio::sync::mpsc::{Receiver, Sender, channel as mpsc_channel};

use crate::boundary::{ConnectionId, EntityId, PlayerChange, PlayerInput, TickOutput, TickTimes};
use crate::config::{CoreConfig, CoreError};
use crate::entities::EntityKind;
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, block_pos_array, entity_id_int,
    prune_batches_through,
};
use crate::journal_budget::{JOURNAL_QUEUE_BYTES, JournalBudget};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

#[path = "core_stop.rs"]
mod stop;

#[cfg(test)]
#[path = "core_stop_tests.rs"]
mod stop_tests;

type OutboxBatch = Vec<(ConnectionId, Vec<PlayerChange>)>;

#[path = "core_session.rs"]
mod session;
pub use session::PlayerSession;

#[cfg(test)]
#[path = "core_session_tests.rs"]
mod session_tests;

/// Only edge consumers share this receiver; the tick owns its sender.
#[derive(Debug)]
struct ReliableOutbox(parking_lot::Mutex<std::sync::mpsc::Receiver<OutboxBatch>>);

impl ReliableOutbox {
    fn drain(&self) -> OutboxBatch {
        let receiver = self.0.lock();
        let mut store: HashMap<ConnectionId, Vec<PlayerChange>> = HashMap::new();
        for batch in receiver.try_iter() {
            for (connection, queue) in batch {
                let slot = store.entry(connection).or_default();
                if !slot
                    .last()
                    .is_some_and(|change| matches!(change, PlayerChange::Disconnect { .. }))
                {
                    slot.extend(queue);
                }
            }
        }
        store
            .into_iter()
            .filter(|(_, queue)| !queue.is_empty())
            .collect()
    }
}

use mc_world::ChunkGenerator;

/// Tick period: 20 TPS.
pub const TICK_PERIOD: std::time::Duration = std::time::Duration::from_millis(50);

/// Per-world fault injection at the frozen native checkpoint's pre-commit gate.
/// No native records or dirty acknowledgements are published on this failure.
#[cfg(test)]
pub static FAIL_CHECKPOINT_AFTER_REGIONS: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Per-world test delay inside the native checkpoint commit (after the WAL gate).
#[cfg(test)]
pub(crate) static DELAY_CHECKPOINT_COMMIT: parking_lot::Mutex<
    Option<(std::path::PathBuf, std::time::Duration)>,
> = parking_lot::const_mutex(None);

/// Per-world test hook: the next native checkpoint commit panics (one shot).
#[cfg(test)]
pub(crate) static PANIC_CHECKPOINT_COMMIT: parking_lot::Mutex<Option<std::path::PathBuf>> =
    parking_lot::const_mutex(None);

/// Per-world crash boundary after the durable native commit and before ACK/pruning.
#[cfg(test)]
static FAIL_CHECKPOINT_AFTER_NATIVE_COMMIT: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Test hook: when armed with a world dir, every [`JournalWriter::append`]
/// for THAT dir fails with a storage error (simulates a torn/failed WAL
/// batch). The journal thread returns (receiver dropped); the tick loop's
/// next Append send fails and it halts (Disconnect all, no more publishes,
/// no checkpoints, no retry worker). Unit tests only (per-dir so parallel
/// tests never poison each other's journals).
#[cfg(test)]
pub static FAIL_JOURNAL_APPEND: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

/// Test-only, world-scoped rendezvous after a successful durable append.
/// The observer may block the journal owner until a crash test kills it.
#[cfg(test)]
type JournalAppendObserver = (std::path::PathBuf, Box<dyn Fn(u64, &[JournalEntry]) + Send>);
#[cfg(test)]
pub(crate) static JOURNAL_APPEND_OBSERVER: std::sync::Mutex<Option<JournalAppendObserver>> =
    std::sync::Mutex::new(None);

/// Remote area ticket: keeps an area simulated without players.
#[derive(Debug, Clone)]
pub struct RemoteTicket {
    /// Ticket owner name.
    pub owner: String,
    /// Chunk center of the ticketed area.
    pub chunk: crate::boundary::ChunkPos,
    /// Radius in chunks.
    pub radius: u32,
    /// Reason string (debugging/audit).
    pub reason: String,
    /// Budget: max chunks this ticket may hold.
    pub budget_chunks: usize,
}
/// Only a completed checkpoint may acknowledge the captured queue tombstones.
type CheckpointAck = Option<(
    Vec<(crate::boundary::ChunkPos, u64)>,
    crate::block_ticks::BlockTicksImage,
    crate::fluids::FluidTicksImage,
    std::collections::BTreeMap<crate::dimensions::Dimension, crate::dimensions::checkpoint::Ack>,
)>;

type CheckpointWrite = (
    Vec<(crate::boundary::ChunkPos, u64)>,
    crate::dimensions::checkpoint::Written,
    mc_world::native::NativeCheckpointIo,
);

/// Work for the journal thread.
enum JournalWork {
    /// One committed tick's entries.
    Append {
        tick_lo: u64,
        tick_hi: u64,
        entries: Vec<JournalEntry>,
        plugin_ack: Option<std::sync::mpsc::Sender<u64>>,
        /// Bytes reserved from the journal budget for this batch; the journal
        /// thread returns them once the batch is appended.
        reserved: usize,
    },
    /// Checkpoint `tick` from a capture frozen on the tick thread, in
    /// write-ahead order (ADR 0013): the journal thread gates the WAL through
    /// `tick` (fsync), then forwards the job to the native writer thread, which
    /// commits every domain in one transaction, reports on the job's `done`
    /// channel and prunes. The tick thread never waits for the native write —
    /// queue admission is blocking backpressure while full, and a commit
    /// never occupies the thread that drains `Append` — and the ACK on `done`
    /// still only reports disk durability, never queue admission.
    Checkpoint(Box<CheckpointJob>),
    /// Save generated chunks into their still-empty slots. Forwarded to the
    /// native writer so the native DB has one writer, ordered with checkpoints.
    SaveGenerated(crate::world::ChunkCapture),
    /// Stop the thread.
    Shutdown,
    /// Copy the native database and WAL after the last committed checkpoint into
    /// a caller-owned directory (hot backup).
    Backup(BackupJob),
}

/// One hot backup request; the answer arrives once the native writer has copied.
struct BackupJob {
    out: PathBuf,
    reply: std::sync::mpsc::Sender<Result<crate::hot_backup::HotBackup, CoreError>>,
}
/// Bounded journal work queue: 128 queued batches (6.4 s of ticks) behind the
/// one the journal thread is draining. The depth only absorbs journal-thread
/// stalls (an fsync waiting behind a multi-hundred-MB redb commit, measured
/// 0.1-1.6 s, parked the two-deep queue's tick for up to 267 ms): while the
/// journal keeps up the queue is empty. The tick thread still sends with
/// blocking `send`, so a journal slower than that parks the tick instead of
/// growing memory; accepted batches are never dropped or reordered. A crash
/// loses at most the batches still queued, i.e. up to this many ticks, and
/// only while the journal thread is stalled. `SyncSender::send` fails only
/// when the journal thread is gone (receiver disconnected) — the existing
/// fail-closed halt.
const JOURNAL_WORK_QUEUE: usize = 128;
/// Production journal work transport: `sync_channel` with room for
/// [`JOURNAL_WORK_QUEUE`] queued batches; `send` blocks while full and fails
/// only when the journal thread is gone.
fn journal_work_channel() -> (
    std::sync::mpsc::SyncSender<JournalWork>,
    std::sync::mpsc::Receiver<JournalWork>,
) {
    std::sync::mpsc::sync_channel(JOURNAL_WORK_QUEUE)
}

/// Queue one tick's `Append`. Reserves the batch's estimated bytes first
/// ([`JOURNAL_QUEUE_BYTES`] bounds the in-flight payload, the count cap alone
/// does not) and then sends; both block under backpressure. `Err` only when
/// the journal thread is gone.
fn send_append(
    work_tx: &std::sync::mpsc::SyncSender<JournalWork>,
    budget: &JournalBudget,
    tick: u64,
    entries: Vec<JournalEntry>,
    plugin_ack: Option<std::sync::mpsc::Sender<u64>>,
) -> Result<(), ()> {
    // Spare Vec capacity is resident too.
    let reserved = crate::journal_budget::entries_bytes(&entries)
        + (entries.capacity() - entries.len()) * std::mem::size_of::<JournalEntry>();
    budget.reserve(reserved).map_err(|_| ())?;
    work_tx
        .send(JournalWork::Append {
            tick_lo: tick,
            tick_hi: tick,
            entries,
            plugin_ack,
            reserved,
        })
        .map_err(|_| ())
}

/// Debug/test spawn request: spawn `count` mobs of `type_id`, spread over a
/// generated area (`spread`) or crowded in one 2x2-chunk area.
/// The core handle. Cloneable; all methods are thread-safe.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    /// Protocol entity type id.
    pub type_id: u32,
    /// How many to spawn.
    pub count: usize,
    /// Spread over the area vs crowded in 2x2 chunks.
    pub spread: bool,
    /// Global index of the first entity in this batch (spread grid offset).
    pub start: usize,
    /// Total entities across all batches (spread grid size).
    pub total: usize,
}

mod recovery;
pub(crate) use recovery::*;

mod moderation_selector;
mod tick_loop;
/// A currently joined player selected for an edge moderation command.
pub type ModerationPlayer = (ConnectionId, uuid::Uuid, String);
use tick_loop::*;

mod bench;
pub use bench::*;

mod capture;
pub(crate) use capture::*;

mod journal;
pub(crate) use journal::*;

#[derive(Debug)]
pub struct Core {
    inner: Arc<CoreInner>,
}

#[derive(Debug)]
struct CoreInner {
    input_tx: Sender<PlayerInput>,
    spawn_tx: Sender<SpawnRequest>,
    ticket_tx: Sender<RemoteTicket>,
    moderation_selector_tx: Sender<moderation_selector::Request>,
    checkpoint_tx: std::sync::mpsc::Sender<std::sync::mpsc::Sender<bool>>,
    backup_tx: std::sync::mpsc::Sender<BackupJob>,
    input_dropped: AtomicU64,
    /// Weak: only the tick thread owns the `Sender`, so its exit (halt or
    /// shutdown) closes every subscriber instead of leaving them waiting.
    tick_tx: broadcast::WeakSender<TickOutput>,
    /// Read-only timings published after all active tick work, never blocking.
    timing_tx: broadcast::WeakSender<(u64, TickTimes)>,
    outbox: ReliableOutbox,
    shutdown_tx: std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    tick_handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    journal_handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    world_lease: std::sync::Mutex<Option<std::fs::File>>,
    worker_count: usize,
    /// World spawn fixed at open (manifest or spawn finder).
    world_spawn: [i32; 3],
    /// Immutable game data the tick runs on (recipes, items, blocks).
    registry: Arc<CoreRegistry>,
    plugin_commands: Arc<[(String, u8)]>,
    plugin_packages: Vec<mc_script::PluginPackage>,
    plugin_stats: Arc<crate::plugin::PluginStats>,
    build_gate: crate::build_precommit::Gateway,
    /// Fail-closed edge projection: only tick-observed unlocked sessions pass.
    require_plugin_login: bool,
    unlocked_sessions: Arc<parking_lot::RwLock<BTreeSet<ConnectionId>>>,
    /// Overworld far-terrain LOD tiles, read beside the chunk workers.
    lod: crate::lod::LodSource,
    /// Bounded immutable real-chunk terrain sources per dimension.
    terrain: [Arc<crate::terrain::TerrainSource>; 3],
    /// Stable opaque world identity for DH LevelInit (random hex, persisted
    /// through the native owner path, never seed-derived).
    terrain_epoch: String,
}

/// Immutable overworld terrain for this configuration and registry: the
/// vanilla router/density cache when mc-edge supplies its content directory,
/// the shared village plan source, and plugin worldgen rules.
///
/// Serving and the offline `world pregenerate` command build the same terrain
/// here, so pregenerated chunks match what a join would otherwise generate.
pub(crate) fn overworld_terrain(
    config: &CoreConfig,
    registry: &CoreRegistry,
) -> Result<mc_worldgen::TerrainGenerator, CoreError> {
    let mut terrain = mc_worldgen::TerrainGenerator::new(config.seed, Arc::clone(&registry.blocks));
    if let Some(vanilla_data_dir) = config.vanilla_data_dir.as_deref() {
        terrain
            .load_vanilla_worldgen(vanilla_data_dir)
            .map_err(CoreError::Worldgen)?;
        // Listing sites and placing their blocks/inhabitants share this
        // one validated plan source, built once before the tick thread.
        let tags = registry
            .worldgen_tags
            .as_ref()
            .expect("full vanilla registry has shared worldgen tags");
        let closure = Arc::new(
            mc_worldgen::village::load_village_closure(
                vanilla_data_dir,
                &mc_data::Identifier::parse("minecraft:villages")
                    .expect("static village structure set"),
                &registry.blocks,
            )
            .map_err(|error| CoreError::Worldgen(error.to_string()))?,
        );
        let decor = Arc::new(
            mc_worldgen::village::VillageDecor::compile(
                vanilla_data_dir,
                &closure,
                &mc_worldgen::vanilla_features::BlockSemantics::new(
                    &registry.blocks,
                    tags.as_ref(),
                ),
            )
            .map_err(|error| CoreError::Worldgen(error.to_string()))?,
        );
        let plans = mc_worldgen::village::plan_source::VillagePlanSource::new(
            closure,
            decor,
            config.seed,
            Arc::clone(&registry.blocks),
            tags.clone(),
            tags.clone(),
        )
        .map_err(|error| CoreError::Worldgen(error.to_string()))?;
        terrain = terrain.with_village_plans(Arc::new(plans));
        for set in ["ancient_cities", "trail_ruins", "trial_chambers"] {
            let closure = Arc::new(
                mc_worldgen::village::load_village_closure(
                    vanilla_data_dir,
                    &mc_data::Identifier::parse(format!("minecraft:{set}"))
                        .expect("static structure set"),
                    &registry.blocks,
                )
                .map_err(|error| CoreError::Worldgen(error.to_string()))?,
            );
            let decor = Arc::new(
                mc_worldgen::village::VillageDecor::compile(
                    vanilla_data_dir,
                    &closure,
                    &mc_worldgen::vanilla_features::BlockSemantics::new(
                        &registry.blocks,
                        tags.as_ref(),
                    ),
                )
                .map_err(|error| CoreError::Worldgen(error.to_string()))?,
            );
            let source = mc_worldgen::village::plan_source::VillagePlanSource::new(
                closure,
                decor,
                config.seed,
                Arc::clone(&registry.blocks),
                tags.clone(),
                tags.clone(),
            )
            .map_err(|error| CoreError::Worldgen(error.to_string()))?;
            terrain = terrain.with_jigsaw_source(Arc::new(source));
        }
    }
    let mut terrain = crate::plugin_startup::apply_worldgen(registry, terrain)?;
    if config.worldgen_gpu {
        match terrain.enable_gpu() {
            Ok(adapter) => tracing::info!(%adapter, "Overworld chunk density runs on the GPU"),
            Err(error) => {
                tracing::warn!(%error, "GPU worldgen unavailable; chunk density runs on the CPU");
            }
        }
    }
    Ok(terrain)
}

/// Serving and offline conversion share the same immutable Nether generator.
pub(crate) fn nether_terrain(
    config: &CoreConfig,
    registry: &CoreRegistry,
) -> Result<mc_worldgen::NetherGenerator, CoreError> {
    let mut terrain =
        mc_worldgen::NetherGenerator::try_new(config.seed, Arc::clone(&registry.blocks))
            .map_err(|error| CoreError::Worldgen(error.to_string()))?;
    if let Some(root) = config.vanilla_data_dir.as_deref() {
        terrain
            .load_vanilla_worldgen(
                root,
                Arc::clone(
                    registry
                        .worldgen_tags
                        .as_ref()
                        .expect("full vanilla registry has worldgen tags"),
                ),
            )
            .map_err(CoreError::Worldgen)?;
    }
    Ok(terrain)
}

pub(crate) fn end_terrain(
    config: &CoreConfig,
    registry: &CoreRegistry,
) -> Result<mc_worldgen::end::EndGenerator, CoreError> {
    let generator =
        mc_worldgen::end::EndGenerator::try_new(config.seed, Arc::clone(&registry.blocks))
            .map_err(|err| CoreError::Worldgen(err.to_string()))?;
    let Some(root) = config.vanilla_data_dir.as_deref() else {
        return Ok(generator);
    };
    generator
        .with_vanilla_data(root, &registry.blocks)
        .map_err(|err| CoreError::Worldgen(err.to_string()))
}

impl Core {
    /// Open the world: build registries, recover the journal, spawn the tick
    /// and journal threads.
    pub fn open(mut config: CoreConfig) -> Result<Self, CoreError> {
        stop::install_panic_hook();
        let is_stage = |path: &std::path::Path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with(".solaris-conversion-"))
        };
        if is_stage(&config.world_dir)
            || std::fs::canonicalize(&config.world_dir).is_ok_and(|path| is_stage(&path))
        {
            return Err(CoreError::Storage(
                "unpublished conversion staging directory cannot be opened by the runtime".into(),
            ));
        }
        std::fs::create_dir_all(&config.world_dir)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let world_lease = std::fs::File::open(&config.world_dir)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        world_lease.try_lock().map_err(|e| {
            CoreError::Storage(format!(
                "world {} is already leased: {e}",
                config.world_dir.display()
            ))
        })?;
        let path = config.world_dir.join("native/state.redb");
        let fresh = !path.exists();
        if fresh {
            match std::fs::read_dir(&config.world_dir) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry.map_err(|e| CoreError::Storage(e.to_string()))?;
                        if entry.file_name() != "datapacks" {
                            return Err(crate::native_checkpoint::import_required(&config));
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(CoreError::Storage(e.to_string())),
            }
        }
        let registry = Arc::new(CoreRegistry::load(config.vanilla_data_dir.as_deref())?);
        let mut plugin_runtime = crate::plugin::PluginRuntime::open(&config)?;
        if let Some(runtime) = &plugin_runtime
            && !runtime.required_auth_plugins.is_empty()
            && !config.require_plugin_login
        {
            tracing::info!(
                plugins = ?runtime.required_auth_plugins,
                "auth plugin deployed: new sessions stay locked until it unlocks them"
            );
            config.require_plugin_login = true;
        }
        let mut registry = Arc::try_unwrap(registry).expect("registry has no readers at startup");
        // Native settlement startup installs on the same pipeline whether or
        // not a plugin deployment exists: the default contribution carries no
        // packages, so `apply` admits only the native catalog. One call either
        // way; the runtime path additionally fills catalogs below.
        match &plugin_runtime {
            Some(runtime) => crate::plugin_startup::apply(
                &mut registry,
                runtime.contribution(),
                runtime.ore_profile(),
                runtime.client_bundles(),
            )?,
            None => crate::plugin_startup::apply(
                &mut registry,
                &mc_plugin_host::DeploymentContribution::default(),
                None,
                &[],
            )?,
        }
        if let Some(runtime) = &mut plugin_runtime {
            runtime.load_catalogs(&registry)?;
        }
        let datapack_root = config.world_dir.join("datapacks");
        let datapacks =
            crate::datapacks::PackSet::load(&datapack_root, crate::datapacks::Selection::default())
                .map_err(CoreError::Registry)?;
        datapacks
            .install_content(&mut registry)
            .map_err(CoreError::Registry)?;
        let registry = Arc::new(registry);
        let plugin_commands = plugin_runtime
            .as_ref()
            .map(crate::plugin::PluginRuntime::commands)
            .unwrap_or_default();
        let plugin_packages = plugin_runtime
            .as_ref()
            .map(|runtime| runtime.boundary.deployed_packages().to_vec())
            .unwrap_or_default();
        let plugin_stats = plugin_runtime
            .as_ref()
            .map(|runtime| Arc::clone(&runtime.stats))
            .unwrap_or_default();
        let (mut build_gate, build_queries) = crate::build_precommit::Gateway::new(
            plugin_runtime
                .as_ref()
                .map(|runtime| runtime.boundary.clone()),
        );
        if fresh {
            std::fs::create_dir_all(path.parent().expect("native DB parent"))
                .map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        let identity = crate::native_checkpoint::native_identity(&config);
        let store = Arc::new(
            if fresh {
                mc_world::native::NativeStore::create(&path, identity)
            } else {
                mc_world::native::NativeStore::open(&path, identity)
            }
            .map_err(|e| crate::native_checkpoint::open_refusal(&config, &path, e))?,
        );
        #[cfg(test)]
        crate::native_checkpoint::tests::register_store(&config, &store)?;
        let (mut tick_state, mut root_manifest, salt_minted) = recover_tick_state(
            &config,
            &registry,
            &store,
            fresh,
            plugin_runtime,
            build_queries,
        )?;
        tick_state.datapacks.install(datapacks, datapack_root);
        let world_spawn = tick_state.world.world_spawn();
        let terrain_epoch = crate::terrain::epoch_for_open(&tick_state)?;
        // Fresh epoch (minted when the stored record had none): stage it in
        // the in-memory globals image now, on the opening thread before any
        // tick/journal owner exists. The initial checkpoint below carries it
        // through the normal atomic transaction; reopened worlds already have
        // it in their record.
        crate::terrain::stage_epoch(&mut tick_state.native_globals, &terrain_epoch)?;
        let lod = crate::lod::LodSource::new(tick_state.world.chunk_source());
        let terrain_sources = |state: &crate::tick::TickState| {
            Arc::new(crate::terrain::TerrainSource::new(
                state.world.chunk_source(),
                terrain_epoch.clone(),
            ))
        };
        let terrain = [
            terrain_sources(&tick_state),
            terrain_sources(
                tick_state
                    .other_dimensions
                    .get(&crate::dimensions::Dimension::Nether)
                    .expect("nether dimension state"),
            ),
            terrain_sources(
                tick_state
                    .other_dimensions
                    .get(&crate::dimensions::Dimension::End)
                    .expect("end dimension state"),
            ),
        ];
        let world_dir = config.world_dir.clone();
        // Fresh worlds, and reopened pre-salt worlds that just minted their
        // entity salt (see install_salt), checkpoint before any tick.
        if fresh || (salt_minted && tick_state.tick > 0) {
            let (work_tx, work_rx) = journal_work_channel();
            let (done, _) = std::sync::mpsc::channel();
            let tick = if fresh { 0 } else { tick_state.tick };
            if !start_checkpoint(&mut tick_state, &work_tx, &done, tick) {
                return Err(CoreError::Storage(
                    "initial native checkpoint capture failed".into(),
                ));
            }
            let JournalWork::Checkpoint(job) = work_rx.recv().expect("initial capture") else {
                unreachable!()
            };
            let mut writer = if fresh {
                JournalWriter::open(&world_dir, config.journal_fsync_every_batches)?
            } else {
                JournalWriter::open_recovered(
                    &world_dir,
                    config.journal_fsync_every_batches,
                    root_manifest.tick,
                    tick_state.tick,
                )?
            };
            write_checkpoint(
                &world_dir,
                &job,
                &tick_state.plugins,
                &mut writer,
                &mut root_manifest,
            )?;
        }
        crate::plugin::start_events(&mut tick_state)?;
        if !plugin_packages.is_empty() {
            tracing::info!(target: "plugin.runtime_ready", loaded_packages = plugin_packages.len(), packages = ?plugin_packages, "native production component host loaded");
        }

        // Channels.
        let (input_tx, input_rx) = mpsc_channel::<PlayerInput>(config.input_queue_cap.max(4));
        let (spawn_tx, spawn_rx) = mpsc_channel::<SpawnRequest>(16);
        let (ticket_tx, ticket_rx) = mpsc_channel::<RemoteTicket>(16);
        let (moderation_selector_tx, moderation_selector_rx) = mpsc_channel(32);
        let (tick_tx, _) = broadcast::channel::<TickOutput>(config.broadcast_cap.max(1));
        let (timing_tx, _) = broadcast::channel::<(u64, TickTimes)>(16384);
        let (outbox_tx, outbox_rx) = std::sync::mpsc::channel();
        let (work_tx, work_rx) = journal_work_channel();
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        let (checkpoint_tx, checkpoint_rx) = std::sync::mpsc::channel();
        let (backup_tx, backup_rx) = std::sync::mpsc::channel();
        // At most one generated-chunk save queued or running: a slow disk
        // holds generated chunks in the world's bounded buffer, and the
        // bounded journal queue applies backpressure, instead of parking
        // work ahead of the journal's appends.
        let generated_busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Journal thread owns appends, fsyncs, checkpoints.
        let journal_world_dir = world_dir.clone();
        let journal_writer = JournalWriter::open_recovered(
            &world_dir,
            config.journal_fsync_every_batches,
            root_manifest.tick,
            tick_state.tick,
        )?;
        let checkpoint_every = config.checkpoint_every_ticks.max(1);
        let journal_busy = std::sync::Arc::clone(&generated_busy);
        let journal_budget = Arc::new(JournalBudget::new(JOURNAL_QUEUE_BYTES));
        let journal_loop_budget = Arc::clone(&journal_budget);
        // Seed the serialization mirror before the tick thread starts. Thereafter
        // only successfully appended post-images mutate it, in the work FIFO.
        let journal_plugins = tick_state.plugins.clone();
        let journal_handle = std::thread::Builder::new()
            .name("mc-core-journal".to_owned())
            .spawn(move || {
                journal_loop(
                    &journal_world_dir,
                    work_rx,
                    journal_writer,
                    &journal_busy,
                    &journal_loop_budget,
                    root_manifest,
                    journal_plugins,
                );
            })
            .map_err(|err| CoreError::Storage(err.to_string()))?;

        // Only known unlocked sessions pass the edge fence; a connection not
        // processed by the tick yet must never inherit administrative access.
        let unlocked_sessions = Arc::new(parking_lot::RwLock::new(BTreeSet::new()));
        let input_log = if config.input_log {
            // Plugin commands and async plugin results enter `run_tick` from
            // wasm threads and are not logged: say so in the header.
            let uncovered = tick_state
                .plugin_runtime
                .iter()
                .map(|_| "plugin_runtime".to_owned())
                .collect();
            Some(crate::input_log::InputLog::open(
                &world_dir,
                tick_state.tick,
                uncovered,
            )?)
        } else {
            None
        };
        let channels = TickChannels {
            checkpoint_every: checkpoint_every.max(1),
            pause_when_empty_seconds: config.pause_when_empty_seconds,
            input_rx,
            spawn_rx,
            ticket_rx,
            moderation_selector_rx,
            tick_broadcast: tick_tx.clone(),
            timing_broadcast: timing_tx.clone(),
            outbox: outbox_tx,
            work_tx,
            journal_budget,
            generated_busy,
            unlocked_sessions: Arc::clone(&unlocked_sessions),
            shutdown_rx,
            checkpoint_rx,
            backup_rx,
            input_log,
            terrain: [
                Arc::clone(&terrain[0]),
                Arc::clone(&terrain[1]),
                Arc::clone(&terrain[2]),
            ],
        };
        let tick_handle = std::thread::Builder::new()
            .name("mc-core-tick".to_owned())
            .spawn(move || {
                tick_loop(tick_state, channels);
            })
            .map_err(|err| CoreError::Storage(err.to_string()))?;
        build_gate.set_tick_owner(tick_handle.thread().clone(), input_tx.clone());

        // Snapshot the worker count once.
        let worker_count = crate::tick::worker_threads();
        Ok(Self {
            inner: Arc::new(CoreInner {
                input_tx,
                spawn_tx,
                ticket_tx,
                moderation_selector_tx,
                checkpoint_tx,
                backup_tx,
                input_dropped: AtomicU64::new(0),
                tick_tx: tick_tx.downgrade(),
                timing_tx: timing_tx.downgrade(),
                outbox: ReliableOutbox(parking_lot::Mutex::new(outbox_rx)),
                shutdown_tx: std::sync::Mutex::new(Some(shutdown_tx)),
                tick_handle: std::sync::Mutex::new(Some(tick_handle)),
                journal_handle: std::sync::Mutex::new(Some(journal_handle)),
                world_lease: std::sync::Mutex::new(Some(world_lease)),
                worker_count,
                world_spawn,
                registry,
                plugin_commands,
                plugin_packages,
                plugin_stats,
                build_gate,
                require_plugin_login: config.require_plugin_login,
                unlocked_sessions,
                lod,
                terrain,
                terrain_epoch,
            }),
        })
    }

    /// Immutable game data the core runs on: the edge announces recipe-book
    /// entries by index into `registry().recipes`, the ids `PlaceRecipe`
    /// carries back.
    #[must_use]
    pub fn registry(&self) -> &Arc<CoreRegistry> {
        &self.inner.registry
    }

    /// Manifest-owned command roots and required permission (0 player, 2 operator).
    #[must_use]
    pub fn plugin_commands(&self) -> &[(String, u8)] {
        &self.inner.plugin_commands
    }

    #[must_use]
    pub fn plugin_packages(&self) -> &[mc_script::PluginPackage] {
        &self.inner.plugin_packages
    }

    #[must_use]
    pub fn plugin_client_bundles(&self) -> &[mc_script::ClientBundle] {
        &self.inner.registry.plugin_client_bundles
    }

    #[must_use]
    pub fn plugin_host_operations(&self) -> u64 {
        self.inner.plugin_stats.operations.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn plugin_command_results(&self) -> u64 {
        self.inner
            .plugin_stats
            .command_results
            .load(Ordering::Relaxed)
    }

    /// Ask the production before-build chain off-tick, under one bounded
    /// deadline. Hook-free deployments return the original input directly.
    pub async fn admit_build(
        &self,
        input: PlayerInput,
    ) -> Result<PlayerInput, crate::build_precommit::BuildRefusal> {
        self.inner.build_gate.admit(input).await
    }

    /// Queue only a reliable client correction, never the refused mutation.
    pub fn resync_build(&self, refusal: crate::build_precommit::BuildRefusal) -> bool {
        self.submit(PlayerInput::BuildResync {
            request: refusal.correction,
        })
    }

    /// Admit the initial view, locale and Join together, reserving one FIFO Leave slot
    /// for every exit, including task cancellation before Play registration.
    pub fn join_session(
        &self,
        connection: ConnectionId,
        uuid: uuid::Uuid,
        name: String,
        permission_level: u8,
        view_distance: u8,
        locale: String,
    ) -> Result<PlayerSession, CoreError> {
        PlayerSession::admit(
            &self.inner.input_tx,
            connection,
            uuid,
            name,
            permission_level,
            view_distance,
            locale,
        )
    }

    /// Submit one decoded player input. Bounded and push-woken: returns
    /// `false` (and counts the drop) when the queue is full. Never blocks.
    /// Safe to call from Tokio I/O threads.
    pub fn submit(&self, input: PlayerInput) -> bool {
        match self.inner.input_tx.try_send(input) {
            Ok(()) => true,
            Err(_) => {
                self.inner.input_dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Subscribe to tick outputs. Bounded broadcast; laggards see `Lagged`
    /// and skip to the latest snapshot (world state is skippable). After the
    /// tick thread exits the receiver is already `Closed`.
    pub fn subscribe(&self) -> broadcast::Receiver<TickOutput> {
        match self.inner.tick_tx.upgrade() {
            Some(tx) => tx.subscribe(),
            None => broadcast::channel(1).1,
        }
    }

    /// Full active tick timings, published after checkpoint capture and
    /// before pacing, independently of world fanout. Bounded, nonblocking;
    /// `Lagged` means lost samples and invalidates affected measurement windows.
    /// Each timing remains paired with its original tick and wall start.
    pub fn subscribe_tick_times(&self) -> broadcast::Receiver<(u64, TickTimes)> {
        match self.inner.timing_tx.upgrade() {
            Some(tx) => tx.subscribe(),
            None => broadcast::channel(1).1,
        }
    }

    /// Number of dropped submit inputs since open.
    #[must_use]
    pub fn dropped_inputs(&self) -> u64 {
        self.inner.input_dropped.load(Ordering::Relaxed)
    }

    /// Fixed worker pool size derived once from the machine.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.inner.worker_count
    }

    /// World spawn block position (feet), fixed at open.
    #[must_use]
    pub fn world_spawn(&self) -> [i32; 3] {
        self.inner.world_spawn
    }

    /// One overworld far-terrain LOD tile (see [`crate::lod`]). Blocking store
    /// reads and router sampling on the caller's thread, never the tick.
    pub fn lod_tile(
        &self,
        tile_x: i32,
        tile_z: i32,
        level: u8,
        stored_only: bool,
    ) -> Result<Vec<crate::lod::LodSample>, CoreError> {
        self.inner.lod.tile(tile_x, tile_z, level, stored_only)
    }

    /// Stable opaque world identity for DH LevelInit: random per world,
    /// persisted through the native owner path, preserved across restarts,
    /// rotated on world reset. Never seed-derived. Cheap clone; samples no
    /// chunks.
    #[must_use]
    pub fn terrain_identity(&self) -> &str {
        &self.inner.terrain_epoch
    }

    /// Cheap per-dimension publication fence for edge cache invalidation
    /// (atomic load, no locks). Advanced by the tick owner only when a
    /// publication carries terrain changes (block writes, admissions,
    /// evictions); idle ticks leave it untouched. `u64::MAX` means busy
    /// (a terrain-changing publication is swapping its coherent bundle
    /// right now) or unknown dimension — never a real fence. The sampler
    /// rejects a busy or changed fence before/after sampling and retries.
    pub fn terrain_revision(&self, dimension: u32) -> u64 {
        self.inner
            .terrain
            .get(dimension as usize)
            .map_or(u64::MAX, |source| source.revision())
    }

    /// One immutable 6x6 real-chunk terrain section centered on the requested
    /// DH detail-6 leaf (see [`crate::terrain`]). Live committed resident
    /// chunks outrank the native store; missing chunks resolve through the
    /// normal load-or-generate path with light baked on the caller thread;
    /// corrupt stored coverage fails closed. Exactly 36 chunks, bounded and
    /// fixed. Blocking off-tick only: never call on the tick thread or a
    /// network thread — the presentation layer runs this on its shared
    /// bounded LOD worker lane.
    pub fn terrain_section(
        &self,
        dimension: u32,
        section_x: i32,
        section_z: i32,
    ) -> Result<crate::terrain::TerrainSection, CoreError> {
        self.inner.terrain.get(dimension as usize).map_or_else(
            || {
                Err(CoreError::Storage(format!(
                    "terrain section: unknown dimension {dimension}"
                )))
            },
            |source| source.section(dimension, section_x, section_z),
        )
    }

    /// The terrain publication's resident snapshot, for regression probes.
    #[cfg(test)]
    pub(crate) fn latest_snapshot(
        &self,
        dimension: crate::dimensions::Dimension,
    ) -> Option<Arc<crate::boundary::WorldSnapshot>> {
        self.inner.terrain[dimension as usize].snapshot()
    }

    /// The overworld generator's far-terrain program, zlib-compressed, built
    /// once on first use (see [`crate::lod::LodSource::program`]). It reveals
    /// the seed's terrain noise: callers send it only on operator opt-in.
    pub fn lod_program(&self) -> Option<std::sync::Arc<Vec<u8>>> {
        self.inner.lod.program()
    }

    /// Edge-side effects fail closed until the native tick has observed this
    /// exact session unlocked. Unknown and not-yet-joined connections are locked
    /// when authentication is required; configuration-off is unchanged.
    #[must_use]
    pub fn session_locked(&self, connection: ConnectionId) -> bool {
        self.inner.require_plugin_login
            && !self.inner.unlocked_sessions.read().contains(&connection)
    }

    /// Add a remote ticket. Over-budget requests are refused explicitly.
    /// Budget: the ticket's chunk count must fit `budget_chunks`.
    pub fn add_ticket(&self, ticket: RemoteTicket) -> Result<(), CoreError> {
        let want = crate::world::chunks_in_radius(ticket.chunk, ticket.radius as i32);
        if want.len() > ticket.budget_chunks {
            return Err(CoreError::TicketOverBudget(format!(
                "{} wants {} chunks over budget {}",
                ticket.owner,
                want.len(),
                ticket.budget_chunks
            )));
        }
        self.inner
            .ticket_tx
            .try_send(ticket)
            .map_err(|_| CoreError::TickStopped)
    }

    /// Debug/test spawn: queue a mob spawn request drained by the tick thread.
    /// Returns `false` when the spawn queue is full. In-crate use only
    /// (`core-bench`, `*_tests.rs`).
    #[doc(hidden)]
    pub fn debug_spawn(&self, request: SpawnRequest) -> bool {
        self.inner.spawn_tx.try_send(request).is_ok()
    }

    /// Tokenize a moderation command with the native balanced selector lexer.
    pub fn moderation_command_words(command: &str) -> Result<Vec<&str>, String> {
        crate::commands::command_words(command).map_err(|error| error.message())
    }

    /// Resolve a player selector on the tick owner, using the command source's
    /// position and dimension. A closed or full queue fails closed.
    pub async fn resolve_moderation_selector(
        &self,
        source: ConnectionId,
        selector: String,
    ) -> Result<Vec<ModerationPlayer>, String> {
        let (reply, receiver) = tokio::sync::oneshot::channel();
        self.inner
            .moderation_selector_tx
            .try_send(moderation_selector::Request {
                source,
                selector,
                reply,
            })
            .map_err(|_| "Selector resolution unavailable".to_owned())?;
        receiver
            .await
            .map_err(|_| "Selector resolution unavailable".to_owned())?
    }

    /// Try-drain reliably queued per-connection changes (never skipped).
    /// The tick hands off owned batches before broadcasting; per-connection
    /// order is preserved even when the edge misses world publications.
    pub fn try_drain_outbox(&self) -> Vec<(ConnectionId, Vec<PlayerChange>)> {
        self.inner.outbox.drain()
    }

    /// Force a durable checkpoint through the tick capture and journal writer.
    /// Blocking; network callers must use a blocking task.
    pub fn checkpoint(&self) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        self.inner.checkpoint_tx.send(tx).is_ok() && rx.recv().unwrap_or(false)
    }

    /// Hot backup of the running world into the existing empty directory `out`:
    /// a forced checkpoint, then a copy of that native database and the WAL
    /// appended since (see [`crate::hot_backup`]). Blocking; network callers
    /// must use a blocking task. The tick is not paused. The caller owns
    /// `out`'s manifest and cleanup after an error.
    pub fn backup(&self, out: &std::path::Path) -> Result<crate::hot_backup::HotBackup, CoreError> {
        if !self.checkpoint() {
            return Err(CoreError::Storage(
                "hot backup: checkpoint failed or the core stopped".into(),
            ));
        }
        let (reply, result) = std::sync::mpsc::channel();
        let job = BackupJob {
            out: out.to_path_buf(),
            reply,
        };
        if self.inner.backup_tx.send(job).is_err() {
            return Err(CoreError::Storage("hot backup: the core stopped".into()));
        }
        result
            .recv()
            .unwrap_or_else(|_| Err(CoreError::Storage("hot backup: the core stopped".into())))
    }

    /// Whether graceful shutdown has been requested through this core handle.
    pub fn shutdown_requested(&self) -> bool {
        self.inner
            .shutdown_tx
            .lock()
            .expect("core shutdown state")
            .is_none()
    }

    /// Graceful shutdown: signals the tick thread, then joins tick and
    /// journal in order. Returns only after the final journal flush (and any
    /// final checkpoint) is durable. Idempotent.
    pub fn shutdown(&self) {
        if let Ok(mut guard) = self.inner.shutdown_tx.lock()
            && let Some(tx) = guard.take()
            && tx.send(()).is_err()
        {
            tracing::error!(
                cause = "tick shutdown channel closed",
                "core already stopped"
            );
        }
        // Join tick first (it sends the journal Shutdown after its final
        // checkpoint flush), then the journal (final flush on exit).
        if let Ok(mut guard) = self.inner.tick_handle.lock()
            && let Some(handle) = guard.take()
        {
            handle.thread().unpark();
            stop::join(handle);
        }
        if let Ok(mut guard) = self.inner.journal_handle.lock()
            && let Some(handle) = guard.take()
        {
            stop::join(handle);
        }
        if let Ok(mut lease) = self.inner.world_lease.lock() {
            lease.take();
        }
    }
}

/// Insert one remote chunk ticket on the tick thread.
pub(crate) fn apply_ticket(state: &mut TickState, owner: String, x: i32, z: i32, radius: u32) {
    state
        .tickets
        .insert(owner, (crate::boundary::ChunkPos { x, z }, radius as i32));
}

/// Connections of `inputs` that any dimension's auth gate holds locked.
fn locked_connections(state: &TickState, inputs: &[PlayerInput]) -> Vec<ConnectionId> {
    let mut locked = Vec::new();
    for input in inputs {
        let connection = input.connection();
        if !locked.contains(&connection)
            && std::iter::once(state)
                .chain(state.other_dimensions.values().map(|world| &**world))
                .any(|world| world.session_locked(connection))
        {
            locked.push(connection);
        }
    }
    locked
}

/// Apply one debug spawn request on the tick thread.
pub(crate) fn apply_spawn_request(state: &mut TickState, request: &SpawnRequest) {
    let air = state.registry.states.air;
    // Centred on the world spawn, which the spawn finder put on dry land:
    // a fixed origin can sit in ocean under vanilla worldgen.
    let [spawn_x, _, spawn_z] = state.world.world_spawn();
    let (origin_x, origin_z) = (f64::from(spawn_x), f64::from(spawn_z));
    let side = (request.total.max(request.count) as f64).sqrt().ceil() as i64;
    for i in 0..request.count {
        let (dx, dz) = if request.spread {
            // One grid over the FULL count: batches carry their global
            // offset so 50 batches tile ~632x632 blocks, not one patch.
            let global = request.start as i64 + i as i64;
            let ix = global % side - side / 2;
            let iz = global / side - side / 2;
            (ix as f64 * 2.0, iz as f64 * 2.0)
        } else {
            let ix = i as f64 % 32.0;
            let iz = (i as f64 / 32.0) % 32.0;
            (ix - 16.0, iz - 16.0)
        };
        let (x, z) = (origin_x + 0.5 + dx, origin_z + 0.5 + dz);
        let y = surface_height(&state.world, air, x, z);
        state.spawn_mob(request.type_id, [x, y, z], 20.0);
    }
}

/// Highest free surface at `(x, z)` for debug spawns. Anchored scan:
/// probe down from a plausible surface estimate instead of the sky.
fn surface_height(world: &ChunkWorld, air: u32, x: f64, z: f64) -> f64 {
    let bx = x.floor() as i32;
    let bz = z.floor() as i32;
    // Find any solid anchor near the typical surface band first.
    let mut y = 100;
    while y > -64 {
        match world.block_at(bx, y, bz) {
            Some(s) if s == air => y -= 1,
            Some(_) => break,
            None => return 80.0,
        }
        if y < 40 {
            break;
        }
    }
    (y + 1).max(-63) as f64
}

#[cfg(test)]
#[path = "native_checkpoint_crash_tests.rs"]
mod native_checkpoint_crash_tests;

#[cfg(test)]
#[path = "native_component_26_3_tests.rs"]
mod native_component_26_3_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "persistence_crash_tests.rs"]
mod persistence_crash_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "native_population_tests.rs"]
mod native_population_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "native_checkpoint_noop_tests.rs"]
mod native_checkpoint_noop_tests;

/// Runtime storage paths: native database and tick WAL only.
pub fn checkpoint_paths(world_dir: &std::path::Path) -> HashMap<String, PathBuf> {
    let mut paths = HashMap::new();
    paths.insert("native_db".to_owned(), world_dir.join("native/state.redb"));
    paths.insert("journal_dir".to_owned(), world_dir.join("journal"));
    paths
}

/// Checkpoint block-entity base: block pos -> contents image.
pub(crate) type CheckpointBlockEntities =
    std::collections::BTreeMap<(i32, i32, i32), crate::journal::ReplayBlockEntity>;

/// One player's checkpoint record: the full [`ReplayPlayer`] state (same
/// fields as the journal post-image: inventory, vitals, xp, mode).
pub struct CheckpointPlayerData {
    pub uuid: String,
    pub record: crate::journal::ReplayPlayer,
}

/// Read-side handle for tests: open state without spawning threads.
pub struct CorePaths {
    /// World directory.
    pub world_dir: PathBuf,
}

impl CorePaths {
    /// Journal directory.
    #[must_use]
    pub fn journal_dir(&self) -> PathBuf {
        self.world_dir.join("journal")
    }
}

#[cfg(test)]
#[path = "core/motion_tests.rs"]
mod motion_tests;

#[cfg(test)]
#[path = "entity_state_persistence_tests.rs"]
mod entity_state_persistence_tests;

#[cfg(test)]
#[path = "mounts_persistence_tests.rs"]
mod mounts_persistence_tests;

#[cfg(test)]
#[path = "raids_recovery_tests.rs"]
mod raids_recovery_tests;

#[cfg(test)]
#[path = "mob_lanes_recovery_tests.rs"]
mod mob_lanes_recovery_tests;

#[cfg(test)]
#[path = "sculk_checkpoint_tests.rs"]
mod sculk_checkpoint_tests;

#[cfg(test)]
#[path = "core/ticket_checkpoint_tests.rs"]
mod ticket_checkpoint_tests;

#[cfg(test)]
#[path = "core/difficulty_persistence_tests.rs"]
mod difficulty_persistence_tests;

#[cfg(test)]
#[path = "core_outbox_tests.rs"]
mod core_outbox_tests;

#[cfg(test)]
#[path = "core_session_lock_tests.rs"]
mod core_session_lock_tests;

#[cfg(test)]
#[path = "core_journal_backpressure_tests.rs"]
mod journal_backpressure_tests;

#[cfg(test)]
#[path = "core/village_generation_tests.rs"]
mod village_generation_tests;
