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

#[derive(Debug)]
pub struct Core {
    inner: Arc<CoreInner>,
}

#[derive(Debug)]
struct CoreInner {
    input_tx: Sender<PlayerInput>,
    spawn_tx: Sender<SpawnRequest>,
    ticket_tx: Sender<RemoteTicket>,
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
    crate::plugin_startup::apply_worldgen(registry, terrain)
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

fn load_dimension_state(
    config: &CoreConfig,
    registry: Arc<CoreRegistry>,
    dimension: crate::dimensions::Dimension,
    root_manifest: &CheckpointManifest,
    replay: Replay,
    store: Arc<mc_world::native::NativeStore>,
    settlement_globals: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Result<TickState, CoreError> {
    let manifest_now = if dimension == crate::dimensions::Dimension::Overworld {
        root_manifest.clone()
    } else {
        root_manifest
            .dimensions
            .get(&dimension)
            .map(|manifest| (**manifest).clone())
            .unwrap_or_default()
    };
    let manifest_tick = root_manifest.tick;
    crate::mounts::persistence::validate_replay(&replay)?;
    let recovered_tick = replay.tick.max(manifest_tick);

    // Worldgen: load the same vanilla router/density cache used by the
    // production v1 startup when mc-edge supplies its content directory.
    let terrain = if dimension == crate::dimensions::Dimension::Overworld {
        Some(overworld_terrain(config, &registry)?)
    } else {
        None
    };
    let world_spawn = if let Some(spawn) = manifest_now.world_spawn {
        spawn
    } else if let Some(terrain) = &terrain {
        let spawn = terrain
            .locate_safe_spawn()
            .ok_or_else(|| CoreError::Worldgen("no bounded dry land spawn found".into()))?;
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z]
    } else {
        [0, 80, 0]
    };
    let generator: Arc<dyn ChunkGenerator> = match dimension {
        crate::dimensions::Dimension::Overworld => Arc::new(terrain.expect("overworld terrain")),
        crate::dimensions::Dimension::Nether => Arc::new(nether_terrain(config, &registry)?),
        crate::dimensions::Dimension::End => Arc::new(end_terrain(config, &registry)?),
    };
    let mut world = ChunkWorld::new_native(
        config.seed,
        store,
        crate::native_checkpoint::dimension_id(dimension),
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        dimension.geometry(),
    );
    world.set_world_spawn(world_spawn, manifest_now.world_spawn_rotation);
    world.restore_inhabited_time(&replay, Some(&manifest_now));
    world.restore_sim_debt(&replay, Some(&manifest_now))?;

    // Replay absolute block post-images into the overlay, then fold once
    // chunks load (fold keeps homeless writes in the overlay).
    for ((x, y, z), state) in &replay.blocks {
        world.set_block_catch_up(*x, *y, *z, *state);
    }
    if !replay.blocks.is_empty() {
        let want = replay
            .blocks
            .keys()
            .map(|(x, _, z)| {
                (
                    crate::boundary::ChunkPos {
                        x: x.div_euclid(16),
                        z: z.div_euclid(16),
                    },
                    0,
                )
            })
            .collect();
        world.ensure_blocking(&want);
        world.fold_overlay();
    }
    // Fold replayed blocks before installing their opaque entities:
    // replacing a block clears any old sign NBT in the base chunk.
    for ((x, y, z), nbt) in &replay.opaque_block_entities {
        if let Some(bytes) = nbt {
            let tag = mc_nbt::read_network(&mut bytes.as_slice()).map_err(|error| {
                CoreError::Storage(format!("storage block entity at ({x},{y},{z}): {error}"))
            })?;
            if let mc_nbt::Tag::Compound(fields) = tag {
                mc_data::storage_blocks::validate(&fields, &registry.items).map_err(|error| {
                    CoreError::Storage(format!("storage block entity at ({x},{y},{z}): {error}"))
                })?;
            }
        }
        let chunk = crate::boundary::ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        world.ensure_blocking(&[(chunk, 0)].into_iter().collect());
        world.fold_overlay();
        world.set_opaque_block_entity(
            crate::boundary::BlockPos {
                x: *x,
                y: *y,
                z: *z,
            },
            nbt.clone(),
        );
    }
    // Replay adoption is already durable. Do not re-journal its replacement
    // removals on the first live tick after installing the final opaque images.
    let _ = world.take_block_entity_removals();

    let mut tick_state = TickState::new(
        Arc::clone(&registry),
        world,
        i32::from(config.simulation_distance),
        config.player_outbox_cap,
    );
    tick_state.require_plugin_login = config.require_plugin_login;
    tick_state.max_airtime_ticks = config.max_airtime_ticks;
    tick_state.enable_command_block = config.enable_command_block;
    tick_state.mob_griefing = config.mob_griefing;
    tick_state.config_view_distance = i32::from(config.view_distance);
    tick_state.dimension = dimension;
    tick_state.tick = recovered_tick;
    // Settlement registries are per-dimension authorities: every dimension
    // hydrates its own rows (matched by home dimension) folded with its
    // dimension-routed WAL tail. Membership columns hydrate with entities below.
    {
        let sequence = match crate::settlements::persistence::checkpoint_sequence(
            settlement_globals,
            root_manifest.tick,
        ) {
            Ok(sequence) => sequence,
            // Fresh worlds and unit fixtures predate the sequence row and
            // hold no settlement rows either; anything else is corruption.
            Err(_)
                if !settlement_globals
                    .keys()
                    .any(|key| key == "settlement_sequence" || key.starts_with("settlement/")) =>
            {
                0
            }
            Err(error) => return Err(error),
        };
        crate::settlements::persistence::hydrate_dimension(
            &mut tick_state,
            settlement_globals,
            &replay,
            sequence,
        )?;
    }
    tick_state.plugins = replay.plugins;
    tick_state.plugins.validate()?;
    tick_state.block_ticks = crate::block_ticks::BlockTicks::from_image(
        replay.block_ticks,
        Some(&manifest_now.block_ticks),
    );
    tick_state.fluid_ticks =
        crate::fluids::FluidTicks::from_image(replay.fluid_ticks, Some(&manifest_now.fluid_ticks));
    // Startup chunks predate normal admission callbacks. Import their
    // native delays only when no manifest/WAL coverage already owns them.
    for chunk in tick_state.world.chunks() {
        tick_state.fluid_ticks.hydrate(chunk, recovered_tick);
    }
    tick_state.gamerules = replay.gamerules.into_iter().collect();
    replay.border.validate()?;
    tick_state.border = replay.border;
    tick_state.difficulty = replay.difficulty;
    if let Some(climate) = replay.climate {
        climate.restore(&mut tick_state)?;
    }
    tick_state.scoreboard = replay.scoreboard.unwrap_or_default();
    tick_state.maps = crate::maps::Maps::restore(
        replay.saved_maps,
        crate::maps::sequence(settlement_globals)?,
    )?;
    tick_state.bossbars = replay
        .bossbars
        .as_deref()
        .map(crate::bossbar::BossBars::from_bytes)
        .transpose()
        .map_err(crate::config::CoreError::Storage)?
        .unwrap_or_default();
    tick_state.generated_entity_claims = replay.generated_entity_claims.clone();
    tick_state.end_fight = replay.end_fight.unwrap_or_default();
    tick_state.raids = replay.raids.unwrap_or_default();
    tick_state.raids.validate()?;
    tick_state.trader_spawner = replay.wandering_trader.unwrap_or_default();
    tick_state.datapacks.restore(replay.scheduled_functions)?;
    tick_state.block_log = crate::block_log::BlockLog::restore(config.block_log, replay.block_log);

    // Replay player + entity records. Full player state (inventory,
    // vitals, xp, mode) restores here once the inventory/survival modules
    // wire into TickState (step 2); today position/health/food restore and
    // the rest rides the replay record.
    for (uuid_str, player) in &replay.players {
        if player.dimension != dimension {
            continue;
        }
        crate::ender_chest::contents(&player.opaque_player_fields, &registry.items)?;
        let uuid = uuid_str.parse::<uuid::Uuid>().map_err(|err| {
            CoreError::Storage(format!(
                "{}: recovered player UUID {uuid_str}: {err}",
                dimension.name()
            ))
        })?;
        tick_state.offline_players.insert(uuid, player.clone());
    }
    let mut max_wire_id: i32 = 0;
    // Drain the replay owner as live rows are restored: retaining both complete
    // populations multiplies peak memory during cold-column growth.
    for (id, record) in replay.entities {
        max_wire_id = max_wire_id.max(id);
        crate::vehicles::validate_replay(&registry, &record)?;
        restore_entity(&mut tick_state, id, &record)?;
    }
    for (pos, record) in &replay.block_entities {
        tick_state
            .block_entities
            .hydrate(*pos, || crate::tick::BlockEntityState {
                slots: record.slots.clone(),
                furnace: record.furnace.as_ref().map(|image| {
                    crate::inventory::furnace::FurnaceState {
                        lit_remaining: image.lit_remaining,
                        lit_total: image.lit_total,
                        cook_progress: image.cook_progress,
                        cook_total: image.cook_total,
                        recipes_used: Arc::clone(&image.recipes_used),
                    }
                }),
            });
    }
    let _ = max_wire_id;
    for (owner, (chunk, (_, radius))) in &replay.tickets {
        tick_state.tickets.insert(
            owner.clone(),
            (
                crate::boundary::ChunkPos {
                    x: chunk[0],
                    z: chunk[1],
                },
                *radius,
            ),
        );
    }
    crate::end_fight::restore(&mut tick_state)?;
    crate::raids::restore(&mut tick_state)?;
    crate::end_portals::validate_gateways(&tick_state).map_err(CoreError::Storage)?;
    crate::conduit::validate_loaded(&tick_state).map_err(CoreError::Storage)?;
    crate::sculk::validate(&tick_state).map_err(CoreError::Storage)?;
    crate::shrieker::validate_loaded(&tick_state).map_err(CoreError::Storage)?;
    if let Some(error) = tick_state.world.load_errors().next() {
        return Err(CoreError::Storage(error.to_string()));
    }
    Ok(tick_state)
}

#[cfg(test)]
pub(crate) fn recover_dimension_state(
    config: &CoreConfig,
    dimension: crate::dimensions::Dimension,
) -> Result<TickState, CoreError> {
    let store = crate::native_checkpoint::tests::fixture_store(config)?;
    let registry = Arc::new(CoreRegistry::load(config.vanilla_data_dir.as_deref())?);
    let reader = store
        .checkpoint_reader()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let mut recovered =
        crate::native_checkpoint::recover_with_reader(config, registry.clone(), reader)?;
    let replay = recovered
        .dimensions
        .remove(&dimension)
        .ok_or_else(|| CoreError::Storage(format!("missing recovered dimension {dimension:?}")))?;
    let mut state = load_dimension_state(
        config,
        registry,
        dimension,
        &recovered.manifest,
        replay,
        store,
        &recovered.checkpoint.globals,
    )?;
    state.native_globals = recovered.checkpoint.globals;
    crate::settlements::persistence::refuse_legacy_plugin_state(&state)?;
    crate::settlements::persistence::finish_recovery(&mut state)?;
    Ok(state)
}

/// Recover the three dimension worlds and the global native state from the
/// store: the checkpoint-recovery half of [`Core::open`], shared with the
/// input-log replayer. No thread is started.
pub(crate) fn recover_tick_state(
    config: &CoreConfig,
    registry: &Arc<CoreRegistry>,
    store: &Arc<mc_world::native::NativeStore>,
    fresh: bool,
    plugin_runtime: Option<crate::plugin::PluginRuntime>,
    build_queries: Option<crate::build_precommit::PreviewQueries>,
) -> Result<(TickState, CheckpointManifest, bool), CoreError> {
    let (root_manifest, mut replays, globals) = if fresh {
        store
            .initialize_dimensions(
                &crate::native_checkpoint::generation_records(config)?["dimensions"],
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut manifest = CheckpointManifest {
            domain_versions: crate::native_checkpoint::domain_versions(),
            ..Default::default()
        };
        for dimension in [
            crate::dimensions::Dimension::Nether,
            crate::dimensions::Dimension::End,
        ] {
            manifest
                .dimensions
                .insert(dimension, Box::new(CheckpointManifest::default()));
        }
        let mut globals = crate::native_checkpoint::generation_records(config)?;
        for key in ["level_metadata", "conversion_extras"] {
            globals.insert(
                key.into(),
                crate::native_checkpoint::tag_record(0, &mc_nbt::Tag::Compound(Vec::new()))?,
            );
        }
        globals.insert(
            "settlement_sequence".into(),
            crate::native_checkpoint::record(
                0,
                serde_json::to_vec(&0u64).map_err(|e| CoreError::Storage(e.to_string()))?,
            ),
        );
        (
            manifest,
            [
                crate::dimensions::Dimension::Overworld,
                crate::dimensions::Dimension::Nether,
                crate::dimensions::Dimension::End,
            ]
            .into_iter()
            .map(|dimension| (dimension, Replay::default()))
            .collect::<std::collections::BTreeMap<_, _>>(),
            globals,
        )
    } else {
        // Opening the sole writer may repair redb's allocator bookkeeping.
        // Semantic refusal below never publishes a new checkpoint transaction.
        let recovered = crate::native_checkpoint::recover_with_reader(
            config,
            Arc::clone(registry),
            store
                .checkpoint_reader()
                .map_err(|e| CoreError::Storage(e.to_string()))?,
        )?;
        (
            recovered.manifest,
            recovered.dimensions,
            recovered.checkpoint.globals,
        )
    };
    let mut tick_state = load_dimension_state(
        config,
        Arc::clone(registry),
        crate::dimensions::Dimension::Overworld,
        &root_manifest,
        replays
            .remove(&crate::dimensions::Dimension::Overworld)
            .expect("root replay"),
        Arc::clone(store),
        &globals,
    )?;
    for dimension in [
        crate::dimensions::Dimension::Nether,
        crate::dimensions::Dimension::End,
    ] {
        let child = load_dimension_state(
            config,
            Arc::clone(registry),
            dimension,
            &root_manifest,
            replays.remove(&dimension).expect("child replay"),
            Arc::clone(store),
            &globals,
        )?;
        tick_state
            .other_dimensions
            .insert(dimension, Box::new(child));
    }
    tick_state.native_globals = globals;
    let salt_minted = crate::entity_uuid::install_salt(&mut tick_state)?;
    crate::settlements::persistence::refuse_legacy_plugin_state(&tick_state)?;
    crate::settlements::persistence::finish_recovery(&mut tick_state)?;
    tick_state.plugin_runtime = plugin_runtime;
    tick_state.build_queries = build_queries;
    crate::plugin_player::restore(&mut tick_state)?;
    crate::plugin::validate_startup(
        &mut tick_state,
        fresh || !root_manifest.domain_versions.contains_key("plugins"),
    )?;
    crate::dimensions::validate_identity(&mut tick_state)?;
    Ok((tick_state, root_manifest, salt_minted))
}

impl Core {
    /// Open the world: build registries, recover the journal, spawn the tick
    /// and journal threads.
    pub fn open(config: CoreConfig) -> Result<Self, CoreError> {
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
            crate::datapacks::PackSet::load(&datapack_root).map_err(CoreError::Registry)?;
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

    /// Retire a submitted Join without shedding its Leave on a full gameplay
    /// queue. The edge retains its admission reservation until this completes.
    /// A closed receiver means the authoritative owner has already stopped.
    pub async fn leave(&self, connection: ConnectionId) {
        let _ = self
            .inner
            .input_tx
            .send(PlayerInput::Leave { connection })
            .await;
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

/// Restore one journaled entity record.
pub(crate) fn restore_entity(
    state: &mut TickState,
    id: i32,
    record: &crate::journal::ReplayEntity,
) -> Result<(), CoreError> {
    let entity_id = EntityId(id);
    let uuid = crate::dimensions::recovery::validate_entity(state, id, record)?;
    state.entities.set_next_wire_id(id);
    let kind = if record.type_id == state.registry.item_type_id {
        EntityKind::Item {
            type_id: record.type_id,
        }
    } else if record.appearance.experience_orb.is_some() {
        EntityKind::ExperienceOrb {
            type_id: record.type_id,
        }
    } else {
        EntityKind::Mob {
            type_id: record.type_id,
        }
    };
    let mut appearance = restore_appearance(record);
    // Item entities mirror their stack on the appearance (as live spawns do):
    // views read the stack from there.
    if matches!(kind, EntityKind::Item { .. }) {
        appearance.item = record.item.clone();
        appearance.health = record.health;
    }
    let appearance = Arc::new(appearance);
    let (movement, half_width, height, speed, damage, sight, reach) =
        if let Some(blueprint) = state.registry.blueprint(record.type_id) {
            (
                blueprint.movement,
                blueprint.half_width,
                blueprint.height,
                blueprint.params.speed,
                blueprint.params.attack_damage,
                blueprint.sensors.sight_range,
                blueprint.sensors.attack_reach,
            )
        } else {
            // Non-mob owners (including falling blocks and TNT) do not have
            // an AI blueprint. Their physical size still comes from the
            // complete validated registry, and their controller restores
            // its movement state below.
            let contract =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(record.type_id)
                    .expect("retained entity type validated before recovery");
            let dimensions = mc_data::entity_types::canonical_dimensions(contract);
            (
                crate::blueprint::MovementClass::Stationary,
                dimensions.half_width(),
                dimensions.height,
                0.0,
                0.0,
                0.0,
                0.0,
            )
        };
    state.entities.spawn(
        entity_id,
        uuid,
        kind,
        crate::entities::ColdFields {
            // Ordinary motion is checkpoint-durable; existing crossing images
            // carry position so replay keeps the right chunk-clock authority.
            pos: record.pos,
            vel: record.vel,
            yaw: record.yaw,
            pitch: record.pitch,
            ground: record.ground,
            health: record.health,
            breed_cooldown: record.breed_cooldown,
            pickup_delay: record.pickup_delay,
            item: record.item.as_ref().map(|stack| crate::entities::StackRow {
                item_id: stack.item_id,
                count: stack.count,
                stack: Some(stack.clone()),
            }),
            appearance,
            movement,
            half_width,
            height,
            speed,
            attack_damage: damage,
            sight_range: sight,
            attack_reach: reach,
        },
    );
    if let Some(slot) = state.entities.slot_of(entity_id) {
        if record.state.entity_life.child_ticks != 0 {
            state.entities.set_half_width(slot, half_width * 0.5);
        }
        state
            .entities
            .set_entity_life(slot, record.state.entity_life.clone());
        let (body, row) = state.entities.parts_mut(slot);
        body.alive = record.alive;
        row.age = record.age;
        row.protected_until = record.state.protected_until;
        row.active_effects = record.active_effects.clone();
        row.villager = record.villager.clone().map(Box::new);
        row.persistent = record.state.persistent;
        row.no_action_time = record.state.no_action_time;
        body.air_supply = record.state.air_supply;
        body.burn_ticks = record.state.burn_ticks;
        row.love = record.state.love;
        row.loot = record.state.loot.clone();
        row.controls = record.state.controls;
        row.falling = record.state.falling;
        row.tnt = record.state.tnt;
        row.special_mob = record.state.special_mob.clone().map(Box::new);
        row.bee = record.state.bee.clone().map(Box::new);
        row.allay = record.state.allay.clone().map(Box::new);
        row.opaque_entity_fields = record.state.opaque_entity_fields.clone();
        crate::entity_life::restore_appearance(body, row);
        if body.burn_ticks != 0 {
            Arc::make_mut(&mut row.appearance).shared_flags |= 1;
        }
        if row.entity_life.child_ticks != 0 {
            Arc::make_mut(&mut row.appearance).baby = true;
            body.height *= 0.5;
        }
        crate::entity_loot::restore(state, slot);
        crate::armor_stand::restore(state, slot);
        crate::item_frame::restore(state, slot);
        crate::vehicles::restore(state, slot);
        crate::bees::appearance(state, slot);
        crate::allay::appearance(state, slot);
        crate::special_mobs::restore(state, slot);
        crate::leash::initialize(state, slot);
        if record.state.falling.is_some() {
            crate::falling::configure(state, entity_id);
        }
        if record.state.tnt.is_some() {
            crate::tnt::restore(state, slot);
        }
        let mut projectile = record.projectile.clone();
        if projectile.is_none() {
            // Initial v2 skeleton arrows had only metadata/age. Preserve their
            // stuck state and disallowed pickup rather than minting an item.
            if let Some(kind) = state
                .registry
                .blueprint(record.type_id)
                .and_then(|blueprint| {
                    crate::projectiles::ProjectileKind::from_name(&blueprint.name)
                })
                .filter(|kind| kind.arrow())
            {
                let mut migrated =
                    crate::projectiles::ProjectileState::new(kind, None, mc_data::ItemStack::EMPTY);
                migrated.age = record.age;
                migrated.mob_damage = true;
                migrated.stuck = record.appearance.extra.iter().any(|(index, value)| {
                    *index == crate::projectiles::ARROW_IN_GROUND_INDEX
                        && matches!(value, crate::journal::EntityDataValueImage::Boolean(true))
                });
                projectile = Some(migrated);
            }
        }
        if let Some(projectile) = &mut projectile {
            crate::projectiles::persistence::restore_clock(projectile, state.tick);
        }
        state.entities.row_mut(slot).projectile = projectile.map(Box::new);
        crate::villager::sync_appearance(state, slot);
        let pos = state.entities.pos(slot);
        let chunk = crate::boundary::ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        };
        state.journaled_entity_state.insert(
            entity_id,
            (
                state.tick,
                chunk,
                Arc::new(crate::journal::EntityStateImage::from_row(
                    state.entities.body(slot),
                    state.entities.row(slot),
                )),
            ),
        );
        // Seed the villager baseline too: without it the first journal batch
        // re-images every restored villager (500k: ~2.6 GB, a 5.7 s first tick).
        // A row whose live state differs from the restored one still journals.
        if let Some(villager) = &state.entities.row(slot).villager {
            state.journaled_villagers.insert(
                entity_id,
                (
                    state.tick,
                    Box::new(crate::villager::persistence::journal_image(villager)),
                ),
            );
        }
    }
    Ok(())
}

/// Restore an [`EntityAppearance`] from its journal image.
fn restore_appearance(record: &crate::journal::ReplayEntity) -> crate::boundary::EntityAppearance {
    let image = &record.appearance;
    let mut equipment: [Option<mc_data::ItemStack>; 6] = Default::default();
    for (slot, stack) in equipment.iter_mut().zip(image.equipment.iter()) {
        *slot = stack.clone();
    }
    crate::boundary::EntityAppearance {
        item: image.item.clone(),
        experience_orb: image.experience_orb,
        spawn_data: image.spawn_data,
        equipment,
        body_armor: record.state.loot.body_armor.as_deref().cloned(),
        saddle: record.state.loot.saddle.as_deref().cloned(),
        passengers: Vec::new(),
        leash_holder: None,
        baby: image.baby,
        pose: image.pose,
        shared_flags: image.shared_flags,
        health: record.health,
        custom_name: image.custom_name.clone(),
        custom_name_visible: image.custom_name_visible,
        extra: image
            .extra
            .iter()
            .map(|(index, value)| (*index, restore_data_value(value)))
            .collect(),
        effect_particles: Vec::new(),
        effects_ambient: false,
        arrow_count: 0,
        stinger_count: 0,
        absorption: record.active_effects.absorption,
    }
}

/// Restore one [`EntityDataValue`](crate::boundary::EntityDataValue) from its image.
fn restore_data_value(
    value: &crate::journal::EntityDataValueImage,
) -> crate::boundary::EntityDataValue {
    match value {
        crate::journal::EntityDataValueImage::WeatheringCopper(v) => {
            crate::boundary::EntityDataValue::WeatheringCopper(*v)
        }
        crate::journal::EntityDataValueImage::CopperGolemState(v) => {
            crate::boundary::EntityDataValue::CopperGolemState(*v)
        }
        crate::journal::EntityDataValueImage::VillagerData {
            kind,
            profession,
            level,
        } => crate::boundary::EntityDataValue::VillagerData {
            kind: *kind,
            profession: *profession,
            level: *level,
        },
        crate::journal::EntityDataValueImage::Byte(v) => crate::boundary::EntityDataValue::Byte(*v),
        crate::journal::EntityDataValueImage::VarInt(v) => {
            crate::boundary::EntityDataValue::VarInt(*v)
        }
        crate::journal::EntityDataValueImage::VarLong(v) => {
            crate::boundary::EntityDataValue::VarLong(*v)
        }
        crate::journal::EntityDataValueImage::Float(v) => {
            crate::boundary::EntityDataValue::Float(*v)
        }
        crate::journal::EntityDataValueImage::Rotations(v) => {
            crate::boundary::EntityDataValue::Rotations(*v)
        }
        crate::journal::EntityDataValueImage::Boolean(v) => {
            crate::boundary::EntityDataValue::Boolean(*v)
        }
        crate::journal::EntityDataValueImage::String(v) => {
            crate::boundary::EntityDataValue::String(v.clone())
        }
        crate::journal::EntityDataValueImage::BlockState(v) => {
            crate::boundary::EntityDataValue::BlockState(*v)
        }
        crate::journal::EntityDataValueImage::Item(v) => {
            crate::boundary::EntityDataValue::Item(v.clone())
        }
        crate::journal::EntityDataValueImage::OptionalBlockPos(v) => {
            crate::boundary::EntityDataValue::OptionalBlockPos(*v)
        }
        crate::journal::EntityDataValueImage::EntityEffectParticle(v) => {
            crate::boundary::EntityDataValue::EntityEffectParticle(*v)
        }
        crate::journal::EntityDataValueImage::OptionalUnsignedInt(v) => {
            crate::boundary::EntityDataValue::OptionalUnsignedInt(*v)
        }
        crate::journal::EntityDataValueImage::OptionalLivingEntityReference(v) => {
            crate::boundary::EntityDataValue::OptionalLivingEntityReference(*v)
        }
    }
}

/// Tick loop: 20 TPS, event-driven wake (no sleeps beyond the tick timer).
/// Channels owned by the tick thread.
struct TickChannels {
    checkpoint_every: u64,
    /// Vanilla `pause-when-empty-seconds`; zero never pauses.
    pause_when_empty_seconds: u32,
    input_rx: Receiver<PlayerInput>,
    spawn_rx: Receiver<SpawnRequest>,
    ticket_rx: Receiver<RemoteTicket>,
    /// Reliable FIFO handoff; only the tick owns the sender.
    outbox: std::sync::mpsc::Sender<OutboxBatch>,
    tick_broadcast: broadcast::Sender<TickOutput>,
    timing_broadcast: broadcast::Sender<(u64, TickTimes)>,
    work_tx: std::sync::mpsc::SyncSender<JournalWork>,
    /// Byte bound on queued `Append` payload (see `journal_budget`).
    journal_budget: Arc<JournalBudget>,
    /// A generated-chunk save is queued or running on the journal thread.
    generated_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Edge-visible projection of tick-observed unlocked sessions.
    unlocked_sessions: Arc<parking_lot::RwLock<BTreeSet<ConnectionId>>>,
    shutdown_rx: std::sync::mpsc::Receiver<()>,
    checkpoint_rx: std::sync::mpsc::Receiver<std::sync::mpsc::Sender<bool>>,
    backup_rx: std::sync::mpsc::Receiver<BackupJob>,
    /// Coherent terrain publications, one per dimension index: swapped by
    /// the tick owner under each source's write lock so edge samplers read
    /// a consistent (snapshot, supplement) bundle plus fence without state.
    terrain: [Arc<crate::terrain::TerrainSource>; 3],
    /// Opt-in per-tick input log (`CoreConfig::input_log`).
    input_log: Option<crate::input_log::InputLog>,
}

/// Online players across every dimension; the empty-server pause counts them.
fn online_players(state: &TickState) -> usize {
    std::iter::once(state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .map(|world| world.players.len())
        .sum()
}

fn tick_loop(mut state: TickState, mut channels: TickChannels) {
    let mut next_tick = std::time::Instant::now();
    let checkpoint_every = channels.checkpoint_every.max(1);
    let mut since_checkpoint = 0u64;
    // At most one checkpoint in flight: its acknowledgement (chunks written
    // at which revision, or failure) returns here and is drained each tick.
    let (checkpoint_done_tx, checkpoint_done_rx) = std::sync::mpsc::channel::<CheckpointAck>();
    let mut checkpoint_in_flight = false;
    // Disconnects due for Leave-equivalent removal (sealed a full tick ago,
    // so the edge has drained the terminal `Disconnect`).
    let mut pending_leaves: Vec<ConnectionId> = Vec::new();
    let mut pending_input = None;
    let (plugin_ack_tx, plugin_ack_rx) = std::sync::mpsc::channel();
    let mut plugin_written_tick = state.tick;
    // Empty-server pause bookkeeping (vanilla `pause-when-empty-seconds`).
    let mut empty_ticks = 0u64;
    let mut paused = false;
    'ticks: loop {
        let wall_start = std::time::Instant::now();
        let wall_start_unix_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0);
        while let Ok(written) = checkpoint_done_rx.try_recv() {
            checkpoint_in_flight = false;
            if let Some((written, block_ticks, fluid_ticks, dimensions)) = written {
                state.world.mark_flushed(&written);
                state.block_ticks.acknowledge_flushed(&block_ticks);
                state.fluid_ticks.acknowledge_flushed(&fluid_ticks);
                crate::dimensions::checkpoint::acknowledge(&mut state, dimensions);
            }
        }
        while let Ok(tick) = plugin_ack_rx.try_recv() {
            plugin_written_tick = plugin_written_tick.max(tick);
        }
        crate::plugin::acknowledge_events(&mut state, plugin_written_tick);
        // Event-driven input drain: everything submitted before this tick.
        let mut inputs = Vec::new();
        for connection in std::mem::take(&mut pending_leaves) {
            inputs.push(PlayerInput::Leave { connection });
        }
        if let Some(input) = pending_input.take() {
            inputs.push(input);
        }
        while let Ok(input) = channels.input_rx.try_recv() {
            inputs.push(input);
        }
        let mut activity = !inputs.is_empty();
        // Debug/test spawns drain here (bench + tests only).
        let logging = channels.input_log.is_some();
        let mut logged_spawns = Vec::new();
        while let Ok(request) = channels.spawn_rx.try_recv() {
            if logging {
                logged_spawns.push(crate::input_log::LoggedSpawn::new(&request));
            }
            apply_spawn_request(&mut state, &request);
            activity = true;
        }
        // Remote tickets drain here; the journal records them each tick.
        let mut logged_tickets = Vec::new();
        while let Ok(ticket) = channels.ticket_rx.try_recv() {
            if logging {
                logged_tickets.push(crate::input_log::LoggedTicket {
                    owner: ticket.owner.clone(),
                    x: ticket.chunk.x,
                    z: ticket.chunk.z,
                    radius: ticket.radius,
                });
            }
            apply_ticket(
                &mut state,
                ticket.owner,
                ticket.chunk.x,
                ticket.chunk.z,
                ticket.radius,
            );
            activity = true;
        }
        // Vanilla `pause-when-empty-seconds`: after the grace period with no
        // online player the world stops advancing. Inputs (a join, a console
        // command) wake one tick; the first player resets the counter and
        // resumes continuous ticking. Saves, backups and shutdown stay
        // serviced while paused, exactly as the vanilla server does.
        let pause_limit = u64::from(channels.pause_when_empty_seconds) * 20;
        let pause_tick = state.tick;
        if pause_limit == 0 {
            paused = false;
        } else if online_players(&state) == 0 {
            empty_ticks = empty_ticks.saturating_add(1);
            if empty_ticks >= pause_limit && !paused {
                paused = true;
                tracing::info!(
                    seconds = channels.pause_when_empty_seconds,
                    "server empty, pausing world"
                );
                // Vanilla autoSave()-s once on entering the pause.
                if !checkpoint_in_flight {
                    checkpoint_in_flight = start_checkpoint(
                        &mut state,
                        &channels.work_tx,
                        &checkpoint_done_tx,
                        pause_tick,
                    );
                    if checkpoint_in_flight {
                        since_checkpoint = 0;
                    }
                }
            }
        } else {
            empty_ticks = 0;
            paused = false;
        }
        if paused && !activity {
            if !checkpoint_in_flight && let Ok(reply) = channels.checkpoint_rx.try_recv() {
                checkpoint_in_flight = start_checkpoint_with_reply(
                    &mut state,
                    &channels.work_tx,
                    &checkpoint_done_tx,
                    pause_tick,
                    Some(reply),
                );
                if checkpoint_in_flight {
                    since_checkpoint = 0;
                }
            }
            while let Ok(job) = channels.backup_rx.try_recv() {
                if let Err(std::sync::mpsc::SendError(JournalWork::Backup(job))) =
                    channels.work_tx.send(JournalWork::Backup(job))
                {
                    let _ = job
                        .reply
                        .send(Err(CoreError::Storage("journal thread stopped".into())));
                }
            }
            if channels.shutdown_rx.try_recv().is_ok() {
                start_checkpoint(
                    &mut state,
                    &channels.work_tx,
                    &checkpoint_done_tx,
                    pause_tick,
                );
                send_shutdown(&mut state, &channels.work_tx);
                break;
            }
            next_tick += TICK_PERIOD;
            let now = std::time::Instant::now();
            if next_tick <= now {
                next_tick = now + TICK_PERIOD;
            }
            if channels.shutdown_rx.recv_timeout(next_tick - now).is_ok() {
                start_checkpoint(
                    &mut state,
                    &channels.work_tx,
                    &checkpoint_done_tx,
                    pause_tick,
                );
                send_shutdown(&mut state, &channels.work_tx);
                break;
            }
            continue;
        }
        let tick_start = std::time::Instant::now();
        // Secrets: connections the auth gate holds locked, before and after
        // this tick (a join locks during it; a plugin unlock lifts it).
        let mut locked = Vec::new();
        if logging && state.require_plugin_login {
            locked = locked_connections(&state, &inputs);
        }
        let (mut output, drains, times) = run_tick(&mut state, &inputs);
        if logging && state.require_plugin_login {
            locked.extend(locked_connections(&state, &inputs));
        }
        let run_time = tick_start.elapsed();
        // Journal: this tick's entries (blocks + players + entities). `send`
        // blocks while the bounded queue is full (backpressure) and fails
        // only when the journal thread is gone (receiver disconnected):
        // HALT — stop committing and publishing. Disconnect everyone so
        // the edge drops the world instead of playing on undurable state,
        // refuse new inputs, publish nothing further.
        let entries = journal_entries_for_tick(&mut state, &output);
        let journal_time = tick_start.elapsed();
        let journal_queue_start = std::time::Instant::now();
        let tick = output.tick;
        // The input log gets exactly what this tick consumed; the drained
        // vectors move to the writer thread (no clone, no I/O on this path).
        if let Some(log) = &mut channels.input_log {
            log.record(tick, inputs, logged_spawns, logged_tickets, locked);
        }
        let out_tick = tick;
        let plugin_effects = match crate::plugin::seal_events(&mut state, tick) {
            Ok(active) => active,
            Err(reason) => {
                halt_authority(&mut state, &channels, reason);
                break;
            }
        };
        let journal_dead = send_append(
            &channels.work_tx,
            &channels.journal_budget,
            tick,
            entries,
            plugin_effects.then(|| plugin_ack_tx.clone()),
        )
        .is_err();
        if journal_dead {
            halt_authority(
                &mut state,
                &channels,
                "journal failed: world halted (fail closed)",
            );
            break;
        }
        let journal_queue_time = journal_queue_start.elapsed();
        let generated_start = std::time::Instant::now();
        // Generated chunks to their empty region slots, a batch at a time
        // (each batch rewrites the regions it touches). A dead journal shows
        // on the next Append.
        if !channels
            .generated_busy
            .load(std::sync::atomic::Ordering::Acquire)
            && let Some(batch) =
                crate::dimensions::checkpoint::take_generated(&mut state, GENERATED_SAVE_BATCH)
        {
            channels
                .generated_busy
                .store(true, std::sync::atomic::Ordering::Release);
            let _ = channels.work_tx.send(JournalWork::SaveGenerated(batch));
        }
        // Timings ride in the output they describe (never a shared
        // last-tick slot a reader could pair with the wrong tick).
        output.times = TickTimes {
            wall_start_unix_ns,
            owner_pre_us: tick_start.duration_since(wall_start).as_micros() as u64,
            journal_build_us: (journal_time - run_time).as_micros() as u64,
            journal_queue_us: journal_queue_time.as_micros() as u64,
            generated_us: generated_start.elapsed().as_micros() as u64,
            total_us: tick_start.elapsed().as_micros().min(u64::MAX as u128) as u64,
            ..times
        };
        let owner_post_start = std::time::Instant::now();
        // Hand off before waking edge subscribers. The receiver coalesces
        // batches in FIFO order and never extends a sealed Disconnect queue.
        if !drains.is_empty() {
            channels
                .outbox
                .send(drains)
                .expect("reliable outbox receiver dropped");
        }
        if state.require_plugin_login {
            let mut mirror = channels.unlocked_sessions.write();
            let worlds = || {
                std::iter::once(&state).chain(state.other_dimensions.values().map(|world| &**world))
            };
            mirror.retain(|connection| {
                worlds().any(|world| {
                    world.connections.entity_of(*connection).is_some()
                        && !world.session_locked(*connection)
                })
            });
            for world in worlds() {
                mirror.extend(
                    world
                        .connections
                        .all_connections()
                        .filter(|connection| !world.session_locked(*connection)),
                );
            }
        }

        // Broadcast: laggards skip to latest (world state is skippable).
        // Terrain freshness: publish the coherent per-dimension bundle
        // (resident snapshot + committed-but-unflushed supplement, fenced)
        // so off-tick sampling never mixes a new snapshot with an old
        // supplement (or the reverse) under an unchanged fence.
        // Block edits, chunk loads and unloads all surface in the change
        // set; anything else is not terrain. Changed publications advance
        // each per-dimension fence; idle ticks leave every fence (and every
        // bundle) untouched so the edge before/after sampling check does
        // not reject slow generations and the shared cache does not
        // invalidate continuously.
        let terrain_changed = crate::dimensions::Dimension::ALL
            .into_iter()
            .any(|dimension| {
                output.in_dimension(dimension).is_some_and(|output| {
                    !output.changes.blocks.is_empty()
                        || !output.changes.chunks_loaded.is_empty()
                        || !output.changes.chunks_unloaded.is_empty()
                })
            });
        if terrain_changed {
            // Mark every fence busy BEFORE swapping any bundle: a sampler
            // that grabbed a pre-fence retries instead of mixing a swapped
            // dimension bundle with a not-yet-swapped one.
            for source in &channels.terrain {
                source.begin_publish();
            }
        }
        // Committed-but-unflushed supplement per dimension: resident dirty
        // chunks plus non-resident overlay writes. Covers edit -> evict ->
        // sample before checkpoint, when neither the snapshot nor the
        // native store carries the edit. Shared-empty when clean (no
        // per-tick allocation); rebuilt every publication so flushed
        // entries drop as soon as the checkpoint ACK clears them.
        // Per-dimension bundle swap, indexed explicitly
        // (0 = overworld, 1 = nether, 2 = end): never rely on map order.
        for (index, dimension) in crate::dimensions::Dimension::ALL.into_iter().enumerate() {
            let world = if dimension == state.dimension {
                &state.world
            } else {
                let Some(world) = state.other_dimensions.get(&dimension) else {
                    // Dimension absent this tick (never on the live path:
                    // all three exist): on a changed publication the fence
                    // is already busy and the bundle swaps below with no
                    // snapshot; on idle there is nothing to publish.
                    if terrain_changed {
                        let source = &channels.terrain[index];
                        source.finish_publish(
                            None,
                            crate::terrain::TerrainSource::empty_supplement(),
                            output.tick,
                        );
                    }
                    continue;
                };
                &world.world
            };
            let source = &channels.terrain[index];
            if !terrain_changed {
                // Idle: leave the fence AND the bundle untouched (no
                // re-swap of identical Arcs) so long generations under a
                // stable fence are never spuriously rejected.
                continue;
            }
            let revision = output.tick;
            let snapshot = output
                .in_dimension(dimension)
                .map(|output| Arc::clone(&output.snapshot));
            let dirty = world.dirty_supplement_data();
            let supplement = if dirty.chunks.is_empty() && dirty.overlay.is_empty() {
                crate::terrain::TerrainSource::empty_supplement()
            } else {
                Arc::new(dirty)
            };
            source.finish_publish(snapshot, supplement, revision);
        }
        let mut tick_times = output.times;
        let _ = channels.tick_broadcast.send(output);
        // Disconnects sealed before this tick: the edge has had a full tick
        // to drain the `Disconnect`, so remove the player as a Leave would
        // (via synthetic input next tick, so despawns flow through `changes`).
        let due: Vec<ConnectionId> = state
            .disconnects
            .iter()
            .filter(|(_, sealed)| *sealed < tick)
            .map(|(connection, _)| *connection)
            .collect();
        state.disconnects.retain(|(_, sealed)| *sealed >= tick);
        pending_leaves.extend(due);
        // The request channel is already FIFO. Leave requests there while the
        // prior capture is in flight, then admit one before the cadence check.
        if !checkpoint_in_flight && let Ok(reply) = channels.checkpoint_rx.try_recv() {
            checkpoint_in_flight = start_checkpoint_with_reply(
                &mut state,
                &channels.work_tx,
                &checkpoint_done_tx,
                out_tick,
                Some(reply),
            );
            if checkpoint_in_flight {
                since_checkpoint = 0;
            }
        }
        // Hot backup requests ride the same FIFO as checkpoints, so the journal
        // thread sees them behind every append of this and earlier ticks.
        while let Ok(job) = channels.backup_rx.try_recv() {
            if let Err(std::sync::mpsc::SendError(JournalWork::Backup(job))) =
                channels.work_tx.send(JournalWork::Backup(job))
            {
                let _ = job
                    .reply
                    .send(Err(CoreError::Storage("journal thread stopped".into())));
            }
        }
        // Checkpoint: every `checkpoint_every` ticks, one in flight at a
        // time (a cadence that comes due while one is writing starts as soon
        // as it is acknowledged). See `start_checkpoint`.
        since_checkpoint += 1;
        if since_checkpoint >= checkpoint_every && !checkpoint_in_flight {
            since_checkpoint = 0;
            checkpoint_in_flight =
                start_checkpoint(&mut state, &channels.work_tx, &checkpoint_done_tx, out_tick);
        }
        // Shutdown check (non-blocking): final checkpoint first so the
        // reopened world sees this tick's domains, then Shutdown (FIFO after
        // it on the journal thread) and exit.
        if channels.shutdown_rx.try_recv().is_ok() {
            start_checkpoint(&mut state, &channels.work_tx, &checkpoint_done_tx, out_tick);
            send_shutdown(&mut state, &channels.work_tx);
            tick_times.owner_post_us = owner_post_start.elapsed().as_micros() as u64;
            tick_times.wall_us = wall_start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
            let _ = channels.timing_broadcast.send((tick, tick_times));
            break;
        }
        tick_times.owner_post_us = owner_post_start.elapsed().as_micros() as u64;
        tick_times.wall_us = wall_start.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        state.tick_control.record_tick(tick_times.wall_us);
        if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
            let pre_time = tick_start.duration_since(wall_start);
            eprintln!(
                "tick outer ms: tick={} pre={:.2} run={:.2} journal={:.2} seal_send_generated={:.2} post={:.2}",
                tick,
                pre_time.as_secs_f64() * 1000.0,
                run_time.as_secs_f64() * 1000.0,
                (journal_time - run_time).as_secs_f64() * 1000.0,
                (tick_times.total_us as f64 / 1000.0) - journal_time.as_secs_f64() * 1000.0,
                tick_times
                    .wall_us
                    .saturating_sub(tick_times.total_us)
                    .saturating_sub(pre_time.as_micros() as u64) as f64
                    / 1000.0,
            );
        }
        let _ = channels.timing_broadcast.send((tick, tick_times));
        // Keep the hook-free shutdown/deadline wait unchanged. A hook-enabled
        // tick also services read-only previews when explicitly unparked: that
        // spends no extra game tick and never renews the admission deadline.
        next_tick += state.tick_control.period();
        let now = std::time::Instant::now();
        // A sprint runs ticks back to back; it still takes the shutdown poll
        // of the behind-schedule branch below.
        if state.tick_control.sprinting() {
            next_tick = now;
        }
        if next_tick > now {
            if state.build_queries.is_none() {
                if channels.shutdown_rx.recv_timeout(next_tick - now).is_ok() {
                    start_checkpoint(&mut state, &channels.work_tx, &checkpoint_done_tx, out_tick);
                    send_shutdown(&mut state, &channels.work_tx);
                    break;
                }
            } else {
                loop {
                    if channels.shutdown_rx.try_recv().is_ok() {
                        start_checkpoint(
                            &mut state,
                            &channels.work_tx,
                            &checkpoint_done_tx,
                            out_tick,
                        );
                        send_shutdown(&mut state, &channels.work_tx);
                        break 'ticks;
                    }
                    // Retire FIFO metadata fences without simulating another
                    // tick. Stop at the first real input: no preview behind it
                    // can become ready until that input has actually applied.
                    for _ in 0..mc_script::precommit::MAX_PRECOMMIT_REQUESTS {
                        if pending_input.is_some() {
                            break;
                        }
                        match channels.input_rx.try_recv() {
                            Ok(PlayerInput::BuildPreviewFence { fence }) => fence.apply(),
                            Ok(input) => pending_input = Some(input),
                            Err(_) => break,
                        }
                    }
                    crate::build_precommit::answer_previews(&mut state);
                    let remaining = next_tick.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    std::thread::park_timeout(remaining);
                }
            }
        } else {
            // Behind: skip catch-up waits, never burst. Still poll
            // shutdown once so a queued stop never stalls behind load.
            next_tick = now;
            if channels.shutdown_rx.try_recv().is_ok() {
                start_checkpoint(&mut state, &channels.work_tx, &checkpoint_done_tx, out_tick);
                send_shutdown(&mut state, &channels.work_tx);
                break;
            }
        }
    }
    if let Some(log) = channels.input_log.take() {
        log.finish(state.tick);
    }
}

/// Generated chunks queued per region-writer job.
const GENERATED_SAVE_BATCH: usize = 64;

/// Hand the remaining generated chunks to the journal thread, then stop it.
fn send_shutdown(state: &mut TickState, work_tx: &std::sync::mpsc::SyncSender<JournalWork>) {
    tracing::error!(
        tick = state.tick,
        cause = "shutdown requested",
        "core tick owner stopping"
    );
    while let Some(batch) = crate::dimensions::checkpoint::take_generated(state, 0) {
        if work_tx.send(JournalWork::SaveGenerated(batch)).is_err() {
            tracing::error!(
                cause = "journal work channel closed",
                "core shutdown could not save generated chunks"
            );
            return;
        }
    }
    if work_tx.send(JournalWork::Shutdown).is_err() {
        tracing::error!(
            cause = "journal work channel closed",
            "core shutdown could not flush journal"
        );
    }
}

/// Halt on an authoritative journal or callback-delivery failure: send
/// `Disconnect{reason}` to every live connection through the reliable
/// outbox, then exit WITHOUT publishing, checkpointing, or accepting
/// further inputs. The broadcast sender drops with us, so subscribers see
/// `Closed` (no further TickOutput, ever). No retry worker.
fn halt_authority(state: &mut TickState, channels: &TickChannels, reason: &str) {
    tracing::error!(
        tick = state.tick,
        cause = reason,
        "core tick owner halted (fail closed)"
    );
    let live: Vec<(ConnectionId, crate::boundary::PlayerChange)> = std::iter::once(&*state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .flat_map(|world| world.connections.all_connections())
        .map(|connection| {
            (
                connection,
                crate::boundary::PlayerChange::Disconnect {
                    reason: reason.to_owned(),
                },
            )
        })
        .collect();
    for (connection, change) in live {
        let world = if state.connections.entity_of(connection).is_some() {
            &mut *state
        } else {
            state
                .other_dimensions
                .values_mut()
                .find(|world| world.connections.entity_of(connection).is_some())
                .map(|world| &mut **world)
                .expect("collected native live connection")
        };
        world.push_player(connection, change);
    }
    let drains = std::mem::take(&mut state.outbox)
        .into_iter()
        .chain(
            state
                .other_dimensions
                .values_mut()
                .flat_map(|world| std::mem::take(&mut world.outbox)),
        )
        .collect();
    channels
        .outbox
        .send(drains)
        .expect("reliable outbox receiver dropped");
}
/// Memory-only container benchmark owner; never starts the paced tick thread.
#[doc(hidden)]
pub struct ContainerBench {
    state: TickState,
}

struct ContainerBenchGenerator {
    air: mc_world::BlockStateId,
}

impl ChunkGenerator for ContainerBenchGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            self.air,
            mc_data::Identifier::parse("minecraft:plains").expect("plains identifier"),
        )
    }
}

impl ContainerBench {
    pub const CHESTS: usize = 20_000;
    pub const COMPARATORS: usize = 4;
    const COLUMNS: usize = 200;
    const Y: i32 = 200;

    pub fn new(vanilla_data_dir: &std::path::Path) -> Result<Self, CoreError> {
        let registry = Arc::new(CoreRegistry::load(Some(vanilla_data_dir))?);
        let air = mc_world::BlockStateId(registry.states.air);
        let generator = Arc::new(ContainerBenchGenerator { air });
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            air,
            [0, Self::Y, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        let mut state = TickState::new(registry, world, 2, 512);
        // Exact rectangle, including the comparators at x=-1. Zero-radius
        // tickets retain every scene chunk without players or new worldgen.
        for x in -1..=24 {
            for z in 0..=12 {
                state.tickets.insert(
                    format!("containers-{x}-{z}"),
                    (crate::boundary::ChunkPos { x, z }, 0),
                );
            }
        }
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        if let Some(error) = state.world.load_errors().next() {
            return Err(CoreError::Storage(error.to_string()));
        }
        let chest = Self::block(&state, "chest", &[]);
        let comparator = Self::block(
            &state,
            "comparator",
            &[
                ("facing", "east"),
                ("mode", "compare"),
                ("powered", "false"),
            ],
        );
        let stone = Self::block(&state, "stone", &[]);
        let item = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:stone").expect("stone identifier"))
            .expect("stone item");
        for index in 0..Self::CHESTS {
            let pos = Self::chest_pos(index);
            state.world.set_block(pos.x, pos.y, pos.z, chest);
            state.block_entities.insert(
                (pos.x, pos.y, pos.z),
                crate::tick::BlockEntityState {
                    slots: vec![mc_data::ItemStack::new(item, 64); 27],
                    furnace: None,
                },
            );
        }
        // Each comparator reads the first chest of a different row; no wire
        // network, automation or shared double chest hides the signal cost.
        for index in 0..Self::COMPARATORS {
            let pos = Self::comparator_pos(index);
            state.world.set_block(pos.x, pos.y - 1, pos.z, stone);
            state.world.set_block(pos.x, pos.y, pos.z, comparator);
        }
        state.world.fold_overlay();
        Ok(Self { state })
    }

    fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
        let id = mc_data::Identifier::parse(format!("minecraft:{name}"))
            .expect("benchmark block identifier");
        let base = state
            .registry
            .blocks
            .block(&id)
            .expect("benchmark block")
            .default;
        let mut values = state
            .registry
            .blocks
            .by_id(base)
            .expect("benchmark block state")
            .properties
            .clone();
        for (key, value) in props {
            values
                .iter_mut()
                .find(|(k, _)| k == key)
                .expect("block property")
                .1 = (*value).into();
        }
        state
            .registry
            .blocks
            .by_name_and_props(&id, &values)
            .expect("block properties")
            .0
    }

    fn chest_pos(index: usize) -> crate::boundary::BlockPos {
        crate::boundary::BlockPos {
            x: (index % Self::COLUMNS) as i32 * 2,
            y: Self::Y,
            z: (index / Self::COLUMNS) as i32 * 2,
        }
    }

    fn comparator_pos(index: usize) -> crate::boundary::BlockPos {
        let mut pos = Self::chest_pos(index * Self::COLUMNS);
        pos.x -= 1;
        pos
    }

    /// Four inventory writes per tick, outside the timed region. All slots
    /// remain nonempty; alternating full/quarter-full changes analog strength.
    pub fn mutate(&mut self, tick: usize) {
        let count = if tick.is_multiple_of(2) { 16 } else { 64 };
        for index in 0..Self::COMPARATORS {
            let pos = Self::chest_pos(index * Self::COLUMNS);
            let record = self
                .state
                .block_entities
                .get_mut(&(pos.x, pos.y, pos.z))
                .expect("benchmark chest remains live");
            for stack in &mut record.slots {
                stack.count = count;
            }
        }
    }

    /// Measures the real dimension-coordinated run_tick, including publication
    /// and all ordinary phases, but not pacing, journal I/O or output disposal.
    pub fn tick(&mut self) -> TickTimes {
        let start = std::time::Instant::now();
        let (_output, _drains, mut times) = run_tick(&mut self.state, &[]);
        times.total_us = start.elapsed().as_micros() as u64;
        times
    }

    pub fn loaded_chunks(&self) -> usize {
        self.state.world.chunks().count()
    }

    /// Untimed scene fence: loaded storage must stay nonempty and settled
    /// comparator NBT must match the four mutated inventories.
    pub fn assert_scene(&self) -> u8 {
        assert_eq!(self.state.block_entities.len(), Self::CHESTS);
        for index in 0..Self::CHESTS {
            let pos = Self::chest_pos(index);
            let record = self
                .state
                .block_entities
                .get(&(pos.x, pos.y, pos.z))
                .expect("all benchmark chests remain live");
            assert_eq!(record.slots.len(), 27);
            assert!(record.slots.iter().all(|stack| !stack.is_empty()));
            let block = self
                .state
                .world
                .block_at(pos.x, pos.y, pos.z)
                .and_then(|id| self.state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                .expect("all benchmark chests remain loaded");
            assert_eq!(block.block.id.path(), "chest");
        }
        let mut level = 0;
        for index in 0..Self::COMPARATORS {
            let chest = Self::chest_pos(index * Self::COLUMNS);
            let count = self.state.block_entities[&(chest.x, chest.y, chest.z)].slots[0].count;
            level = if count == 64 { 15 } else { 4 };
            let pos = Self::comparator_pos(index);
            let chunk = self
                .state
                .world
                .chunk(crate::boundary::ChunkPos {
                    x: pos.x.div_euclid(16),
                    z: pos.z.div_euclid(16),
                })
                .expect("comparator remains loaded");
            let bytes = chunk
                .block_entities
                .get(&mc_world::BlockPos {
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                })
                .expect("comparator has committed analog output");
            let mc_nbt::Tag::Compound(fields) =
                mc_nbt::read_network(&mut bytes.as_slice()).expect("comparator NBT")
            else {
                panic!("comparator NBT must be a compound");
            };
            let output = fields.iter().find_map(|(name, tag)| {
                if name == "OutputSignal"
                    && let mc_nbt::Tag::Int(value) = tag
                {
                    Some(*value)
                } else {
                    None
                }
            });
            assert_eq!(
                output,
                Some(i32::from(level)),
                "comparator {index} analog output"
            );
        }
        level
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

/// Convert region/checkpoint NBT Motion to core units; journal images stay in
/// blocks/second. Vanilla Entity.java saves/loads delta movement unchanged.
pub(crate) fn velocity_from_vanilla_motion(motion: [f64; 3]) -> [f64; 3] {
    motion.map(|component| component * 20.0)
}

/// Restore sheep's canonical vanilla fields into its synchronized wool byte.
pub(crate) fn appearance_from_vanilla_entity(
    record: &mc_data::entity_nbt::VanillaEntity,
) -> crate::journal::EntityAppearanceImage {
    let mut image = crate::journal::EntityAppearanceImage {
        health: record.health,
        experience_orb: record.experience_orb,
        ..Default::default()
    };
    if let Some(wool) = record.sheep_wool {
        image.extra.push((
            crate::blueprint::SHEEP_WOOL_DATA_INDEX,
            crate::journal::EntityDataValueImage::Byte(wool as i8),
        ));
    }
    for (key, tag) in &record.extra_fields {
        match (key.as_str(), tag) {
            ("CustomName", mc_nbt::Tag::String(name)) => image.custom_name = Some(name.clone()),
            ("CustomNameVisible", mc_nbt::Tag::Byte(value)) => {
                image.custom_name_visible = *value != 0;
            }
            _ => {}
        }
    }
    image
}

/// Collect one checkpoint's full domain data from live tick state: player
/// records (same fold-back as the journal post-images), entity base (full
/// records WITH motion), and block-entity storage.
pub(crate) fn checkpoint_data_for(
    state: &TickState,
    tick: u64,
) -> (
    Vec<CheckpointPlayerData>,
    std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    crate::core::CheckpointBlockEntities,
) {
    let mut players: Vec<CheckpointPlayerData> = state
        .players
        .keys()
        .filter_map(|id| {
            let slot = state.entities.slot_of_any(*id)?;
            let uuid = state.entities.row(slot).uuid.to_string();
            let record = player_record_for(state, *id, tick)?;
            Some(CheckpointPlayerData { uuid, record })
        })
        .collect();
    // Offline players (disconnected since their last journal post-image):
    // include their pending images so the manifest never advances past
    // their only durable record before pruning.
    for (uuid, record) in &state.offline_players {
        let mut record = record.clone();
        record.tick = tick;
        players.push(CheckpointPlayerData {
            uuid: uuid.to_string(),
            record,
        });
    }
    let entities = checkpoint_entities(state, tick);
    let block_entities = state
        .block_entities
        .iter()
        .map(|(pos, live)| {
            (
                *pos,
                crate::journal::ReplayBlockEntity {
                    slots: live.slots.clone(),
                    furnace: live.furnace_image(),
                    tick,
                },
            )
        })
        .collect();
    (players, entities, block_entities)
}

/// Read-only inputs of one entity's checkpoint record. `TickState` owns channel
/// endpoints and is not `Sync`, so capture workers borrow only these (like the
/// row-preparation `PrepView`).
struct EntityCaptureView<'a> {
    entities: &'a crate::entities::EntityStore,
    registry: &'a crate::registry::CoreRegistry,
    tick: u64,
}

/// Fields whose owners need the whole `TickState`. Gathered serially (only
/// raiders, the ominous item spawner and End fight bodies have any) and spliced
/// at the positions the serial capture put them.
#[derive(Default)]
struct EntityCaptureExtras {
    /// Raid membership, then the ominous item spawner's pending item.
    before_allay: Vec<(String, mc_nbt::Tag)>,
    /// End fight dragon/crystal state.
    end_fight: Vec<(String, mc_nbt::Tag)>,
}

/// Entity base as vanilla records grouped by chunk. The Solaris wire id rides
/// as `solaris:WireId` so journal tail entries (keyed by wire id) still match
/// after recovery. Records are built in parallel (about 10 us each: 110-137 ms
/// serial at 11.5k entities, on the tick thread) and grouped serially in slot
/// order, so the output is identical to a serial slot-order capture.
fn checkpoint_entities(
    state: &TickState,
    tick: u64,
) -> std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> {
    use rayon::prelude::*;
    let slots: Vec<crate::entities::EntitySlot> = state.entities.slots().collect();
    let extras = entity_capture_extras(state, &slots);
    let view = EntityCaptureView {
        entities: &state.entities,
        registry: &state.registry,
        tick,
    };
    let mut records = Vec::new();
    // Indexed collection keeps slot order without a sort.
    slots
        .par_iter()
        .enumerate()
        .with_min_len(256)
        .map(|(index, &slot)| {
            entity_capture_record(&view, slot, capture_extras_for(&extras, index))
        })
        .collect_into_vec(&mut records);
    group_entity_records(records)
}

/// Serial gather of the state-dependent fields, keyed by slot index.
fn entity_capture_extras(
    state: &TickState,
    slots: &[crate::entities::EntitySlot],
) -> Vec<(usize, EntityCaptureExtras)> {
    use crate::entities::EntityKind;
    let mut gathered = Vec::new();
    for (index, &slot) in slots.iter().enumerate() {
        let type_id = match state.entities.body(slot).kind {
            EntityKind::Mob { type_id }
            | EntityKind::Item { type_id }
            | EntityKind::ExperienceOrb { type_id } => type_id,
            EntityKind::Player { .. } => continue,
        };
        let mut before_allay = crate::raids::entity_fields(state, slot);
        if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
            .is_some_and(|contract| contract.name == "minecraft:ominous_item_spawner")
        {
            before_allay.extend(crate::trial_spawner::entity_fields(
                state,
                state.entities.row(slot).uuid,
            ));
        }
        let end_fight = crate::end_fight::entity_fields(state, slot);
        if !before_allay.is_empty() || !end_fight.is_empty() {
            gathered.push((
                index,
                EntityCaptureExtras {
                    before_allay,
                    end_fight,
                },
            ));
        }
    }
    gathered
}

fn capture_extras_for(
    extras: &[(usize, EntityCaptureExtras)],
    index: usize,
) -> Option<&EntityCaptureExtras> {
    extras
        .binary_search_by_key(&index, |(slot_index, _)| *slot_index)
        .ok()
        .map(|position| &extras[position].1)
}

/// Group records by chunk in the order given.
fn group_entity_records(
    records: impl IntoIterator<Item = Option<((i32, i32), mc_data::entity_nbt::VanillaEntity)>>,
) -> std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> {
    let mut entities: std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk> =
        std::collections::BTreeMap::new();
    for ((cx, cz), record) in records.into_iter().flatten() {
        entities
            .entry((cx, cz))
            .or_insert_with(|| mc_world::anvil::EntityChunk {
                x: cx,
                z: cz,
                entities: Vec::new(),
            })
            .entities
            .push(record);
    }
    entities
}

/// One non-player entity's vanilla checkpoint record and its chunk, or `None`
/// for players and entity types without a vanilla contract.
fn entity_capture_record(
    view: &EntityCaptureView<'_>,
    slot: crate::entities::EntitySlot,
    extras: Option<&EntityCaptureExtras>,
) -> Option<((i32, i32), mc_data::entity_nbt::VanillaEntity)> {
    use crate::entities::EntityKind;
    let body = view.entities.body(slot);
    let row = view.entities.row(slot);
    let type_id = match body.kind {
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
        EntityKind::Player { .. } => return None,
    };
    let name = view.registry.entity_type_name(type_id)?.to_owned();
    let tick = view.tick;
    let rot = view.entities.rot(slot);
    let pos = view.entities.pos(slot);
    let vel = view.entities.vel(slot);
    let sheep_wool = (name == "minecraft:sheep").then(|| crate::tick::wool_of(&row.appearance));
    let ageable = view.registry.blueprint(type_id).is_some_and(|blueprint| {
        blueprint.capabilities.ageable
            || blueprint
                .goals
                .iter()
                .any(|goal| goal.kind == crate::blueprint::GoalKind::Breed)
    });
    let mut extra_fields = row
        .villager
        .as_ref()
        .map(|villager| {
            crate::villager::persistence::villager_to_fields(&view.registry.items, villager, tick)
                .expect("live villager state satisfies persistence invariants")
        })
        .unwrap_or_default();
    extra_fields.extend(
        crate::journal::EntityStateImage::from_row(body, row).to_fields(&view.registry.blocks),
    );
    if type_id & crate::blueprint::CUSTOM_TYPE_FLAG != 0 {
        let presentation = view
            .registry
            .entity_type_name(crate::blueprint::wire_type_id(type_id))?;
        extra_fields.push((
            "solaris:Presentation".into(),
            mc_nbt::Tag::String(presentation.into()),
        ));
    }
    if let Some(extras) = extras {
        extra_fields.extend(extras.before_allay.iter().cloned());
    }
    if let Some(allay) = &row.allay {
        extra_fields.extend(crate::allay::to_fields(allay, &view.registry.items));
    }
    if let Some(vehicle) = &row.entity_life.vehicle {
        extra_fields.extend(crate::vehicles::to_fields(vehicle, &view.registry.items));
    }
    extra_fields.extend(
        crate::entity_loot::equipment_to_fields(
            &view.registry.items,
            &row.appearance.equipment,
            &row.loot,
        )
        .expect("live entity equipment satisfies persistence invariants"),
    );
    if let Some(name) = &row.appearance.custom_name {
        extra_fields.push(("CustomName".into(), mc_nbt::Tag::String(name.clone())));
    }
    extra_fields.push((
        "CustomNameVisible".into(),
        mc_nbt::Tag::Byte(i8::from(row.appearance.custom_name_visible)),
    ));
    if let Some(extras) = extras {
        extra_fields.extend(extras.end_fight.iter().cloned());
    }
    if !row.opaque_entity_fields.is_empty() {
        let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice())
            .expect("validated retained entity fields");
        if let mc_nbt::Tag::Compound(fields) = opaque {
            for (key, value) in fields {
                if !(row.projectile.as_ref().is_some_and(|projectile| {
                    crate::projectiles::persistence::owns_field(projectile.kind, &key)
                }) || row.villager.is_some() && crate::villager::persistence::owns_field(&key)
                    || extra_fields.iter().any(|(known, _)| known == &key))
                {
                    extra_fields.push((key, value));
                }
            }
        }
    }
    let tadpole = name == "minecraft:tadpole";
    let mut record = mc_data::entity_nbt::VanillaEntity {
        id: name,
        uuid: row.uuid,
        pos,
        // Core velocities are blocks/second; vanilla NBT Motion is blocks/tick.
        motion: vel.map(|component| component / 20.0),
        yaw: rot[0],
        pitch: rot[1],
        on_ground: view.entities.ground(slot),
        health: row.health,
        age: if tadpole {
            row.entity_life.age_lock.tadpole_age.min(i32::MAX as u32) as i32
        } else if ageable {
            if row.entity_life.child_ticks != 0 {
                -(row.entity_life.child_ticks.min(i32::MAX as u32) as i32)
            } else {
                row.breed_cooldown.min(i32::MAX as u32) as i32
            }
        } else {
            row.age.min(i32::MAX as u32) as i32
        },
        breed_cooldown: row.breed_cooldown as i32,
        pickup_delay: body.pickup_delay as i32,
        item: row
            .item
            .as_deref()
            .map(crate::entities::StackRow::full_stack),
        experience_orb: row.appearance.experience_orb,
        sheep_wool,
        active_effects: crate::journal::active_effects_to_nbt(&row.active_effects),
        absorption_amount: row.active_effects.absorption,
        wire_id: entity_id_int(body.id),
        extra_fields,
    };
    crate::item_frame::capture(&mut record, &row.appearance);
    if let Some(projectile) = &row.projectile {
        record
            .extra_fields
            .retain(|(key, _)| !crate::projectiles::persistence::owns_field(projectile.kind, key));
        record.age = projectile.age.min(i32::MAX as u32) as i32;
        for (key, value) in
            crate::projectiles::persistence::to_fields(projectile, &view.registry.items)
        {
            if key == "Item" {
                record.item = Some(projectile.stack.clone());
            } else {
                record.extra_fields.push((key, value));
            }
        }
    }
    // Floor division: truncation files negative-coordinate entities
    // under the wrong chunk/region (vanilla keys by floor).
    let (cx, cz) = (
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    );
    Some(((cx, cz), record))
}

/// Full player record for journal/checkpoint from live state: fold the open
/// menu back (as if closed) so carried/grid rejoin the inventory.
/// Full player record for journal/checkpoint from live state: real
/// custody, never folded. Inventory 41 + cursor + open crafting grid ride as
/// separate stacks; nothing is merged or dropped during serialization.
pub(crate) fn player_record_for(
    state: &TickState,
    id: EntityId,
    tick: u64,
) -> Option<crate::journal::ReplayPlayer> {
    player_record(state, id, tick, true)
}

fn player_record(
    state: &TickState,
    id: EntityId,
    tick: u64,
    include_statistics: bool,
) -> Option<crate::journal::ReplayPlayer> {
    if state.entity_session_locked(id) {
        return None;
    }
    let slot = state.entities.slot_of_any(id)?;
    let player = state.players.get(&id)?;
    let rot = state.entities.rot(slot);
    // Window 0 lives in `inventory_menu`; container windows in `open`.
    let menu = player.open.as_ref().unwrap_or(&player.inventory_menu);
    let (carried, grid) = (menu.carried.clone(), menu.grid.clone());
    Some(crate::journal::ReplayPlayer {
        opaque_player_fields: player.opaque_player_fields.clone(),
        statistics: if include_statistics {
            player.statistics.at_tick(tick)
        } else {
            Default::default()
        },
        advancement_progress: player.advancement_progress.clone(),
        unlocked_recipes: player.unlocked_recipes.clone(),
        dimension: state.dimension,
        pos: state.entities.pos(slot),
        yaw: rot[0],
        pitch: rot[1],
        health: player.vitals.health,
        food: player.vitals.food,
        saturation: player.vitals.saturation,
        exhaustion: player.vitals.exhaustion,
        air: player.vitals.air,
        experience_level: player.vitals.experience.level,
        experience_progress: player.vitals.experience.progress,
        experience_total: player.vitals.experience.total,
        game_mode: player.game_mode as u8,
        selected: player.inventory.selected,
        inventory: player.inventory.slots.clone(),
        carried,
        grid,
        respawn: player.respawn_point,
        respawn_dimension: player.respawn_dimension,
        respawn_forced: player.respawn_forced,
        respawn_yaw: player.respawn_yaw,
        respawn_pitch: player.respawn_pitch,
        active_effects: player.active_effects.clone(),
        current_impulse_impact_pos: player.current_impulse_impact_pos,
        fall_flying: player.fall_flying,
        enchantment_seed: player.enchantment_seed,
        enchantment_offer_seed: player.enchantment_offer_seed,
        warden_warning: player.warden_warning,
        burn_ticks: state.entities.body(slot).burn_ticks,
        portal: state.entities.row(slot).entity_life.portal,
        tick,
    })
}

/// One checkpoint frozen on the tick thread for the journal thread.
struct CheckpointJob {
    tick: u64,
    globals: std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    dimensions: std::collections::BTreeMap<
        crate::dimensions::Dimension,
        crate::dimensions::checkpoint::Capture,
    >,
    regions: crate::world::ChunkCapture,
    players: Vec<CheckpointPlayerData>,
    entities: std::collections::BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    items: std::sync::Arc<mc_data::items::ItemRegistry>,
    world_spawn: [i32; 3],
    world_spawn_rotation: [f32; 2],
    scoreboard: crate::scoreboard::Scoreboard,
    saved_maps: std::collections::BTreeMap<i32, Arc<crate::maps::SavedMap>>,
    bossbars: crate::bossbar::BossBars,
    raids: crate::raids::Raids,
    wandering_trader: crate::mob_lanes::TraderSpawner,
    scheduled_functions: Vec<(u64, String)>,
    generated_entity_claims: std::collections::BTreeSet<String>,
    tickets: std::collections::BTreeMap<String, ([i32; 2], (u64, i32))>,
    block_ticks: crate::block_ticks::BlockTicksImage,
    fluid_ticks: crate::fluids::FluidTicksImage,
    inhabited_time: Vec<crate::journal::InhabitedTimeImage>,
    debt: Vec<crate::journal::SimDebtImage>,
    climate: crate::weather::ClimateImage,
    border: crate::command_border::Border,
    difficulty: crate::survival::Difficulty,
    gamerules: std::collections::BTreeMap<String, String>,
    /// Per-record settlement rows (`settlement/<id>`) plus the shared
    /// `settlement_sequence` row. No aggregate registry blob.
    settlements: std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    /// Block-log deltas since the previous capture and retention floors.
    block_log: crate::block_log::Persist,
    /// Chunks written at which revision, or `None` when the checkpoint
    /// failed (no manifest; the journal retains everything).
    done: std::sync::mpsc::Sender<CheckpointAck>,
    reply: Option<std::sync::mpsc::Sender<bool>>,
}

/// Freeze checkpoint `tick` (dirty chunks by `Arc`, player/entity/block-
/// entity records by value) and hand it to the journal thread, which
/// writes it in write-ahead order (see [`write_checkpoint`]). Returns
/// whether a job was sent; queue admission blocks only while the bounded
/// journal queue is full, and never waits for the disk write itself.
fn start_checkpoint(
    state: &mut TickState,
    work_tx: &std::sync::mpsc::SyncSender<JournalWork>,
    done: &std::sync::mpsc::Sender<CheckpointAck>,
    tick: u64,
) -> bool {
    start_checkpoint_with_reply(state, work_tx, done, tick, None)
}

fn start_checkpoint_with_reply(
    state: &mut TickState,
    work_tx: &std::sync::mpsc::SyncSender<JournalWork>,
    done: &std::sync::mpsc::Sender<CheckpointAck>,
    tick: u64,
    reply: Option<std::sync::mpsc::Sender<bool>>,
) -> bool {
    let capture_start = std::time::Instant::now();
    let (players, entities, block_entities) = checkpoint_data_for(state, tick);
    let data_time = capture_start.elapsed();
    crate::campfire::checkpoint(state);
    crate::bees::checkpoint(state);
    crate::jukebox::checkpoint(state);
    crate::brewing::checkpoint(state);
    crate::sculk::checkpoint(state);
    crate::crafter::checkpoint(state);
    crate::shelf::checkpoint(state);
    crate::shulker::checkpoint(state);
    crate::redstone::containers::checkpoint(state);
    crate::end_portals::checkpoint(state);
    let owners_time = capture_start.elapsed();
    let absent_block_ticks = state.block_ticks.checkpoint(&mut state.world, tick);
    let absent_fluid_ticks = state.fluid_ticks.checkpoint(&mut state.world, tick);
    let scheduler_time = capture_start.elapsed();
    let mut regions = state.world.capture_dirty(&block_entities);
    let chunks_time = capture_start.elapsed();
    state
        .world
        .capture_scheduled_block_ticks(&mut regions, absent_block_ticks);
    state
        .world
        .capture_scheduled_fluid_ticks(&mut regions, absent_fluid_ticks);
    let absent_time = capture_start.elapsed();
    let dimensions = crate::dimensions::checkpoint::capture(state, tick);
    let dimensions_time = capture_start.elapsed();
    if regions.load_error().is_some()
        || dimensions
            .values()
            .any(|capture| capture.regions.load_error().is_some())
    {
        if let Some(reply) = reply {
            let _ = reply.send(false);
        }
        return false;
    }
    let job = CheckpointJob {
        tick,
        globals: state
            .native_globals
            .iter()
            .filter(|(name, _)| name.as_str() != "plugins")
            .map(|(name, record)| (name.clone(), record.clone()))
            .collect(),
        dimensions,
        regions,
        players,
        entities,
        items: std::sync::Arc::clone(&state.registry.items),
        world_spawn: state.world.world_spawn(),
        world_spawn_rotation: state.world.world_spawn_rotation(),
        scoreboard: state.scoreboard.clone(),
        saved_maps: state.maps.records.clone(),
        bossbars: state.bossbars.clone(),
        raids: state.raids.clone(),
        wandering_trader: state.trader_spawner,
        scheduled_functions: state.datapacks.image(),
        generated_entity_claims: state.generated_entity_claims.clone(),
        tickets: checkpoint_tickets(state, tick),
        block_ticks: state.block_ticks.image(),
        fluid_ticks: state.fluid_ticks.image(),
        inhabited_time: state.world.inhabited_checkpoint(tick),
        debt: state.world.debt_checkpoint(tick),
        climate: crate::weather::ClimateImage::capture(state),
        border: state.border,
        difficulty: state.difficulty,
        gamerules: state
            .gamerules
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        settlements: match crate::settlements::persistence::checkpoint_rows(state, tick) {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(tick, %error, "settlement checkpoint skipped; journal retained");
                if let Some(reply) = reply {
                    let _ = reply.send(false);
                }
                return false;
            }
        },
        block_log: crate::block_log::checkpoint(state),
        done: done.clone(),
        reply,
    };
    if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
        eprintln!(
            "checkpoint capture ms: tick={} data={:.2} owners={:.2} scheduler={:.2} chunks={:.2} absent={:.2} dimensions={:.2} job={:.2}",
            tick,
            data_time.as_secs_f64() * 1000.0,
            (owners_time - data_time).as_secs_f64() * 1000.0,
            (scheduler_time - owners_time).as_secs_f64() * 1000.0,
            (chunks_time - scheduler_time).as_secs_f64() * 1000.0,
            (absent_time - chunks_time).as_secs_f64() * 1000.0,
            (dimensions_time - absent_time).as_secs_f64() * 1000.0,
            (capture_start.elapsed() - dimensions_time).as_secs_f64() * 1000.0,
        );
    }
    work_tx.send(JournalWork::Checkpoint(Box::new(job))).is_ok()
}

/// Settlement rows the WAL deleted since the previous checkpoint: present in
/// the durable globals image but absent from this capture. Removing them here
/// keeps the tombstone and the row deletion in one atomic transaction.
fn list_stale_settlement_rows(
    globals: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    captured: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Vec<String> {
    globals
        .keys()
        .filter(|key| {
            key.starts_with("settlement/")
                && !key.starts_with("settlement_sequence")
                && !captured.contains_key(key.as_str())
        })
        .cloned()
        .collect()
}

/// Freeze durable ticket post-images on the same tick thread as all world domains.
pub(crate) fn checkpoint_tickets(
    state: &TickState,
    tick: u64,
) -> std::collections::BTreeMap<String, ([i32; 2], (u64, i32))> {
    state
        .tickets
        .iter()
        .filter(|(owner, _)| !owner.starts_with("solaris:portal:"))
        .map(|(owner, (centre, radius))| (owner.clone(), ([centre.x, centre.z], (tick, *radius))))
        .collect()
}

fn player_journal_entry(
    uuid: String,
    record: &crate::journal::ReplayPlayer,
    previous: Option<&crate::journal::ReplayPlayer>,
    tick: u64,
) -> JournalEntry {
    JournalEntry::Player {
        uuid,
        opaque_player_fields: record.opaque_player_fields.clone(),
        statistics: Box::new(record.statistics.clone()),
        advancement_progress: previous
            .is_none_or(|old| !Arc::ptr_eq(&old.advancement_progress, &record.advancement_progress))
            .then(|| Arc::clone(&record.advancement_progress)),
        unlocked_recipes: previous
            .is_none_or(|old| !Arc::ptr_eq(&old.unlocked_recipes, &record.unlocked_recipes))
            .then(|| Arc::clone(&record.unlocked_recipes)),
        dimension: record.dimension,
        pos: record.pos,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        food: record.food,
        saturation: record.saturation,
        exhaustion: record.exhaustion,
        air: record.air,
        experience_level: record.experience_level,
        experience_progress: record.experience_progress,
        experience_total: record.experience_total,
        game_mode: record.game_mode,
        selected: record.selected,
        inventory: record.inventory.clone(),
        carried: record.carried.clone(),
        grid: record.grid.clone(),
        respawn: record.respawn,
        respawn_dimension: record.respawn_dimension,
        respawn_forced: record.respawn_forced,
        respawn_yaw: record.respawn_yaw,
        respawn_pitch: record.respawn_pitch,
        active_effects: record.active_effects.clone(),
        current_impulse_impact_pos: record.current_impulse_impact_pos,
        fall_flying: record.fall_flying,
        enchantment_seed: record.enchantment_seed,
        enchantment_offer_seed: record.enchantment_offer_seed,
        warden_warning: record.warden_warning,
        burn_ticks: record.burn_ticks,
        portal: record.portal,
        tick,
    }
}

/// Build journal entries for one committed tick (absolute post-images).
pub(crate) fn journal_entries_for_tick(
    state: &mut TickState,
    output: &TickOutput,
) -> Vec<JournalEntry> {
    // Classify item births/crossings before entity_images advances the
    // existing WAL baselines (restored items already have a baseline).
    crate::abstract_sim::cancel_external_writes(state);
    let mut entries = Vec::new();
    crate::plugin::journal(state, &mut entries);
    // Settlement images always ride an explicit Dimension envelope, every
    // dimension including Overworld. Child ticks extend the raw images and the
    // existing child wrap below supplies their envelope.
    let mut settlement_entries = Vec::new();
    crate::settlements::persistence::journal_entries(state, output.tick, &mut settlement_entries);
    if state.dimension == crate::dimensions::Dimension::Overworld {
        if !settlement_entries.is_empty() {
            entries.push(JournalEntry::Dimension {
                dimension: state.dimension,
                entries: settlement_entries,
            });
        }
    } else {
        entries.extend(settlement_entries);
    }
    state.block_ticks.journal_entries(&mut entries);
    state.fluid_ticks.journal(&mut entries);
    state.world.journal_inhabited_time(&mut entries);
    crate::end_fight::journal(state, &mut entries);
    crate::raids::journal(state, &mut entries);
    crate::mob_lanes::journal(state, &mut entries);
    state.datapacks.journal(&mut entries);
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.gamerules_dirty)
    {
        entries.push(JournalEntry::GameRules {
            values: state
                .gamerules
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        });
    }
    let climate = crate::weather::ClimateImage::capture(state);
    if state.dimension == crate::dimensions::Dimension::Overworld
        && state.journaled_climate != Some(climate)
    {
        entries.push(JournalEntry::Climate { value: climate });
        state.journaled_climate = Some(climate);
    }
    if state.journaled_border != Some(state.border) {
        entries.push(JournalEntry::Border {
            value: state.border,
        });
        state.journaled_border = Some(state.border);
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && state.journaled_difficulty != Some(state.difficulty)
    {
        entries.push(JournalEntry::Difficulty {
            value: state.difficulty,
        });
        state.journaled_difficulty = Some(state.difficulty);
    }
    for claim in std::mem::take(&mut state.new_generated_entity_claims) {
        entries.push(JournalEntry::GeneratedEntityClaim { claim });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.scoreboard_dirty)
    {
        entries.push(JournalEntry::Scoreboard {
            value: state.scoreboard.clone(),
        });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld
        && std::mem::take(&mut state.bossbars_dirty)
    {
        entries.push(JournalEntry::Bossbars {
            value: state.bossbars.to_bytes(),
        });
    }
    if state.dimension == crate::dimensions::Dimension::Overworld {
        for id in std::mem::take(&mut state.maps.dirty) {
            entries.push(JournalEntry::SavedMap {
                id,
                value: Arc::clone(&state.maps.records[&id]),
            });
        }
    }
    // Removals precede final storage images: a replacement at the same cell
    // must clear its old family without deleting the newly installed entity.
    let mut removed = std::mem::take(&mut state.block_entity_removed);
    removed.extend(
        state
            .world
            .take_block_entity_removals()
            .into_iter()
            .map(|pos| (pos.x, pos.y, pos.z)),
    );
    removed.sort_unstable();
    removed.dedup();
    for (x, y, z) in removed {
        state.journaled_block_entities.remove(&(x, y, z));
        entries.push(JournalEntry::RemoveBlockEntity {
            pos: [x, y, z],
            tick: output.tick,
        });
    }
    // Block-entity post-images (absolute) for containers whose custody was
    // touched since the last batch and whose contents differ from their last
    // journaled image (equality stays the arbiter).
    for cell in state.block_entities.take_journal_touched() {
        let Some(live) = state.block_entities.get(&cell) else {
            state.journaled_block_entities.remove(&cell);
            continue;
        };
        let image = (live.slots.clone(), live.furnace_image());
        if state.journaled_block_entities.get(&cell) == Some(&image) {
            continue;
        }
        entries.push(JournalEntry::BlockEntity {
            pos: [cell.0, cell.1, cell.2],
            slots: image.0.clone(),
            furnace: image.1.clone(),
            tick: output.tick,
        });
        state.journaled_block_entities.insert(cell, image);
    }
    for change in &output.changes.blocks {
        entries.push(JournalEntry::Block {
            pos: block_pos_array(change.position),
            state: change.state,
        });
    }
    let mut opaque = std::mem::take(&mut state.opaque_changes);
    // Keep the latest owner image for a cell. Timer-only owners deliberately
    // leave the chunk's wire/checkpoint image stale until capture.
    opaque.reverse();
    opaque.sort_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    opaque.dedup_by_key(|(pos, _)| (pos.x, pos.y, pos.z));
    for (pos, staged) in opaque {
        let live = state
            .world
            .chunk(crate::boundary::ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .and_then(|chunk| chunk.block_entities.get(&pos));
        // Presence fences removed storage, not timer values: replacing a live
        // owner image with older cached NBT would roll retained timers back.
        let nbt = match (staged, live) {
            (Some(bytes), Some(_)) => Some(bytes),
            (_, live) => live.cloned(),
        };
        entries.push(JournalEntry::OpaqueBlockEntity {
            pos: block_pos_array(pos),
            nbt,
        });
    }
    crate::block_log::journal(state, &mut entries);
    // Player post-images whose durable fields (inventory, selected, vitals
    // incl. xp, game mode, position) differ from the last journaled one.
    // The open menu's grid/cursor fold back via close semantics at journal
    // time: journal the state as if closed.
    state
        .journaled_players
        .retain(|id, _| state.players.contains_key(id));
    let ids: Vec<EntityId> = state.players.keys().copied().collect();
    for id in ids {
        let Some(mut record) = player_record(state, id, 0, false) else {
            continue;
        };
        let statistics = &state.players[&id].statistics;
        if let Some(previous) = state.journaled_players.get_mut(&id)
            && record.eq_except_statistics(previous)
        {
            let changes = statistics.changes_since(&previous.statistics);
            let timers = (statistics.timers() != previous.statistics.timers())
                .then_some(statistics.timers());
            if !changes.is_empty() || timers.is_some() {
                previous.statistics.apply_changes(&changes);
                if let Some(timers) = timers {
                    previous.statistics.apply_timers(timers);
                }
                entries.push(JournalEntry::Statistics {
                    uuid: state
                        .entities
                        .slot_of_any(id)
                        .map(|slot| state.entities.row(slot).uuid.to_string())
                        .unwrap_or_default(),
                    changes,
                    timers,
                    tick: output.tick,
                });
            }
            continue;
        }
        record.statistics = statistics.clone();
        let previous = state.journaled_players.get(&id);
        let uuid = state
            .entities
            .slot_of_any(id)
            .map(|slot| state.entities.row(slot).uuid.to_string())
            .unwrap_or_default();
        entries.push(player_journal_entry(uuid, &record, previous, output.tick));
        state.journaled_players.insert(id, record);
    }
    // Leave freezes timers and closes menu custody before removing the live
    // player. Its final postimage must enter this WAL batch, not wait for a
    // checkpoint while the previous connected anchor keeps accruing time.
    for uuid in std::mem::take(&mut state.left_players) {
        if let Some(record) = state.offline_players.get(&uuid)
            && record.tick == output.tick
        {
            entries.push(player_journal_entry(
                uuid.to_string(),
                record,
                None,
                output.tick,
            ));
        }
    }
    crate::journal_touched::entity_images(state, output, &mut entries);
    for id in &output.changes.entities_removed {
        // Wire removals include intermediate dimension departures. A same-tick
        // return has live custody here again and must keep its final image.
        if state.entities.slot_of(*id).is_some() {
            continue;
        }
        entries.push(JournalEntry::RemoveEntity {
            id: entity_id_int(*id),
            tick: output.tick,
        });
    }
    crate::chunk_tickets::journal(state, output.tick, &mut entries);
    for (dimension, world) in &mut state.other_dimensions {
        let child_output = output
            .snapshot
            .dimensions
            .get(dimension)
            .expect("every resident dimension publishes in the same tick");
        entries.push(JournalEntry::Dimension {
            dimension: *dimension,
            entries: journal_entries_for_tick(world, child_output),
        });
    }
    state.world.journal_debt(&mut entries);
    entries
}

/// Work for the native writer thread, the single redb writer. FIFO, so saves
/// of generated chunks stay ordered with checkpoints.
enum NativeWork {
    /// Commit a checkpoint whose WAL coverage the journal thread already gated
    /// (`Err`: the gate failed, so nothing is written and the job reports failure).
    /// The plugin mirror is the journal thread's FIFO image frozen at `job.tick`.
    Checkpoint {
        job: Box<CheckpointJob>,
        plugins: Result<Box<crate::plugin::PluginState>, CoreError>,
    },
    SaveGenerated(crate::world::ChunkCapture),
    /// Copy the last committed checkpoint and the WAL after it (hot backup).
    Backup {
        job: BackupJob,
        through: u64,
    },
}

/// Journal thread: owns WAL appends and their fsyncs, and gates each
/// checkpoint's WAL coverage. Checkpoint jobs arrive FIFO behind the appends of
/// every tick they cover, so the gate only has to confirm coverage (see
/// [`write_checkpoint`]). The multi-second redb commit runs on a second thread
/// ([`native_writer_loop`]) so a later tick's `Append` is never parked behind
/// it. The function returns only after that thread has drained, so a shutdown
/// still completes the final checkpoint.
fn journal_loop(
    world_dir: &std::path::Path,
    work_rx: std::sync::mpsc::Receiver<JournalWork>,
    mut writer: JournalWriter,
    generated_busy: &std::sync::atomic::AtomicBool,
    budget: &JournalBudget,
    initial_manifest: CheckpointManifest,
    mut plugins: crate::plugin::PluginState,
) {
    // Tick of the last committed checkpoint, published by the native writer
    // before it acknowledges. In normal running a gate only follows the previous
    // checkpoint's ACK (`checkpoint_in_flight`), so it reads that tick. A
    // shutdown may start its final checkpoint with one still in flight; the gate
    // then reads an older tick. The writer's verified startup interval and
    // contiguous append coverage keep that gate independent of pruned files.
    let committed_tick = std::sync::atomic::AtomicU64::new(initial_manifest.tick);
    // Set by the native writer after a panic; the journal side then halts exactly
    // like an append failure.
    let native_failed = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Owned by this closure: returning drops the receiver at once (before the
        // scope joins the native writer), so the tick's next `send` fails instead of
        // blocking behind an in-flight commit.
        let work_rx = work_rx;
        // Returning on any path closes the budget, waking a tick thread parked
        // in `reserve` so it halts like a failed `send`.
        let _close = crate::journal_budget::CloseOnDrop(budget);
        let (native_tx, native_rx) = std::sync::mpsc::channel::<NativeWork>();
        std::thread::Builder::new()
            .name("mc-core-native".to_owned())
            .spawn_scoped(scope, || {
                native_writer_loop(
                    world_dir,
                    native_rx,
                    generated_busy,
                    &committed_tick,
                    &native_failed,
                    initial_manifest,
                );
            })
            .expect("spawn native checkpoint writer");
        // Fail-closed journal: once an append fails, the WAL has a hole no
        // later fsync can fill. Return (dropping our receiver) so the tick
        // loop's next `work_tx.send` fails — the existing channel carries the
        // failure to the tick owner, which halts (no more commits, no more
        // publishes, Disconnect to all). No retry worker: the hole is
        // unfillable.
        while let Ok(work) = work_rx.recv() {
            if native_failed.load(std::sync::atomic::Ordering::Acquire) {
                tracing::error!("native checkpoint writer died; world halting (fail closed)");
                return;
            }
            match work {
                JournalWork::Append {
                    tick_lo,
                    tick_hi,
                    entries,
                    plugin_ack,
                    reserved,
                } => {
                    if let Err(error) = writer.append(tick_lo, tick_hi, &entries) {
                        tracing::error!(tick_lo, tick_hi, error = %stop::Chain(&error), "journal append failed; world halting (fail closed)");
                        return;
                    }
                    for entry in &entries {
                        if let JournalEntry::Plugins { value } = entry {
                            plugins.apply_delta(value);
                        }
                    }
                    if let Some(ack) = plugin_ack {
                        let _ = ack.send(tick_hi);
                    }
                    #[cfg(test)]
                    if let Some((root, observer)) = JOURNAL_APPEND_OBSERVER
                        .lock()
                        .expect("journal append observer")
                        .as_ref()
                        && root == world_dir
                    {
                        if let Err(error) = writer.flush() {
                            tracing::error!(tick_hi, error = %stop::Chain(&error), "journal flush failed; world halting (fail closed)");
                            return;
                        }
                        observer(tick_hi, &entries);
                    }
                    budget.release(reserved);
                }
                JournalWork::Checkpoint(job) => {
                    let gate = writer.gate(
                        committed_tick.load(std::sync::atomic::Ordering::Acquire),
                        job.tick,
                    );
                    let plugins = gate.map(|()| Box::new(plugins.clone()));
                    if native_tx
                        .send(NativeWork::Checkpoint { job, plugins })
                        .is_err()
                    {
                        tracing::error!(
                            cause = "native writer channel closed",
                            "journal checkpoint handoff failed; world halting"
                        );
                        return;
                    }
                }
                JournalWork::Backup(job) => match writer.flush() {
                    // Everything appended so far is durable; the native writer
                    // copies it behind every earlier checkpoint commit.
                    Ok(()) => {
                        let through = writer.covered_through().unwrap_or(0);
                        if native_tx.send(NativeWork::Backup { job, through }).is_err() {
                            tracing::error!(
                                cause = "native writer channel closed",
                                "journal backup handoff failed; world halting"
                            );
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = job.reply.send(Err(error));
                    }
                },
                JournalWork::SaveGenerated(chunks) => {
                    if native_tx.send(NativeWork::SaveGenerated(chunks)).is_err() {
                        tracing::error!(
                            cause = "native writer channel closed",
                            "journal generated-chunk handoff failed; world halting"
                        );
                        return;
                    }
                }
                JournalWork::Shutdown => {
                    if let Err(error) = writer.flush() {
                        tracing::error!(error = %stop::Chain(&error), "journal final flush failed");
                    }
                    tracing::error!(cause = "shutdown requested", "core journal owner stopped");
                    return;
                }
            }
        }
        tracing::error!(
            cause = "tick work channel closed",
            "core journal owner stopped"
        );
    });
}

/// Native writer thread: the single redb writer for checkpoints and generated
/// saves. In normal running at most one checkpoint and one generated save are
/// outstanding (`checkpoint_in_flight`, `generated_busy`); shutdown may add a
/// final checkpoint and more generated saves, which this FIFO drains before the
/// journal scope exits. Only a successful Immediate commit permits pruning
/// batches wholly covered by that newly durable checkpoint.
///
/// A panic in one job fails closed: the job reports failure on `done`/`reply`,
/// `native_failed` makes the journal side halt, and this thread stops.
fn native_writer_loop(
    world_dir: &std::path::Path,
    work_rx: std::sync::mpsc::Receiver<NativeWork>,
    generated_busy: &std::sync::atomic::AtomicBool,
    committed_tick: &std::sync::atomic::AtomicU64,
    native_failed: &std::sync::atomic::AtomicBool,
    initial_manifest: CheckpointManifest,
) {
    // The current complete manifest is the recovery/pruning authority.
    let mut prev_manifest = initial_manifest;
    let mut block_log = crate::block_log::Backlog::default();
    while let Ok(work) = work_rx.recv() {
        let notify = match &work {
            NativeWork::Checkpoint { job, .. } => Some((job.done.clone(), job.reply.clone())),
            NativeWork::SaveGenerated(_) => None,
            NativeWork::Backup { .. } => None,
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_native_work(
                work,
                world_dir,
                generated_busy,
                committed_tick,
                &mut prev_manifest,
                &mut block_log,
            );
        }));
        if let Err(payload) = outcome {
            tracing::error!(
                panic = stop::panic_message(&*payload),
                "native checkpoint writer panicked; world halting (fail closed)"
            );
            if let Some((done, reply)) = notify {
                let _ = done.send(None);
                if let Some(reply) = reply {
                    let _ = reply.send(false);
                }
            }
            native_failed.store(true, std::sync::atomic::Ordering::Release);
            return;
        }
    }
    tracing::error!(
        cause = "journal work channel closed",
        "native checkpoint writer stopped"
    );
}

fn run_native_work(
    work: NativeWork,
    world_dir: &std::path::Path,
    generated_busy: &std::sync::atomic::AtomicBool,
    committed_tick: &std::sync::atomic::AtomicU64,
    prev_manifest: &mut CheckpointManifest,
    block_log: &mut crate::block_log::Backlog,
) {
    match work {
        NativeWork::Checkpoint { mut job, plugins } => {
            let started = std::time::Instant::now();
            // Block-log rows join the table before the commit that may prune
            // their WAL; a failed write keeps them queued for the next job.
            block_log.absorb(std::mem::take(&mut job.block_log));
            let written = plugins.and_then(|plugins| {
                let store = job
                    .regions
                    .store()
                    .ok_or_else(|| CoreError::Storage("checkpoint has no native store".into()))?;
                block_log.write(store, job.regions.registry(), &job.items)?;
                commit_checkpoint(world_dir, &job, &plugins, prev_manifest)
            });
            let committed = written.is_ok();
            match &written {
                Ok((_, _, io)) => {
                    committed_tick.store(prev_manifest.tick, std::sync::atomic::Ordering::Release);
                    tracing::info!(
                        tick = job.tick,
                        total_ms = started.elapsed().as_millis() as u64,
                        chunk_ms = io.chunk_micros / 1000,
                        entity_ms = io.entity_micros / 1000,
                        db_commit_ms = io.commit_micros / 1000,
                        entities_visited = io.entities_visited,
                        rewritten_entities = io.rewritten_entities,
                        entity_payload_bytes = io.entity_payload_bytes,
                        rewritten_chunks = io.rewritten_chunks,
                        chunk_payload_bytes = io.chunk_payload_bytes,
                        sections_encoded = io.sections_encoded,
                        section_records_written = io.section_records_written,
                        "checkpoint committed"
                    );
                }
                Err(error) => {
                    tracing::warn!(tick = job.tick, %error, "checkpoint skipped; journal retained");
                }
            }
            let CheckpointJob {
                block_ticks,
                fluid_ticks,
                dimensions,
                done,
                reply,
                ..
            } = *job;
            let ack = written.ok().map(|(written, mut child_written, _)| {
                let dimensions = dimensions
                    .into_iter()
                    .map(|(dimension, capture)| {
                        (
                            dimension,
                            crate::dimensions::checkpoint::Ack {
                                written: child_written
                                    .remove(&dimension)
                                    .expect("completed dimension write"),
                                block_ticks: capture.block_ticks,
                                fluid_ticks: capture.fluid_ticks,
                            },
                        )
                    })
                    .collect();
                (written, block_ticks, fluid_ticks, dimensions)
            });
            let _ = done.send(ack);
            if let Some(reply) = reply {
                let _ = reply.send(committed);
            }
            if committed {
                prune_batches_through(&world_dir.join("journal"), prev_manifest.tick);
            }
        }
        NativeWork::SaveGenerated(chunks) => {
            // Best effort: a failed save only means generating again.
            if let Err(error) = chunks.fill_generated() {
                tracing::warn!(%error, "generated chunks not saved");
            }
            generated_busy.store(false, std::sync::atomic::Ordering::Release);
        }
        NativeWork::Backup { job, through } => {
            let started = std::time::Instant::now();
            let result =
                crate::hot_backup::copy_world(world_dir, &job.out, prev_manifest.tick, through);
            match &result {
                Ok(backup) => tracing::info!(
                    checkpoint_tick = backup.checkpoint_tick,
                    wal_tick = backup.wal_tick,
                    files = backup.files.len(),
                    total_ms = started.elapsed().as_millis() as u64,
                    "hot backup copied"
                ),
                Err(error) => tracing::warn!(%error, "hot backup failed"),
            }
            let _ = job.reply.send(result);
        }
    }
}

/// Gate the WAL, commit every dimension/domain in one Immediate native txn,
/// without acknowledging or pruning the captured revisions here.
fn write_checkpoint(
    world_dir: &std::path::Path,
    job: &CheckpointJob,
    plugins: &crate::plugin::PluginState,
    writer: &mut JournalWriter,
    prev_manifest: &mut CheckpointManifest,
) -> Result<CheckpointWrite, CoreError> {
    writer.gate(prev_manifest.tick, job.tick)?;
    commit_checkpoint(world_dir, job, plugins, prev_manifest)
}

/// Native commit of a checkpoint whose WAL coverage is already gated durable.
fn commit_checkpoint(
    _world_dir: &std::path::Path,
    job: &CheckpointJob,
    plugins: &crate::plugin::PluginState,
    prev_manifest: &mut CheckpointManifest,
) -> Result<CheckpointWrite, CoreError> {
    let written = job.regions.written();
    #[cfg(test)]
    {
        let delay = DELAY_CHECKPOINT_COMMIT
            .lock()
            .as_ref()
            .filter(|(dir, _)| *dir == _world_dir)
            .map(|(_, delay)| *delay);
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }
    #[cfg(test)]
    if PANIC_CHECKPOINT_COMMIT
        .lock()
        .take_if(|dir| dir.as_path() == _world_dir)
        .is_some()
    {
        panic!("injected native writer panic");
    }
    #[cfg(test)]
    if FAIL_CHECKPOINT_AFTER_REGIONS
        .lock()
        .ok()
        .and_then(|mut armed| armed.take_if(|dir| *dir == _world_dir))
        .is_some()
    {
        return Err(CoreError::Storage("injected checkpoint failure".to_owned()));
    }
    let dimensions = crate::dimensions::checkpoint::images(&job.dimensions, job.tick);
    let dimension_written = job
        .dimensions
        .iter()
        .map(|(&dimension, capture)| (dimension, capture.regions.written()))
        .collect();
    let mut merged = prev_manifest.players.clone();
    for player in job
        .players
        .iter()
        .chain(job.dimensions.values().flat_map(|world| &world.players))
    {
        merged.insert(player.uuid.clone(), job.tick);
    }
    let manifest = CheckpointManifest {
        tick: job.tick,
        domain_versions: crate::native_checkpoint::domain_versions(),
        dimensions,
        players: merged,
        world_spawn: Some(job.world_spawn),
        world_spawn_rotation: job.world_spawn_rotation,
        generated_entity_claims: job.generated_entity_claims.clone(),
        tickets: job.tickets.clone(),
        block_ticks: job.block_ticks.clone(),
        fluid_ticks: job.fluid_ticks.clone(),
        inhabited_time: job.inhabited_time.clone(),
        debt: job.debt.clone(),
        climate: Some(job.climate),
        border: job.border,
        difficulty: job.difficulty,
        gamerules: job.gamerules.clone(),
        end_fight: None,
        raids: Some(job.raids.clone()),
        wandering_trader: Some(job.wandering_trader),
        scheduled_functions: job.scheduled_functions.clone(),
    };
    crate::native_checkpoint::validate_manifest(&manifest)?;
    let mut globals = job.globals.clone();
    // Per-record settlement rows ride the same atomic native transaction.
    // Tombstones are row deletions: drop checkpoint rows the WAL removed.
    for (key, row) in &job.settlements {
        globals.insert(key.clone(), row.clone());
    }
    for key in list_stale_settlement_rows(&globals, &job.settlements) {
        globals.remove(&key);
    }
    // This runs only on the journal thread (or startup before tick ownership).
    // FIFO processing freezes the mirror at exactly job.tick: later appends
    // cannot be applied until this complete native transaction returns.
    globals.insert(
        "plugins".into(),
        crate::native_checkpoint::record(
            job.tick,
            serde_json::to_vec(plugins).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(
            job.tick,
            serde_json::to_vec(&manifest).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    globals.insert(
        "scoreboard".into(),
        crate::native_checkpoint::tag_record(job.tick, &job.scoreboard.to_nbt())?,
    );
    globals.insert(
        "bossbars".into(),
        crate::native_checkpoint::tag_record(job.tick, &job.bossbars.to_nbt())?,
    );
    crate::maps::checkpoint(&job.saved_maps, job.tick, &mut globals)?;
    crate::native_checkpoint::refresh_metadata(&mut globals, &manifest)?;
    let extras = globals
        .get("conversion_extras")
        .ok_or_else(|| CoreError::Storage("missing native conversion extras".into()))?;
    crate::native_checkpoint::decode_tag(extras, "conversion_extras")?;
    for record in globals.values_mut() {
        record.captured_tick = job.tick;
    }
    let players = crate::native_checkpoint::encode_players(
        job.players
            .iter()
            .chain(job.dimensions.values().flat_map(|capture| &capture.players)),
        &job.items,
        job.tick,
    )?;
    let entities = std::iter::once((crate::dimensions::Dimension::Overworld, &job.entities))
        .chain(
            job.dimensions
                .iter()
                .map(|(&dimension, capture)| (dimension, &capture.entities)),
        )
        .flat_map(|(dimension, chunks)| {
            crate::native_checkpoint::entity_snapshots(dimension, chunks, &job.items, job.tick)
        })
        .map(|entity| entity.map_err(|e| mc_world::native::NativeError::Source(e.to_string())));
    let store = job
        .regions
        .store()
        .ok_or_else(|| CoreError::Storage("checkpoint has no native store".into()))?;
    let io = store
        .commit_checkpoint_stream(
            mc_world::native::NativeCheckpoint {
                tick: job.tick,
                previous_tick: prev_manifest.tick,
                chunks: &[],
                entities: &[],
                players: &players,
                globals: &globals,
            },
            |write| {
                for capture in std::iter::once(&job.regions)
                    .chain(job.dimensions.values().map(|capture| &capture.regions))
                {
                    capture
                        .visit_native(|snapshot| {
                            write(snapshot).map_err(|error| CoreError::Storage(error.to_string()))
                        })
                        .map_err(|error| {
                            mc_world::native::NativeError::Source(error.to_string())
                        })?;
                }
                Ok(())
            },
            entities,
            job.regions.registry(),
            &job.items,
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    #[cfg(test)]
    if FAIL_CHECKPOINT_AFTER_NATIVE_COMMIT
        .lock()
        .ok()
        .and_then(|mut armed| armed.take_if(|dir| *dir == _world_dir))
        .is_some()
    {
        return Err(CoreError::Storage(
            "injected crash after native commit before acknowledgement".into(),
        ));
    }
    *prev_manifest = manifest;
    Ok((written, dimension_written, io))
}

#[cfg(test)]
#[path = "native_checkpoint_crash_tests.rs"]
mod native_checkpoint_crash_tests;

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
mod motion_tests {
    use super::*;

    #[test]
    fn checkpoint_motion_uses_vanilla_tick_units_and_recovers_core_seconds() {
        let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        ));
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 80, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
        let cow_type = registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:cow").expect("cow id"))
            .expect("cow type");
        let cow = state.spawn_mob(cow_type, [0.0, 80.0, 0.0], 20.0);
        let slot = state.entities.slot_of(cow).expect("cow slot");
        state.entities.set_vel(slot, [1.0, 0.0, 0.0]);
        let (_, chunks, _) = checkpoint_data_for(&state, 1);
        let record = &chunks[&(0, 0)].entities[0];
        let fields =
            mc_data::entity_nbt::entity_to_fields(&registry.items, record).expect("encode raw NBT");
        let motion = fields
            .iter()
            .find(|(name, _)| name == "Motion")
            .map(|(_, value)| value)
            .expect("Motion tag");
        assert_eq!(
            motion,
            &mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::DOUBLE,
                elements: vec![
                    mc_nbt::Tag::Double(0.05),
                    mc_nbt::Tag::Double(0.0),
                    mc_nbt::Tag::Double(0.0),
                ],
            })
        );
        let decoded = mc_data::entity_nbt::entity_from_fields(
            &registry.items,
            mc_data::entity_nbt::VanillaEntity::default(),
            &fields,
        )
        .expect("decode raw NBT");
        assert_eq!(
            velocity_from_vanilla_motion(decoded.motion),
            [1.0, 0.0, 0.0]
        );
        assert_eq!(state.entities.vel(slot), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn checkpoint_sheep_preserves_color_and_sheared_until_regrowth() {
        use crate::boundary::{ConnectionId, Hand, PlayerInput};
        use crate::tick::run_tick;

        let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        ));
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 80, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
        run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "shearer".to_owned(),
                permission_level: 0,
            }],
        );
        state.world.ensure_blocking(&state.desired_chunks());
        let player = state
            .connections
            .entity_of(ConnectionId(1))
            .expect("player");
        let player_slot = state.entities.slot_of(player).expect("player slot");
        let pos = state.entities.pos(player_slot);
        let sheep_type = registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:sheep").expect("sheep id"))
            .expect("sheep type");
        let sheep = state.spawn_mob(sheep_type, pos, 8.0);
        let sheep_slot = state.entities.slot_of(sheep).expect("sheep slot");
        crate::tick::set_wool(
            Arc::make_mut(&mut state.entities.row_mut(sheep_slot).appearance),
            0x10 | 15,
        );
        let (_, chunks, _) = checkpoint_data_for(&state, 1);
        let record = chunks
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|record| record.id == "minecraft:sheep")
            .expect("checkpoint sheep");
        let fields = mc_data::entity_nbt::entity_to_fields(&registry.items, record).expect("NBT");
        assert!(fields.contains(&("Color".to_owned(), mc_nbt::Tag::Byte(15))));
        assert!(fields.contains(&("Sheared".to_owned(), mc_nbt::Tag::Byte(1))));
        let dir = std::env::temp_dir().join(format!("solaris-sheep-{}", uuid::Uuid::new_v4()));
        mc_world::anvil::entity_regions::write_entity_regions(&dir, &registry.items, &chunks, 1)
            .expect("write checkpoint entities");
        let back = mc_world::anvil::entity_regions::read_entity_regions(&dir, &registry.items)
            .expect("read checkpoint entities");
        std::fs::remove_dir_all(&dir).expect("remove test entities");
        let record = back
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|record| record.id == "minecraft:sheep")
            .expect("recovered sheep");
        assert_eq!(record.sheep_wool, Some(0x10 | 15));
        let replay = crate::journal::ReplayEntity {
            uuid: record.uuid.to_string(),
            type_id: sheep_type,
            pos: record.pos,
            vel: velocity_from_vanilla_motion(record.motion),
            yaw: record.yaw,
            pitch: record.pitch,
            ground: record.on_ground,
            health: record.health,
            alive: true,
            age: record.age as u32,
            breed_cooldown: record.breed_cooldown as u32,
            pickup_delay: record.pickup_delay as u32,
            item: record.item.clone(),
            appearance: appearance_from_vanilla_entity(record),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 1,
        };
        let recovered = EntityId(1000);
        restore_entity(&mut state, recovered.0, &replay).unwrap();
        let slot = state.entities.slot_of(recovered).expect("restored sheep");
        assert_eq!(
            crate::tick::wool_of(&state.entities.row(slot).appearance),
            0x10 | 15
        );
        let shears = registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:shears").expect("shears id"))
            .expect("shears");
        state
            .players
            .get_mut(&player)
            .expect("player")
            .inventory
            .slots[0] = mc_data::ItemStack::new(shears, 1);
        let shear = PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: recovered,
            hand: Hand::Main,
            location: [0.0; 3],
        };
        let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&shear));
        assert!(
            output.changes.entities_spawned.is_empty(),
            "sheared sheep pays nothing after recovery"
        );
        assert_eq!(state.players[&player].inventory.slots[0].damage, None);
        // Vanilla Sheep.ate clears only the sheared bit when wool regrows.
        let slot = state.entities.slot_of(recovered).expect("restored sheep");
        crate::tick::set_wool(
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
            15,
        );
        let (output, _, _) = run_tick(&mut state, &[shear]);
        let black_wool = registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:black_wool").expect("wool id"))
            .expect("wool");
        let wool: i32 = output
            .changes
            .entities_spawned
            .iter()
            .filter_map(|id| state.entities.slot_of(*id))
            .filter_map(|slot| state.entities.row(slot).item.as_ref())
            .filter(|stack| stack.item_id == black_wool)
            .map(|stack| stack.count)
            .sum();
        assert!((1..=3).contains(&wool), "regrown sheep pays colored wool");
        assert_eq!(state.players[&player].inventory.slots[0].damage, Some(1));
    }

    #[test]
    fn checkpoint_orb_restores_value_count_age_and_motion() {
        let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        ));
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 80, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
        let mut spawned = Vec::new();
        crate::experience_orbs::award(&mut state, [0.5, 80.0, 0.5], 7, &mut spawned);
        let id = spawned[0];
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).age = 234;
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).experience_orb = Some((7, 4));
        state.entities.set_vel(slot, [1.0, 2.0, 3.0]);
        let (_, chunks, _) = checkpoint_data_for(&state, 100);
        let native = &chunks[&(0, 0)].entities[0];
        let fields = mc_data::entity_nbt::entity_to_fields(&registry.items, native).unwrap();
        let decoded = mc_data::entity_nbt::entity_from_fields(
            &registry.items,
            mc_data::entity_nbt::VanillaEntity::default(),
            &fields,
        )
        .unwrap();
        assert_eq!(decoded.experience_orb, Some((7, 4)));
        assert_eq!(decoded.age, 234);
        let record = crate::journal::ReplayEntity {
            uuid: decoded.uuid.to_string(),
            type_id: registry
                .entity_types
                .id_of(&mc_data::Identifier::parse(decoded.id.clone()).unwrap())
                .unwrap(),
            pos: decoded.pos,
            vel: velocity_from_vanilla_motion(decoded.motion),
            yaw: decoded.yaw,
            pitch: decoded.pitch,
            ground: decoded.on_ground,
            health: decoded.health,
            alive: true,
            age: decoded.age as u32,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: None,
            appearance: appearance_from_vanilla_entity(&decoded),
            active_effects: Default::default(),
            villager: None,
            projectile: None,
            state: Default::default(),
            tick: 100,
        };
        state.remove_entity(slot);
        restore_entity(&mut state, id.0, &record).unwrap();
        let slot = state.entities.slot_of(id).unwrap();
        assert!(matches!(
            state.entities.body(slot).kind,
            EntityKind::ExperienceOrb { .. }
        ));
        assert_eq!(
            state.entities.row(slot).appearance.experience_orb,
            Some((7, 4))
        );
        assert_eq!(state.entities.row(slot).age, 234);
        assert_eq!(state.entities.vel(slot), [1.0, 2.0, 3.0]);
        assert_eq!(
            state.entities.experience_orbs().collect::<Vec<_>>(),
            vec![id]
        );
    }
}

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
mod ticket_checkpoint_tests {
    use super::*;
    use crate::dimensions::Dimension;
    use crate::journal::read_complete_batches_after;
    use crate::native_checkpoint::tests::read_manifest;

    fn config(root: &std::path::Path) -> CoreConfig {
        let mut config = CoreConfig::new(root.to_owned(), 42);
        config.checkpoint_every_ticks = u64::MAX;
        config.journal_fsync_every_batches = 1;
        config.view_distance = 1;
        config.simulation_distance = 0;
        config
    }

    #[test]
    fn native_manifest_rejects_missing_domain_versions() {
        assert!(serde_json::from_str::<CheckpointManifest>(r#"{"tick":9}"#).is_err());
    }

    #[test]
    fn durable_api_ticket_survives_checkpoint_empty_tail_restart_without_portal_ticket() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        let core = Core::open(config.clone()).unwrap();
        let mut receiver = core.subscribe();
        for (owner, chunk) in [
            ("example:forced", crate::boundary::ChunkPos { x: 3, z: 4 }),
            (
                "solaris:portal:test",
                crate::boundary::ChunkPos { x: 9, z: 10 },
            ),
        ] {
            core.add_ticket(RemoteTicket {
                owner: owner.into(),
                chunk,
                radius: 0,
                reason: "checkpoint regression".into(),
                budget_chunks: 1,
            })
            .unwrap();
        }
        // Admission occurs before broadcast on the tick thread.
        receiver.blocking_recv().unwrap();
        receiver.blocking_recv().unwrap();
        assert!(core.checkpoint());
        core.shutdown();
        let manifest = read_manifest(&config).unwrap();
        assert_eq!(manifest.tickets.len(), 1);
        assert_eq!(manifest.tickets["example:forced"].0, [3, 4]);
        assert_eq!(manifest.tickets["example:forced"].1, (manifest.tick, 0));
        prune_batches_through(&directory.path().join("journal"), manifest.tick);
        assert!(
            read_complete_batches_after(&directory.path().join("journal"), 0)
                .unwrap()
                .is_empty()
        );
        let restored = Core::open(config.clone()).unwrap();
        let mut receiver = restored.subscribe();
        let mut forced_loaded = false;
        for _ in 0..100 {
            let output = receiver.blocking_recv().unwrap();
            forced_loaded |= output
                .snapshot
                .chunks
                .iter()
                .any(|chunk| chunk.pos == mc_world::ChunkPos { x: 3, z: 4 });
            if forced_loaded {
                break;
            }
        }
        assert!(
            forced_loaded,
            "durable ticket forces loading without any players"
        );
        assert!(restored.checkpoint());
        let recovered = read_manifest(&config).unwrap();
        assert_eq!(recovered.tickets.len(), 1);
        assert_eq!(recovered.tickets["example:forced"].0, [3, 4]);
        assert!(!recovered.tickets.contains_key("solaris:portal:test"));
        restored.shutdown();
    }

    #[test]
    fn ticket_capture_is_frozen_and_child_isolated_with_wal_override() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        let core = Core::open(config.clone()).unwrap();
        core.shutdown();
        let mut manifest = read_manifest(&config).unwrap();
        let load = |dimension, _: &CheckpointManifest| {
            recover_dimension_state(&config, dimension).unwrap()
        };
        let mut state = load(Dimension::Overworld, &manifest);
        for (dimension, centre) in [
            (Dimension::Overworld, [3, 4]),
            (Dimension::Nether, [5, 6]),
            (Dimension::End, [7, 8]),
        ] {
            let world = if dimension == Dimension::Overworld {
                &mut state
            } else {
                state
                    .other_dimensions
                    .insert(dimension, Box::new(load(dimension, &manifest)));
                state.other_dimensions.get_mut(&dimension).unwrap()
            };
            world.tickets.insert(
                "example:shared-owner".into(),
                (
                    crate::boundary::ChunkPos {
                        x: centre[0],
                        z: centre[1],
                    },
                    1,
                ),
            );
            world.tickets.insert(
                "solaris:portal:transient".into(),
                (crate::boundary::ChunkPos { x: 20, z: 20 }, 0),
            );
        }
        let tick = manifest.tick + 1;
        let (work_tx, work_rx) = journal_work_channel();
        let (done_tx, _done_rx) = std::sync::mpsc::channel();
        assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
        // Mutations after capture must not leak into the journal-thread write.
        state.tickets.clear();
        for child in state.other_dimensions.values_mut() {
            child.tickets.clear();
        }
        let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
            panic!("expected frozen checkpoint job");
        };
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(tick, tick, &[]).unwrap();
        write_checkpoint(
            directory.path(),
            &job,
            &state.plugins,
            &mut writer,
            &mut manifest,
        )
        .unwrap();
        prune_batches_through(&directory.path().join("journal"), tick);
        assert!(
            read_complete_batches_after(&directory.path().join("journal"), 0)
                .unwrap()
                .is_empty()
        );
        for (dimension, centre) in [
            (Dimension::Overworld, [3, 4]),
            (Dimension::Nether, [5, 6]),
            (Dimension::End, [7, 8]),
        ] {
            let restored = load(dimension, &manifest);
            assert_eq!(restored.tickets.len(), 1);
            assert_eq!(
                restored.tickets["example:shared-owner"],
                (
                    crate::boundary::ChunkPos {
                        x: centre[0],
                        z: centre[1]
                    },
                    1
                ),
            );
        }
        writer
            .append(
                tick + 1,
                tick + 1,
                &[JournalEntry::Dimension {
                    dimension: Dimension::Nether,
                    entries: vec![JournalEntry::Ticket {
                        owner: "example:shared-owner".into(),
                        chunk: [11, 12],
                        tick: tick + 1,
                        radius: 2,
                    }],
                }],
            )
            .unwrap();
        writer.flush().unwrap();
        assert_eq!(
            load(Dimension::Nether, &manifest).tickets["example:shared-owner"],
            (crate::boundary::ChunkPos { x: 11, z: 12 }, 2),
        );
        assert_eq!(
            load(Dimension::Overworld, &manifest).tickets["example:shared-owner"],
            (crate::boundary::ChunkPos { x: 3, z: 4 }, 1),
        );
        assert_eq!(
            load(Dimension::End, &manifest).tickets["example:shared-owner"],
            (crate::boundary::ChunkPos { x: 7, z: 8 }, 1),
        );
    }
}

#[cfg(test)]
mod difficulty_persistence_tests {
    use super::*;
    use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
    use crate::journal::read_complete_batches_after;
    use crate::native_checkpoint::tests::read_manifest;
    use crate::survival::Difficulty;

    #[test]
    fn difficulty_checkpoint_is_frozen_and_wal_restart_preserves_join_and_query() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::new(directory.path().to_owned(), 42);
        config.checkpoint_every_ticks = u64::MAX;
        config.journal_fsync_every_batches = 1;
        config.view_distance = 1;
        config.simulation_distance = 0;
        let core = Core::open(config.clone()).unwrap();
        core.shutdown();
        let mut manifest = read_manifest(&config).unwrap();
        let load = |_: &CheckpointManifest| {
            recover_dimension_state(&config, crate::dimensions::Dimension::Overworld).unwrap()
        };
        let mut state = load(&manifest);
        for dimension in [
            crate::dimensions::Dimension::Nether,
            crate::dimensions::Dimension::End,
        ] {
            state.other_dimensions.insert(
                dimension,
                Box::new(recover_dimension_state(&config, dimension).unwrap()),
            );
        }
        state.difficulty = Difficulty::Hard;
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = journal_entries_for_tick(&mut state, &output);
        assert!(entries.iter().any(|entry| matches!(
            entry,
            JournalEntry::Difficulty {
                value: Difficulty::Hard
            }
        )));
        let tick = output.tick;
        let (work_tx, work_rx) = journal_work_channel();
        let (done_tx, _done_rx) = std::sync::mpsc::channel();
        assert!(start_checkpoint(&mut state, &work_tx, &done_tx, tick));
        state.difficulty = Difficulty::Easy;
        let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
            panic!("expected frozen checkpoint");
        };
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(tick, tick, &entries).unwrap();
        write_checkpoint(
            directory.path(),
            &job,
            &state.plugins,
            &mut writer,
            &mut manifest,
        )
        .unwrap();
        assert_eq!(manifest.difficulty, Difficulty::Hard);
        prune_batches_through(&directory.path().join("journal"), tick);
        assert!(
            read_complete_batches_after(&directory.path().join("journal"), 0)
                .unwrap()
                .is_empty()
        );
        let mut restored = load(&manifest);
        assert_eq!(restored.difficulty, Difficulty::Hard);
        let connection = ConnectionId(17);
        let (output, drains, _) = crate::tick::run_tick(
            &mut restored,
            &[
                PlayerInput::Join {
                    connection,
                    uuid: uuid::Uuid::new_v4(),
                    name: "DifficultyTester".into(),
                    permission_level: 0,
                },
                PlayerInput::Command {
                    connection,
                    command: "difficulty normal".into(),
                    permission_level: 4,
                },
            ],
        );
        assert!(
            drains
                .iter()
                .any(|(_, changes)| changes.iter().any(|change| matches!(
                    change,
                    PlayerChange::DifficultyChanged { difficulty: 3, .. }
                )))
        );
        assert_eq!(restored.difficulty, Difficulty::Normal);
        let entries = journal_entries_for_tick(&mut restored, &output);
        assert!(entries.iter().any(|entry| matches!(
            entry,
            JournalEntry::Difficulty {
                value: Difficulty::Normal
            }
        )));
        writer.append(output.tick, output.tick, &entries).unwrap();
        writer.flush().unwrap();
        let mut restarted = load(&manifest);
        assert_eq!(restarted.difficulty, Difficulty::Normal);
        let (_, drains, _) = crate::tick::run_tick(
            &mut restarted,
            &[PlayerInput::Join {
                connection,
                uuid: uuid::Uuid::new_v4(),
                name: "QueryTester".into(),
                permission_level: 0,
            }],
        );
        assert!(
            drains
                .iter()
                .any(|(_, changes)| changes.iter().any(|change| matches!(
                    change,
                    PlayerChange::DifficultyChanged { difficulty: 2, .. }
                )))
        );
        let (_, drains, _) = crate::tick::run_tick(
            &mut restarted,
            &[PlayerInput::Command {
                connection,
                command: "difficulty".into(),
                permission_level: 4,
            }],
        );
        assert!(
            drains
                .iter()
                .any(|(_, changes)| changes.iter().any(|change| {
                    matches!(change, PlayerChange::StyledChat { content, .. }
                if format!("{content:?}").contains("options.difficulty.normal"))
                }))
        );
        let error = serde_json::from_str::<CheckpointManifest>(r#"{"tick":9}"#).unwrap_err();
        assert!(error.to_string().contains("domain_versions"), "{error}");
    }
}

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
mod village_generation_tests {
    use super::*;

    #[test]
    #[ignore = "requires the local vanilla 26.1.2 content cache"]
    fn fresh_v2_sites_describe_generated_villagers_from_the_same_plans() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = CoreConfig::new(directory.path().to_owned(), 81);
        config.vanilla_data_dir = Some(
            std::env::var_os("SOLARIS_CONTENT_CACHE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
                }),
        );
        let startup = std::time::Instant::now();
        let registry = Arc::new(CoreRegistry::load(config.vanilla_data_dir.as_deref()).unwrap());
        let store = Arc::new(
            mc_world::native::NativeStore::create(
                &directory.path().join("world.db"),
                crate::native_checkpoint::native_identity(&config),
            )
            .unwrap(),
        );
        let manifest = CheckpointManifest {
            world_spawn: Some([0, 128, 0]),
            ..Default::default()
        };
        let mut state = load_dimension_state(
            &config,
            registry,
            crate::dimensions::Dimension::Overworld,
            &manifest,
            Replay::default(),
            store,
            &std::collections::BTreeMap::new(),
        )
        .unwrap();
        eprintln!("seed81 registry/dimension startup: {:?}", startup.elapsed());
        assert!(state.world.resident_positions().is_empty());
        let sites = state.world.village_sites_in_region((-68, -68), (101, 101));
        assert!(
            !sites.is_empty(),
            "fresh configured v2 world must expose native villages"
        );
        assert!(
            state.world.resident_positions().is_empty(),
            "site listing must not materialize chunks"
        );
        let site = &sites[0];
        assert!(!site.pieces.is_empty());
        let wanted = (site.min.z.div_euclid(16)..=site.max.z.div_euclid(16))
            .flat_map(|z| {
                (site.min.x.div_euclid(16)..=site.max.x.div_euclid(16))
                    .map(move |x| (crate::boundary::ChunkPos { x, z }, 0))
            })
            .collect();
        let loaded = state.world.ensure_blocking(&wanted);
        let markers: Vec<_> = state
            .world
            .resident_positions()
            .iter()
            .flat_map(|pos| state.world.chunk(*pos).unwrap().generated_entities())
            .filter(|marker| {
                site.pieces.iter().any(|piece| {
                    marker.claim.contains(&format!(
                        "@{}:{}:{}#",
                        piece.position.x, piece.position.y, piece.position.z
                    ))
                })
            })
            .collect();
        assert!(
            !markers.is_empty(),
            "listed village {:?} must place authored villagers",
            site.start_chunk
        );
        let expected: std::collections::HashSet<_> = markers
            .iter()
            .map(|marker| crate::villager::generated_uuid(&marker.claim))
            .collect();
        assert_eq!(
            expected.len(),
            markers.len(),
            "each authored villager has a unique claim"
        );
        let mut changes = crate::boundary::ChangeSet::default();
        crate::villager::generated::load(&mut state, &loaded, &mut changes);
        for marker in &markers {
            let uuid = crate::villager::generated_uuid(&marker.claim);
            let slot = state
                .entities
                .slot_of_uuid(uuid)
                .expect("authored villager materialized");
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            assert!(row.villager.is_some());
            assert_eq!(state.entities.pos(slot), marker.position);
            assert!(changes.entities_spawned.contains(&body.id));
        }
        let count = changes.entities_spawned.len();
        crate::villager::generated::load(&mut state, &loaded, &mut changes);
        assert_eq!(
            changes.entities_spawned.len(),
            count,
            "the same site must not duplicate villagers"
        );
        assert_eq!(
            state
                .world
                .village_sites_in_region(site.start_chunk, site.start_chunk),
            vec![site.clone()]
        );
        eprintln!(
            "seed81 listed {} villages; {:?} materialized {} original UUIDs",
            sites.len(),
            site.start_chunk,
            expected.len()
        );
    }
}
