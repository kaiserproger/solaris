//! Tick driver: inputs -> entity phase -> merge -> block phase ->
//! commit -> publish, at 20 TPS (ADR 0013 "Tick").
//!
//! The entity phase runs in parallel by data over scoped threads sized once
//! from the machine (no work stealing across phase boundaries, no locks on
//! the tick path: workers return owned writes). Merge applies intents one at
//! a time in fixed order with whole-intent rejection. The block phase runs
//! serially in vanilla-like update order. Commit folds blocks, applies entity
//! writes, and publishes the snapshot. Publish hands a [`TickOutput`] to broadcast
//! (world state: laggards skip to latest) and per-player
//! [`PlayerChange`](crate::boundary::PlayerChange) results to bounded
//! per-connection queues (never skipped; overflow disconnects).

use mc_physics::mth;
use std::collections::{BTreeMap, BTreeSet};

use mc_data::statistics::StatType;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

use crate::boundary::{
    BlockPos, ChangeSet, ChunkPos, ConnectionId, DigAction, EntityAppearance, EntityEvent,
    EntityId, Hand, PlayerChange, PlayerInput, TickOutput, TickTimes, WorldSnapshot,
};
use crate::entities::{
    Body, ColdFields, EntityKind, EntitySlot, EntityStore, MOTION_ALIVE, MOTION_EFFECTS,
    MOTION_OWNED, MOTION_PHASE_MASK, MOTION_PHASE_SHIFT, MOTION_PLAYER, MOTION_REST,
    MOTION_SPECIAL,
};
use crate::intent::{Footprint, Intent, IntentKind, InvClaim, sort_intents};
use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;

mod auth;
pub(crate) use auth::unlock_session;
mod container_access;
mod damage;
mod death_message;
mod elytra;
mod entity_lifecycle;
mod entity_phase;
mod hydration;
mod residency;
pub(crate) use hydration::sync_chunk_block_entities;
mod input;
pub(crate) mod merchant;
pub(crate) mod mining;
mod passive_follow;
mod stations;
mod system_graph;
use entity_phase::*;
mod players;
pub(crate) use players::*;
mod containers;
mod merge;
pub(crate) use containers::*;
mod build;
pub(crate) use build::*;

#[cfg(test)]
mod furnace_tests;
#[cfg(test)]
pub(crate) mod mining_tests;

#[cfg(test)]
#[path = "auth_gate_tests.rs"]
mod auth_gate_tests;

/// Fixed worker count: derived once from the machine.
#[must_use]
pub fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .max(1)
}

/// Reuse capacity, not a slot snapshot: age expiry can compact the store before
/// despawn, and the two callers have different global-player admission rules.
pub(crate) fn active_entity_slots(
    state: &mut TickState,
    include_global_players: bool,
) -> Vec<EntitySlot> {
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    // Every row has current chunk membership. When all occupied chunks are
    // active, the dense range is already the unique, ordered admission list.
    let all_active = state
        .entities
        .villager_chunks()
        .keys()
        .all(|chunk| state.world.is_active(chunk.x, chunk.z))
        && state
            .entities
            .brain_sensor_chunks()
            .keys()
            .all(|chunk| state.world.is_active(chunk.x, chunk.z));
    if all_active {
        slots.extend(state.entities.slots());
        return slots;
    }
    // Exact membership permits a dense scan when most rows are active, even
    // with inactive occupied chunks. Keep sparse activity and old transition
    // entries on the indexed union below.
    if prefer_dense_entity_admission(state) {
        let mut previous: Option<(ChunkPos, bool)> = None;
        for slot in state.entities.slots() {
            let chunk = state.entities.chunk_of(slot);
            let active = match previous {
                Some((last, active)) if last == chunk => active,
                _ => {
                    let active = state.world.is_active_position(chunk.x, chunk.z);
                    previous = Some((chunk, active));
                    active
                }
            };
            if active
                || include_global_players
                    && state.players.contains_key(&state.entities.body(slot).id)
            {
                slots.push(slot);
            }
        }
        return slots;
    }
    let mut marks = std::mem::take(&mut state.serial_entity_marks);
    marks.resize(state.entities.len(), 0);
    let mut admitted = 0;
    for chunk in state
        .world
        .active_positions()
        .filter(|chunk| state.world.is_active(chunk.x, chunk.z))
    {
        state.entities.for_each_slot_in_chunk(chunk, |slot| {
            let mark = &mut marks[slot.0 as usize];
            admitted += usize::from(*mark == 0);
            *mark = 1;
        });
    }
    if include_global_players {
        for id in state.players.keys() {
            if let Some(slot) = state.entities.slot_of_any(*id) {
                let mark = &mut marks[slot.0 as usize];
                admitted += usize::from(*mark == 0);
                *mark = 1;
            }
        }
    }
    slots.reserve(admitted);
    for (index, &marked) in marks.iter().enumerate() {
        if marked != 0 {
            slots.push(EntitySlot(index as u32));
        }
    }
    marks.clear();
    state.serial_entity_marks = marks;
    slots
}

/// Read hot positions only for mostly active populations with exact membership.
/// Bucket counts reject widely dormant worlds before any per-chunk lookups;
/// population weights also reject dormant rows concentrated in a few chunks.
fn prefer_dense_entity_admission(state: &TickState) -> bool {
    let occupied =
        state.entities.villager_chunks().len() + state.entities.brain_sensor_chunks().len();
    // Each occupied chunk can occur in both indexes. Conservatively retain the
    // indexed path when active chunks cannot cover half the bucket entries.
    if occupied > state.world.active_positions().count().saturating_mul(4)
        || !state.entities.has_exact_chunk_membership()
    {
        return false;
    }
    let needed = state.entities.len().div_ceil(2);
    let mut active = 0;
    for (chunk, ids) in state
        .entities
        .villager_chunks()
        .iter()
        .chain(state.entities.brain_sensor_chunks())
    {
        if state.world.is_active_position(chunk.x, chunk.z) {
            active += ids.len();
            if active >= needed {
                return true;
            }
        }
    }
    active >= needed
}

/// Connections registry: connection -> player entity + identity.
#[derive(Debug, Default)]
pub struct Connections {
    by_connection: HashMap<ConnectionId, EntityId>,
    by_entity: HashMap<EntityId, ConnectionId>,
    names: HashMap<ConnectionId, String>,
    uuids: HashMap<ConnectionId, uuid::Uuid>,
    /// Transient negotiated client view radii, keyed by live connection.
    /// Recorded on [`PlayerInput::SetViewDistance`](crate::boundary::PlayerInput::SetViewDistance)
    /// (which precedes Join), moved with the connection across dimensions,
    /// and cleared on leave/remove. Never persisted: unnegotiated callers
    /// fall back to the configured server view.
    view_distances: HashMap<ConnectionId, u8>,
    /// Transient client UI locales, keyed by live connection.
    /// Recorded on [`PlayerInput::SetLocale`](crate::boundary::PlayerInput::SetLocale)
    /// (which precedes Join), moved with the connection across dimensions,
    /// and cleared on leave/remove. Never persisted: unnegotiated callers read
    /// English.
    locales: HashMap<ConnectionId, String>,
}

impl Connections {
    pub(crate) fn insert(
        &mut self,
        connection: ConnectionId,
        entity: EntityId,
        uuid: uuid::Uuid,
        name: String,
    ) {
        self.by_connection.insert(connection, entity);
        self.by_entity.insert(entity, connection);
        self.names.insert(connection, name);
        self.uuids.insert(connection, uuid);
    }

    /// Connection for a player entity, if joined.
    #[must_use]
    pub fn entity_connection(&self, entity: EntityId) -> Option<ConnectionId> {
        self.by_entity.get(&entity).copied()
    }

    /// Identity uuid for a connection, if joined.
    #[must_use]
    pub fn uuid_of(&self, connection: ConnectionId) -> Option<uuid::Uuid> {
        self.uuids.get(&connection).copied()
    }

    /// Display name for a player entity, if joined.
    #[must_use]
    pub fn name_of(&self, entity: EntityId) -> Option<&str> {
        self.by_entity
            .get(&entity)
            .and_then(|connection| self.names.get(connection))
            .map(String::as_str)
    }

    /// Negotiated client view radius for a connection, if the edge admitted one.
    /// Unnegotiated callers get `None` and fall back to the configured view.
    #[must_use]
    pub fn view_distance_of(&self, connection: ConnectionId) -> Option<u8> {
        self.view_distances.get(&connection).copied()
    }

    pub(crate) fn set_view_distance(&mut self, connection: ConnectionId, view_distance: u8) {
        self.view_distances.insert(connection, view_distance);
    }

    /// Client UI locale (`ClientInformation.language`) for a connection, if the
    /// edge admitted one. Unnegotiated callers get `None` and read English.
    #[must_use]
    pub fn locale_of(&self, connection: ConnectionId) -> Option<&str> {
        self.locales.get(&connection).map(String::as_str)
    }

    pub(crate) fn set_locale(&mut self, connection: ConnectionId, locale: String) {
        self.locales.insert(connection, locale);
    }

    pub(crate) fn remove_by_connection(&mut self, connection: ConnectionId) -> Option<EntityId> {
        self.view_distances.remove(&connection);
        self.locales.remove(&connection);
        let entity = self.by_connection.remove(&connection)?;
        self.by_entity.remove(&entity);
        self.names.remove(&connection);
        self.uuids.remove(&connection);
        Some(entity)
    }

    pub(crate) fn transfer_to(&mut self, connection: ConnectionId, destination: &mut Self) {
        let entity = self
            .by_connection
            .remove(&connection)
            .expect("joined connection");
        self.by_entity.remove(&entity);
        let uuid = self.uuids.remove(&connection).expect("joined UUID");
        let name = self.names.remove(&connection).expect("joined name");
        destination.insert(connection, entity, uuid, name);
        if let Some(view_distance) = self.view_distances.remove(&connection) {
            destination.view_distances.insert(connection, view_distance);
        }
        if let Some(locale) = self.locales.remove(&connection) {
            destination.locales.insert(connection, locale);
        }
    }

    /// Player entity for a connection, if joined.
    #[must_use]
    pub fn entity_of(&self, connection: ConnectionId) -> Option<EntityId> {
        self.by_connection.get(&connection).copied()
    }

    /// All joined connections (reliable fan-out targets).
    pub fn all_connections(&self) -> impl Iterator<Item = ConnectionId> + '_ {
        self.by_connection.keys().copied()
    }
}

/// One accepted damage intent, retaining its pre-commit hit context.
#[derive(Debug, Clone, Copy)]
pub struct DamagePiece {
    pub amount: f32,
    pub kind: crate::survival::DamageKind,
    pub source: Option<crate::intent::DamageSource>,
    pub(crate) physical: Option<crate::physical_defense::PhysicalImpact>,
    pub(crate) custom_armor: bool,
}

/// Authoritative tick state. Single owner: the tick thread.
pub struct TickState {
    pub config_sim_distance: i32,
    /// Client residency radius, independent of gameplay simulation.
    pub config_view_distance: i32,
    active_player_centers: Vec<ChunkPos>,
    active_ticket_ranges: Vec<(ChunkPos, i32, i32)>,
    active_sim_radius: i32,
    pub registry: Arc<CoreRegistry>,
    pub(crate) datapacks: crate::datapacks::Runtime,
    pub(crate) command_context: crate::command_context::Runtime,
    pub(crate) debug_profiler: crate::DebugProfiler,
    /// Whether a `/debug function` trace is executing (recursive traces refuse).
    pub(crate) debug_tracing: bool,
    /// Completed function-trace artifacts, oldest first.
    pub(crate) debug_traces: Vec<crate::DebugTraceArtifact>,
    pub world: ChunkWorld,
    /// This world's dimension; other worlds share its tick owner and WAL.
    pub dimension: crate::dimensions::Dimension,
    pub other_dimensions: std::collections::BTreeMap<crate::dimensions::Dimension, Box<TickState>>,
    pub(crate) dimension_requests: Vec<crate::dimensions::travel::Request>,
    pub(crate) dimension_respawns: Vec<EntityId>,
    pub(crate) dimension_routes: Vec<crate::dimensions::Dimension>,
    pub entities: EntityStore,
    pub connections: Connections,
    /// Opt-in native login fence; authorization belongs to the live session.
    pub(crate) require_plugin_login: bool,
    pub(crate) locked_sessions: BTreeSet<ConnectionId>,
    /// Consecutive unsupported non-descending movement samples permitted before
    /// anti-flight corrects and disconnects the player; zero disables it.
    pub(crate) max_airtime_ticks: u32,
    pub(crate) enable_command_block: bool,
    pub(crate) spawn_protection: u32,
    pub(crate) spawn_operators: std::collections::BTreeSet<uuid::Uuid>,
    pub(crate) brushing: crate::brushing::Brushing,
    pub(crate) command_blocks_due: Vec<BlockPos>,
    /// Per-mob switches layered under the `mob_griefing` gamerule.
    pub(crate) mob_griefing: crate::config::MobGriefingRules,
    /// Player state per player entity (inventory, menus, vitals, mode).
    pub players: HashMap<EntityId, crate::player::PlayerState>,
    /// Recovered offline records by uuid: consumed by Join to restore full
    /// player state (inventory, vitals, xp, mode, position).
    pub offline_players: HashMap<uuid::Uuid, crate::journal::ReplayPlayer>,
    pub(crate) advancement_events: Vec<crate::advancements::Event>,
    pub(crate) advancement_reward_spawns: Vec<EntityId>,
    pub(crate) native_globals:
        std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    pub(crate) plugins: crate::plugin::PluginState,
    pub(crate) plugin_runtime: Option<crate::plugin::PluginRuntime>,
    pub(crate) build_queries: Option<crate::build_precommit::PreviewQueries>,
    pub(crate) plugin_spawned: Vec<EntityId>,
    pub(crate) plugin_block_writes: Vec<(BlockPos, u32)>,
    pub(crate) plugin_players: crate::plugin_player::PlayerAuthority,
    pub(crate) plugin_damage: Vec<(EntitySlot, DamagePiece)>,
    pub(crate) plugin_interactions: HashMap<(EntityId, u32), mc_script::ScriptEvent>,
    /// Per-connection reliable outbox (bounded; overflow disconnects).
    pub outbox: HashMap<ConnectionId, Vec<PlayerChange>>,
    pub outbox_cap: usize,
    /// Connections to drop after this tick's publish: (connection, tick the
    /// `Disconnect` was sealed into its queue).
    pub disconnects: Vec<(ConnectionId, u64)>,
    pub tick: u64,
    /// Unix seconds source for real-calendar stamps (advancement obtained
    /// dates). Production reads the system clock; deterministic runs inject a
    /// fixed one, so the wall clock is an explicit external input.
    pub(crate) wall_clock: fn() -> i64,
    /// Secret per-world salt of [`TickState::entity_uuid`]; the world's own is
    /// installed by `recover_tick_state`. The zero default is for unit-test
    /// states, which want reproducible ids.
    pub(crate) entity_salt: [u8; crate::entity_uuid::SALT_LEN],
    pub day_time: i64,
    /// Block edits staged by merge this tick: (pos, new state).
    pub merge_block_writes: Vec<(BlockPos, u32)>,
    /// Reused block-phase projection; merge writes remain visible to commit.
    block_phase_writes: Vec<(BlockPos, u32)>,
    /// Reused ordered admission for serial simulation passes.
    pub(crate) serial_entity_slots: Vec<EntitySlot>,
    /// Byte admission scratch, rebuilt for each caller after structural changes.
    pub(crate) serial_entity_marks: Vec<u8>,
    /// Reused predicate caches; each owning pass resets them before reading.
    pub(crate) contact_hazards_scratch: crate::contact_hazards::Scratch,
    pub(crate) portal_match_cache: crate::world::SectionMatchCache,
    /// Negative portal probes, checked against identity/cells/content revision.
    pub(crate) portal_contact_stamps: Vec<Option<crate::world::ContactStamp>>,
    /// Accepted breaks retain their once-rolled, pre-write loot image.
    pub merge_broken: Vec<(EntityId, BlockPos, u32, Vec<mc_data::ItemStack>, i32)>,
    /// Command destroys use empty-tool loot regardless of sender game mode.
    pub(crate) merge_command_broken: Vec<(BlockPos, u32, Vec<mc_data::ItemStack>)>,
    pub(crate) merge_command_block_entities: Vec<(BlockPos, crate::command_block_entity::Prepared)>,
    /// Container contents staged by merge for broken containers:
    /// ((x,y,z), slots). Commit spawns each non-empty slot as a drop,
    /// closes viewers, removes storage, marks dirty, stages the tombstone.
    pub merge_container_contents: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    /// Viewers of containers broken this tick: (viewer, broken cell). A
    /// double chest's viewers close when either half breaks, so they are
    /// found at merge time, while both halves still stand. Commit closes
    /// each menu.
    pub merge_closed_viewers: Vec<(EntityId, (i32, i32, i32))>,
    /// Entity damage staged by merge: target slot and source-aware hit.
    pub merge_damage: Vec<(EntitySlot, DamagePiece)>,
    /// Item entities consumed by merge.
    pub merge_consumed_items: HashSet<EntitySlot>,
    /// Children spawned by merge.
    pub merge_spawns: Vec<MergeSpawn>,
    /// Animal harvest drops, journaled with all other committed spawns.
    pub merge_life_drops: Vec<([f64; 3], mc_data::ItemStack)>,
    /// Buckets filled/emptied out of a stack: (actor, new bucket item).
    pub merge_gives: Vec<(EntityId, u32)>,
    /// Scheduled water/lava ticks.
    pub fluid_ticks: crate::fluids::FluidTicks,
    pub(crate) block_ticks: crate::block_ticks::BlockTicks,
    pub(crate) redstone: crate::redstone::RedstoneState,
    pub(crate) redstone_containers: crate::redstone::containers::ContainerState,
    /// Random-tick section eligibility keyed by committed chunk revision.
    pub(crate) farm_sections: crate::farming::FarmSections,
    /// Pickup events staged by merge: (item slot, collector slot, count).
    pub merge_pickups: Vec<(EntitySlot, EntitySlot, i32)>,
    /// Entities whose viewer-visible appearance changed this tick (mob
    /// health, partially picked item stacks): published as metadata.
    pub appearance_changed: Vec<EntityId>,
    /// Players whose final Leave image entered `offline_players` this tick;
    /// drained by the journal so it never scans every offline record.
    pub(crate) left_players: Vec<uuid::Uuid>,
    /// Reused tick-local fence index: knot id and whether a live leash uses it.
    pub(crate) leash_knots: HashMap<[i32; 3], (EntityId, bool)>,
    /// Remote tickets: owner -> (centre chunk, radius).
    pub tickets: BTreeMap<String, (ChunkPos, i32)>,
    /// Derived pearl tickets, owner presence and the WAL ticket diff.
    pub(crate) chunk_tickets: crate::chunk_tickets::ChunkTickets,
    /// World revision when the previous entity phase read the world: a
    /// resting mob whose footprint chunks are no newer stays valid.
    pub entity_phase_revision: u64,
    /// Reused, non-authoritative entity-phase buffers; empty between ticks.
    entity_phase_scratch: EntityPhaseScratch,
    /// Reused per-tick explosion line-of-sight results; never persists or
    /// changes the ordered explosion application.
    pub(crate) explosion_exposure: crate::explosion::ExposureCache,
    /// Last swing tick per attacker (combat strength recharge).
    pub last_swing: HashMap<EntityId, u64>,
    /// Stand-specific player double-punch window; environmental hurt is unrelated.
    pub(crate) armor_stand_hits: HashMap<EntityId, u64>,
    /// World difficulty (commands).
    pub difficulty: crate::survival::Difficulty,
    pub(crate) journaled_difficulty: Option<crate::survival::Difficulty>,
    /// Visible weather target and independent rain-clock duration.
    pub weather: (crate::commands::Weather, i64),
    pub(crate) thunder_remaining_ticks: i64,
    pub(crate) clear_remaining_ticks: i64,
    pub(crate) thundering: bool,
    /// Reused precipitation worklist; only active resident columns are sampled.
    pub(crate) weather_chunks: Vec<ChunkPos>,
    /// Retained client fade levels, independent of the target and cycle clock.
    pub(crate) rain_level: f32,
    pub(crate) thunder_level: f32,
    pub(crate) journaled_climate: Option<crate::weather::ClimateImage>,
    pub border: crate::command_border::Border,
    pub(crate) journaled_border: Option<crate::command_border::Border>,
    /// Game rules (commands).
    pub gamerules: HashMap<String, String>,
    pub(crate) gamerules_dirty: bool,
    /// Transient category counts and chunk-local spawn overlap candidates.
    pub(crate) spawn_census: crate::natural_spawn::SpawnCensus,
    /// Column rest-height cache: (block x, block z) -> rest feet y.
    /// Next per-actor intent sequence.
    seq_counter: HashMap<EntityId, u32>,
    /// Live block-entity storage by block position (chest/furnace contents).
    /// Journaled + checkpointed as post-images.
    pub block_entities: crate::redstone::custody::BlockEntities,
    /// Block-entity positions removed this tick (container broken): emitted
    /// as `JournalEntry::RemoveBlockEntity` and cleared at commit, so the
    /// tombstone shares the tick with the block change.
    pub block_entity_removed: Vec<(i32, i32, i32)>,
    /// Open container position per player entity (set by UseItemOn).
    pub open_containers: crate::redstone::custody::OpenContainers,
    pub(crate) vehicle_menus: crate::vehicles::VehicleMenus,
    /// Client-predicted block uses and respawns wait for native world admission.
    pub(crate) deferred_uses: Vec<PlayerInput>,
    pub(crate) sign_editors: HashMap<ConnectionId, crate::signs::Editor>,
    pub opaque_changes: Vec<(BlockPos, Option<Vec<u8>>)>,
    pub(crate) gateway_ages: HashMap<BlockPos, crate::end_portals::GatewayAge>,
    pub(crate) campfires: crate::campfire::Campfires,
    pub(crate) hives: crate::bees::Hives,
    pub(crate) jukeboxes: crate::jukebox::Jukeboxes,
    pub(crate) lecterns: crate::lectern::Lecterns,
    pub(crate) brewing: crate::brewing::BrewingStands,
    pub(crate) crafters: crate::crafter::Crafters,
    pub(crate) merge_shulker_placements: Vec<crate::shulker::Placement>,
    pub(crate) vibrations: crate::vibration::Bus,
    pub(crate) sculk: crate::sculk::SculkState,
    pub(crate) trial_spawners: crate::trial_spawner::Spawners,
    pub(crate) monster_spawners: crate::monster_spawner::Spawners,
    pub(crate) vaults: crate::vault::Vaults,
    pub(crate) shriekers: crate::shrieker::Shriekers,
    /// Players lying in a bed ([`crate::sleep`]).
    pub sleepers: HashMap<EntityId, crate::sleep::Sleeper>,
    /// Last journaled player / block-entity post-images. Images are
    /// absolute, so the journal writes one only when it differs from the
    /// last; an unchanged record is still covered by an earlier batch or
    /// the checkpoint.
    pub journaled_players: HashMap<EntityId, crate::journal::ReplayPlayer>,
    pub journaled_block_entities: HashMap<(i32, i32, i32), BlockEntityImage>,
    /// Attributed block/container history behind `/co` (dimension-local).
    pub(crate) block_log: crate::block_log::BlockLog,
    pub(crate) journaled_villagers: HashMap<EntityId, (u64, Box<crate::villager::VillagerState>)>,
    /// Last comparison tick and chunk anchor lifecycle clocks and crossings.
    pub(crate) journaled_entity_state:
        HashMap<EntityId, (u64, ChunkPos, Arc<crate::journal::EntityStateImage>)>,
    /// Touched-set journal caches (motion membership, clock rechecks, sweeps).
    pub(crate) journal_scan: crate::journal_touched::JournalScan,
    /// Creeper fuses, ranged mob charging and committed blasts.
    pub hostile: crate::hostile::HostileState,
    pub projectiles: crate::projectiles::ProjectileRuntime,
    pub end_fight: crate::end_fight::Fight,
    pub(crate) journaled_end_fight: Option<crate::end_fight::Fight>,
    pub raids: crate::raids::Raids,
    pub(crate) journaled_raids: Option<crate::raids::Raids>,
    pub(crate) trader_spawner: crate::mob_lanes::TraderSpawner,
    /// Vanilla restart-local 1200-tick admission timer; not saved data.
    pub(crate) trader_tick_delay: u32,
    pub(crate) journaled_trader_spawner: Option<crate::mob_lanes::TraderSpawner>,
    pub scoreboard: crate::scoreboard::Scoreboard,
    pub(crate) command_storage: crate::command_storage::Storage,
    pub(crate) command_storage_dirty: bool,
    pub maps: crate::maps::Maps,
    pub scoreboard_dirty: bool,
    pub bossbars_dirty: bool,
    pub(crate) dimension_globals: crate::dimensions::globals::Globals,
    pub(crate) villager_pois: crate::settlements::allocator::PoiCache,
    pub(crate) generated_entity_claims: std::collections::BTreeSet<String>,
    pub(crate) new_generated_entity_claims: Vec<String>,
    pub(crate) generated_entity_initial_scan: bool,
    pub(crate) settlements: crate::settlements::Registry,
    pub(crate) settlement_sequence: u64,
    pub(crate) autonomy_queue: crate::settlements::autonomy::AutonomyQueue,
    /// Per-connection certify-dialog corner marks (transient UI draft, never persisted).
    pub(crate) settlement_marks: HashMap<ConnectionId, crate::settlements::ui::Marks>,
    pub(crate) tick_budget: crate::tick_budget::Budget,
    /// Global `/tick` rate manager (moved between worlds with the other globals).
    pub(crate) tick_control: crate::tick_control::TickControl,
    /// Global `/bossbar` registry (moved between worlds with the other globals).
    pub bossbars: crate::bossbar::BossBars,
    /// `/locate` async searches owned by this dimension's tick (worker + drain).
    pub(crate) locate: crate::command_locate::LocateState,
}

/// Journaled block-entity contents: slots + furnace progress.
pub type BlockEntityImage = (
    Vec<mc_data::ItemStack>,
    Option<crate::journal::FurnaceImage>,
);

/// Live block-entity contents (slots + furnace progress).
#[derive(Debug, Clone, Default)]
pub struct BlockEntityState {
    pub slots: Vec<mc_data::ItemStack>,
    pub furnace: Option<crate::inventory::furnace::FurnaceState>,
}

impl BlockEntityState {
    /// Journal/checkpoint image of the furnace progress.
    pub(crate) fn furnace_image(&self) -> Option<crate::journal::FurnaceImage> {
        self.furnace
            .as_ref()
            .map(|progress| crate::journal::FurnaceImage {
                lit_remaining: progress.lit_remaining,
                lit_total: progress.lit_total,
                cook_progress: progress.cook_progress,
                cook_total: progress.cook_total,
                recipes_used: Arc::clone(&progress.recipes_used),
            })
    }
}

/// One child spawn staged by merge.
#[derive(Debug, Clone)]
pub struct MergeSpawn {
    pub pos: [f64; 3],
    pub type_id: u32,
    pub health: f32,
    pub wool: Option<u8>,
    pub taming: Option<crate::taming::TamingState>,
    pub advancement_breeding: Option<(EntityId, serde_json::Value)>,
}

/// Per-cell records decoded from one chunk (hydration decode phase output).
pub(crate) type CellRecords<T> = Vec<((i32, i32, i32), T)>;

impl TickState {
    /// Serial-pass block write: visible to later reads this tick and staged
    /// for the canonical block phase (neighbours, comparators, WAL).
    pub(crate) fn write_block(&mut self, pos: BlockPos, id: u32) {
        self.world.set_block(pos.x, pos.y, pos.z, id);
        self.merge_block_writes.push((pos, id));
    }

    pub fn new(
        registry: Arc<CoreRegistry>,
        world: ChunkWorld,
        sim_distance: i32,
        outbox_cap: usize,
    ) -> Self {
        Self {
            config_sim_distance: sim_distance,
            config_view_distance: sim_distance,
            active_player_centers: Vec::new(),
            active_ticket_ranges: Vec::new(),
            active_sim_radius: -1,
            registry,
            world,
            dimension: crate::dimensions::Dimension::Overworld,
            other_dimensions: Default::default(),
            dimension_requests: Vec::new(),
            dimension_respawns: Vec::new(),
            dimension_routes: Vec::new(),
            entities: EntityStore::default(),
            connections: Connections::default(),
            require_plugin_login: false,
            locked_sessions: BTreeSet::new(),
            max_airtime_ticks: 80,
            enable_command_block: false,
            spawn_protection: 16,
            spawn_operators: Default::default(),
            brushing: Default::default(),
            command_blocks_due: Vec::new(),
            mob_griefing: crate::config::MobGriefingRules::default(),
            players: HashMap::default(),
            offline_players: HashMap::default(),
            advancement_events: Vec::new(),
            advancement_reward_spawns: Vec::new(),
            native_globals: Default::default(),
            datapacks: Default::default(),
            command_context: Default::default(),
            debug_profiler: Default::default(),
            debug_tracing: false,
            debug_traces: Vec::new(),
            plugins: Default::default(),
            plugin_runtime: None,
            build_queries: None,
            plugin_spawned: Vec::new(),
            plugin_block_writes: Vec::new(),
            plugin_players: Default::default(),
            plugin_damage: Vec::new(),
            plugin_interactions: HashMap::default(),
            outbox: HashMap::default(),
            outbox_cap,
            disconnects: Vec::new(),
            tick: 0,
            wall_clock: crate::advancements::system_unix_seconds,
            entity_salt: [0; crate::entity_uuid::SALT_LEN],
            day_time: 0,
            merge_block_writes: Vec::new(),
            block_phase_writes: Vec::new(),
            serial_entity_slots: Vec::new(),
            serial_entity_marks: Vec::new(),
            contact_hazards_scratch: Default::default(),
            portal_match_cache: Default::default(),
            portal_contact_stamps: Vec::new(),
            merge_broken: Vec::new(),
            merge_command_broken: Vec::new(),
            merge_container_contents: Vec::new(),
            merge_closed_viewers: Vec::new(),
            merge_damage: Vec::new(),
            merge_consumed_items: HashSet::default(),
            merge_spawns: Vec::new(),
            merge_life_drops: Vec::new(),
            merge_gives: Vec::new(),
            fluid_ticks: crate::fluids::FluidTicks::default(),
            block_ticks: crate::block_ticks::BlockTicks::default(),
            redstone: crate::redstone::RedstoneState::default(),
            redstone_containers: crate::redstone::containers::ContainerState::default(),
            farm_sections: crate::farming::FarmSections::default(),
            merge_pickups: Vec::new(),
            appearance_changed: Vec::new(),
            left_players: Vec::new(),
            leash_knots: HashMap::default(),
            tickets: BTreeMap::new(),
            chunk_tickets: crate::chunk_tickets::ChunkTickets::default(),
            entity_phase_revision: 0,
            entity_phase_scratch: EntityPhaseScratch::default(),
            explosion_exposure: crate::explosion::ExposureCache::default(),
            last_swing: HashMap::default(),
            armor_stand_hits: HashMap::default(),
            // Vanilla dedicated-server default; the join sends it.
            difficulty: crate::survival::Difficulty::Easy,
            journaled_difficulty: None,
            weather: (crate::commands::Weather::Clear, 0),
            thunder_remaining_ticks: 0,
            clear_remaining_ticks: 0,
            thundering: false,
            weather_chunks: Vec::new(),
            rain_level: 0.0,
            thunder_level: 0.0,
            journaled_climate: None,
            border: Default::default(),
            journaled_border: None,
            gamerules: HashMap::default(),
            gamerules_dirty: false,
            spawn_census: Default::default(),
            seq_counter: HashMap::default(),
            block_entities: Default::default(),
            block_entity_removed: Vec::new(),
            open_containers: Default::default(),
            vehicle_menus: crate::vehicles::VehicleMenus::default(),
            deferred_uses: Vec::new(),
            sign_editors: HashMap::default(),
            opaque_changes: Vec::new(),
            gateway_ages: HashMap::default(),
            campfires: crate::campfire::Campfires::default(),
            hives: crate::bees::Hives::default(),
            jukeboxes: crate::jukebox::Jukeboxes::default(),
            lecterns: crate::lectern::Lecterns::default(),
            brewing: crate::brewing::BrewingStands::default(),
            crafters: crate::crafter::Crafters::default(),
            merge_shulker_placements: Vec::new(),
            merge_command_block_entities: Vec::new(),
            vibrations: crate::vibration::Bus::default(),
            sculk: crate::sculk::SculkState::default(),
            trial_spawners: Default::default(),
            monster_spawners: Default::default(),
            vaults: Default::default(),
            shriekers: Default::default(),
            sleepers: HashMap::default(),
            journaled_players: HashMap::default(),
            journaled_block_entities: HashMap::default(),
            block_log: Default::default(),
            journaled_villagers: HashMap::default(),
            journaled_entity_state: HashMap::default(),
            journal_scan: Default::default(),
            hostile: crate::hostile::HostileState::default(),
            projectiles: crate::projectiles::ProjectileRuntime::default(),
            end_fight: Default::default(),
            journaled_end_fight: None,
            raids: Default::default(),
            journaled_raids: None,
            trader_spawner: Default::default(),
            trader_tick_delay: 1200,
            journaled_trader_spawner: None,
            scoreboard: crate::scoreboard::Scoreboard::default(),
            command_storage: Default::default(),
            command_storage_dirty: false,
            maps: Default::default(),
            scoreboard_dirty: false,
            bossbars_dirty: false,
            bossbars: Default::default(),
            dimension_globals: Default::default(),
            villager_pois: Default::default(),
            generated_entity_claims: Default::default(),
            new_generated_entity_claims: Vec::new(),
            generated_entity_initial_scan: false,
            settlements: Default::default(),
            settlement_sequence: 0,
            autonomy_queue: Default::default(),
            settlement_marks: HashMap::default(),
            tick_budget: Default::default(),
            tick_control: Default::default(),
            locate: Default::default(),
        }
    }

    pub(crate) fn session_locked(&self, connection: ConnectionId) -> bool {
        self.require_plugin_login && self.locked_sessions.contains(&connection)
    }

    pub(crate) fn entity_session_locked(&self, entity: EntityId) -> bool {
        self.require_plugin_login
            && self
                .players
                .get(&entity)
                .is_some_and(|player| self.locked_sessions.contains(&player.connection))
    }

    pub(crate) fn next_seq(&mut self, actor: EntityId) -> u32 {
        let seq = self.seq_counter.get(&actor).copied().unwrap_or(0);
        self.seq_counter.insert(actor, seq + 1);
        seq
    }

    /// Reliable per-player push. On cap overflow the queue is sealed with a
    /// terminal `Disconnect` (always last) and the connection is scheduled
    /// for Leave-equivalent removal on a later tick; further pushes are
    /// dropped so nothing can land after `Disconnect`.
    pub fn push_player(&mut self, connection: ConnectionId, change: PlayerChange) {
        if self
            .disconnects
            .iter()
            .any(|(sealed, _)| *sealed == connection)
        {
            return;
        }
        crate::vibration::menu(self, connection, &change);
        let queue = self.outbox.entry(connection).or_default();
        if queue.len() >= self.outbox_cap {
            queue.push(PlayerChange::Disconnect {
                reason: "reliable queue overflow".to_owned(),
            });
            self.disconnects.push((connection, self.tick));
            return;
        }
        queue.push(change);
    }

    /// Fence movement when the authoritative sync is created, before the
    /// off-tick edge can send it or admit an older queued client pose.
    pub(crate) fn correct_position(
        &mut self,
        connection: ConnectionId,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) {
        let Some(entity) = self.connections.entity_of(connection) else {
            return;
        };
        let Some(player) = self.players.get_mut(&entity) else {
            return;
        };
        let teleport_id = player.begin_teleport();
        self.push_player(
            connection,
            PlayerChange::PositionCorrected {
                teleport_id,
                position,
                yaw,
                pitch,
            },
        );
    }

    pub(crate) fn stage_command_break(&mut self, pos: BlockPos, broken: u32) -> bool {
        let drops =
            if crate::command_rules::boolean(self, crate::command_rules::BoolRule::BLOCK_DROPS) {
                crate::block_drop_context::roll(
                    self,
                    pos,
                    broken,
                    &mc_data::ItemStack::EMPTY,
                    self.tick ^ ((pos.x as u64) << 40) ^ ((pos.z as u64) << 16) ^ pos.y as u64,
                )
            } else {
                Ok(Vec::new())
            };
        let Some(drops) = crate::block_drop_context::admit(self, pos, drops) else {
            return false;
        };
        self.stage_container_removal(pos, !crate::block_drop_context::retains_contents(&drops));
        self.merge_command_broken.push((pos, broken, drops));
        true
    }

    pub(crate) fn stage_command_container_removal(&mut self, pos: BlockPos) {
        self.stage_container_removal(pos, false);
    }

    pub(crate) fn stage_container_removal(&mut self, pos: BlockPos, drop_contents: bool) {
        crate::plugin_inventories::invalidate_block(self, pos);
        let cell = (pos.x, pos.y, pos.z);
        crate::sculk::removed(self, pos);
        if self
            .merge_container_contents
            .iter()
            .any(|(other, _)| *other == cell)
        {
            return;
        }
        let record = self.block_entities.get(&cell);
        let opaque = self
            .world
            .chunk(ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .is_some_and(|chunk| chunk.block_entities.contains_key(&pos));
        if record.is_none() && !opaque {
            return;
        }
        let slots = if drop_contents {
            record
                .map(|record| record.slots.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.merge_container_contents.push((cell, slots));
        let viewers: Vec<EntityId> = self
            .open_containers
            .iter()
            .filter(|(_, open)| container_cells(self, **open).contains(&cell))
            .map(|(viewer, _)| *viewer)
            .collect();
        self.merge_closed_viewers
            .extend(viewers.into_iter().map(|viewer| (viewer, cell)));
    }
}

/// Wool byte (color id, `0x10` sheared) of a wool-growing mob; white when
/// unset.
pub(crate) fn wool_of(appearance: &EntityAppearance) -> u8 {
    appearance
        .extra
        .iter()
        .find_map(|(index, value)| match value {
            crate::boundary::EntityDataValue::Byte(byte)
                if *index == crate::blueprint::SHEEP_WOOL_DATA_INDEX =>
            {
                Some(*byte as u8)
            }
            _ => None,
        })
        .unwrap_or(0)
}

/// Writes the wool byte of a wool-growing mob.
pub(crate) fn set_wool(appearance: &mut EntityAppearance, wool: u8) {
    let value = crate::boundary::EntityDataValue::Byte(wool as i8);
    match appearance
        .extra
        .iter_mut()
        .find(|(index, _)| *index == crate::blueprint::SHEEP_WOOL_DATA_INDEX)
    {
        Some(entry) => entry.1 = value,
        None => appearance
            .extra
            .push((crate::blueprint::SHEEP_WOOL_DATA_INDEX, value)),
    }
}

/// Block sampler over the tick's read state for `mc-physics`.
pub(crate) struct TickSampler<'a> {
    pub(crate) world: &'a ChunkWorld,
    pub(crate) registry: &'a CoreRegistry,
}

impl mc_physics::BlockSampler for TickSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        match self.world.physics_block_at(x, y, z) {
            Some(state) => self.registry.materials.classify(state),
            // Missing chunk blocks movement: never a floor, never passable.
            None => mc_physics::BlockMaterial::Solid,
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        self.registry.max_collision_box_y
    }

    fn stationary_downward_clip(
        &self,
        body: mc_physics::EntityBody,
        displacement_y: f64,
        max_scan_cells: usize,
        max_candidates: usize,
    ) -> Option<f64> {
        mc_physics::stationary_support_clip(
            self,
            body,
            displacement_y,
            max_scan_cells,
            max_candidates,
            self.registry.max_collision_boxes_per_cell,
        )
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        emit_collision_boxes(self.registry, self.world.physics_block_at(x, y, z), emit);
    }
}

/// Worker-thread sampler for the entity phase: missing columns resolve as
/// [`BlockMaterial::Solid`] so unloaded chunks block motion and never form
/// a floor. Shares the tick's read state by reference (no locks, no copies).
struct WorkerSampler<'a> {
    block_at: &'a dyn Fn(i32, i32, i32) -> Option<u32>,
    registry: &'a CoreRegistry,
}

impl mc_physics::BlockSampler for WorkerSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        match (self.block_at)(x, y, z) {
            Some(state) => self.registry.materials.classify(state),
            None => mc_physics::BlockMaterial::Solid,
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        self.registry.max_collision_box_y
    }

    fn stationary_downward_clip(
        &self,
        body: mc_physics::EntityBody,
        displacement_y: f64,
        max_scan_cells: usize,
        max_candidates: usize,
    ) -> Option<f64> {
        mc_physics::stationary_support_clip(
            self,
            body,
            displacement_y,
            max_scan_cells,
            max_candidates,
            self.registry.max_collision_boxes_per_cell,
        )
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        emit_collision_boxes(self.registry, (self.block_at)(x, y, z), emit);
    }
}

fn emit_collision_boxes(
    registry: &CoreRegistry,
    state: Option<u32>,
    emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
) {
    if let Some(boxes) = state.and_then(|state| registry.collision_boxes.get(state as usize)) {
        for &collision_box in boxes {
            emit(collision_box);
        }
    } else {
        // Unknown states and missing chunks remain conservative obstacles.
        emit(mc_physics::BlockCollisionBox::FULL_BLOCK);
    }
}

#[cfg(test)]
#[path = "tick_collision_tests.rs"]
mod collision_sampler_tests;

/// Build a [`PlayerState`] from an offline record (rejoin) or defaults.
/// Pure data hydration: inventory slots, vitals, xp, game mode.
fn hydrate_player(
    connection: ConnectionId,
    offline: Option<&crate::journal::ReplayPlayer>,
) -> crate::player::PlayerState {
    let mut player =
        crate::player::PlayerState::new(connection, crate::commands::GameMode::Survival);
    let Some(record) = offline else {
        return player;
    };
    player
        .opaque_player_fields
        .clone_from(&record.opaque_player_fields);
    player.statistics.clone_from(&record.statistics);
    player
        .advancement_progress
        .clone_from(&record.advancement_progress);
    player.unlocked_recipes.clone_from(&record.unlocked_recipes);
    if record.inventory.len() == player.inventory.slots.len() {
        player.inventory.slots = record.inventory.clone();
    }
    player.inventory.selected = record.selected.min(8);
    player.vitals.health = record.health;
    player.vitals.food = record.food;
    player.vitals.saturation = record.saturation;
    player.vitals.exhaustion = record.exhaustion;
    player.vitals.air = record.air;
    player.vitals.experience.level = record.experience_level;
    player.vitals.experience.progress = record.experience_progress;
    player.vitals.experience.total = record.experience_total;
    player.respawn_point = record.respawn;
    player.respawn_dimension = record.respawn_dimension;
    player.respawn_forced = record.respawn_forced;
    player.respawn_yaw = record.respawn_yaw;
    player.respawn_pitch = record.respawn_pitch;
    player.active_effects = record.active_effects.clone();
    player.current_impulse_impact_pos = record.current_impulse_impact_pos;
    player.fall_flying = record.fall_flying;
    player.warden_warning = record.warden_warning;
    player.enchantment_seed = record.enchantment_seed;
    player.enchantment_offer_seed = record.enchantment_offer_seed;
    player.game_mode = match record.game_mode {
        1 => crate::commands::GameMode::Creative,
        2 => crate::commands::GameMode::Adventure,
        3 => crate::commands::GameMode::Spectator,
        _ => crate::commands::GameMode::Survival,
    };
    player
}

/// Run one full tick: inputs -> entity phase -> merge -> block phase ->
/// commit -> publish. Returns the [`TickOutput`] plus per-connection
/// reliable outbox drains and the phase times (`total_us` is filled by the
/// tick loop).
pub fn run_tick(
    state: &mut TickState,
    inputs: &[PlayerInput],
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let wrapper_start = std::time::Instant::now();
    let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
        .is_some()
        .then(std::time::Instant::now);
    crate::plugin::pump(state);
    crate::tick_control::begin_tick(state);
    let pump_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    let dimensions_start = std::time::Instant::now();
    let mut result = crate::dimensions::run_tick(state, inputs);
    let dimensions_time = dimensions_start.elapsed();
    let wrapper_tail_start = std::time::Instant::now();
    let dimensions_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    crate::build_precommit::answer_previews(state);
    let previews_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    crate::plugin::observe_players(state);
    result.2.plugins_us = dimensions_start.duration_since(wrapper_start).as_micros() as u64
        + wrapper_tail_start.elapsed().as_micros() as u64;
    // This envelope includes routing/travel/globals and destruction of local
    // phase scratch after the dimension function returns. It is not disk I/O
    // or edge network fanout.
    let local_us = result.2.input_us
        + result.2.chunks_us
        + result.2.entity_us
        + result.2.merge_us
        + result.2.block_us
        + result.2.commit_us
        + result.2.snapshot_us
        + result.2.views_us
        + result.2.publish_us;
    result.2.coordination_us = (dimensions_time.as_micros() as u64).saturating_sub(local_us);
    if let Some(start) = phase_start {
        let total = start.elapsed();
        eprintln!(
            "tick wrapper ms: tick={} pump={:.2} dimensions={:.2} previews={:.2} observe={:.2} total={:.2}",
            state.tick,
            pump_done.as_secs_f64() * 1000.0,
            dimensions_done.saturating_sub(pump_done).as_secs_f64() * 1000.0,
            previews_done.saturating_sub(dimensions_done).as_secs_f64() * 1000.0,
            total.saturating_sub(previews_done).as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0,
        );
    }
    result
}

struct DimensionPhaseLog {
    start: std::time::Instant,
    before_return: std::time::Duration,
    logging: std::time::Duration,
    allocator_us: u64,
}

/// One local world's phases; the dimension coordinator owns the global tick.
pub(crate) fn run_dimension_tick<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let mut phase_log =
        std::env::var_os("MC_CORE_PHASE_LOG")
            .is_some()
            .then(|| DimensionPhaseLog {
                start: std::time::Instant::now(),
                before_return: std::time::Duration::ZERO,
                logging: std::time::Duration::ZERO,
                allocator_us: 0,
            });
    let result = run_dimension_tick_inner(state, inputs, phase_log.as_mut());
    if let Some(log) = phase_log {
        // The inner call has returned, so this includes destruction of every
        // phase-local allocation, unlike the phase print inside that call.
        let total = log.start.elapsed();
        eprintln!(
            "dimension envelope ms: tick={} dimension={:?} total={:.2} return_cleanup={:.2} phase_log={:.2}",
            state.tick,
            state.dimension,
            total.as_secs_f64() * 1000.0,
            total
                .saturating_sub(log.before_return)
                .saturating_sub(log.logging)
                .as_secs_f64()
                * 1000.0,
            log.logging.as_secs_f64() * 1000.0,
        );
    }
    result
}

/// Publish current player attributes from the inventory/effect owners.
pub(crate) fn refresh_player_attributes(state: &mut TickState, dirty_only: bool) {
    let attribute_changes: Vec<_> = state
        .players
        .iter_mut()
        .filter_map(|(&entity, player)| {
            if dirty_only && player.attribute_equipment.is_some() {
                return None;
            }
            let connection = state.connections.entity_connection(entity)?;
            let change = player.equipment_attributes(
                &state.registry.item_rules(),
                &state
                    .entities
                    .row(state.entities.slot_of(entity)?)
                    .loot
                    .attributes,
            )?;
            let max_health = player.effective_attribute("max_health").unwrap_or(20.0) as f32;
            let health = if player.vitals.health > max_health {
                player.vitals.health = max_health;
                Some((max_health, player.health_change()))
            } else {
                None
            };
            Some((entity, connection, change, health))
        })
        .collect();
    for (entity, connection, change, health) in attribute_changes {
        state.push_player(connection, change);
        if let Some((health, health_change)) = health {
            if let Some(slot) = state.entities.slot_of(entity) {
                let row = state.entities.row_mut(slot);
                row.health = health;
                Arc::make_mut(&mut row.appearance).health = health;
                state.appearance_changed.push(entity);
            }
            state.push_player(connection, health_change);
        }
    }
}

fn run_dimension_tick_inner<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
    mut phase_log: Option<&mut DimensionPhaseLog>,
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let input_start = std::time::Instant::now();
    refresh_player_attributes(state, false);
    // False while `/tick freeze` holds the world: only players, input, chunk
    // loading and merge/commit run; see `tick_control`.
    let sim = state.tick_control.runs_normally();
    let previous_weather = crate::weather::view(state);
    for player in state.players.values_mut() {
        player.statistics.advance_tick(state.tick);
        player.chat_spam_ticks = player.chat_spam_ticks.saturating_sub(1);
    }
    crate::chunk_tickets::refresh(state);
    state.refresh_active_chunks(state.tick + 1);
    // 1. Natural spawns/despawns, then inputs: player join/leave/move/dig/
    // attack/chat; intents staged.
    let mut intents: Vec<Intent> = Vec::new();
    let mut changes = ChangeSet::default();
    if sim {
        crate::datapacks::tick(state, &mut changes);
    }
    if sim {
        crate::natural_spawn::advance_ages_and_run(state, &mut changes);
        crate::mob_lanes::traders(state, &mut changes);
        crate::vibration::tick(state);
        crate::plugin_npcs::advance(state);
    }
    changes.entities_spawned.append(&mut state.plugin_spawned);
    state
        .merge_block_writes
        .append(&mut state.plugin_block_writes);
    let mut deferred = std::mem::take(&mut state.deferred_uses);
    // MC-28289: snapshot each player's held main-hand stack BEFORE this
    // tick's inputs run, so the melee path resolves attack_damage /
    // attack_speed from the pre-tick item when a swap+attack share a tick.
    for player in state.players.values_mut() {
        let held = player
            .inventory
            .slots
            .get(usize::from(player.inventory.selected.min(8)))
            .cloned()
            .unwrap_or(mc_data::ItemStack::EMPTY);
        player.attribute_snapshot_held = Some(held);
    }
    mining::advance(state);
    mining::take_finished(state, &mut deferred);
    apply_inputs(state, &deferred, &mut intents, &mut changes);
    apply_inputs(state, inputs, &mut intents, &mut changes);
    state.refresh_active_chunks(state.tick + 1);
    crate::physical_attacks::advance(state, &mut intents);
    if sim {
        crate::shrieker::run(state);
    }
    tick_players(state, &mut changes, &mut intents);
    if sim {
        crate::raids::raid_omens(state);
    }
    crate::effects::run(state, &mut intents);
    crate::contact_hazards::run(state, &mut intents);
    crate::dimensions::rules::void_intents(state, &mut intents);
    crate::enchantment_locations::run(state, &mut intents);
    if sim {
        crate::end_fight::prepare(state);
    }
    let input_time = input_start.elapsed();

    // Desired chunks from players + tickets; load/unload.
    let chunk_start = std::time::Instant::now();
    let want = state.desired_chunks();
    let loaded = state.world.ensure(&want);
    state.refresh_active_chunks(state.tick + 1);
    state.accumulate_inhabited_time();
    // Inputs can mutate a resident-inactive container immediately before it
    // leaves demand. Fence its old time before eviction can forget authority.
    crate::abstract_sim::cancel_external_writes(state);
    for &(pos, _) in &state.merge_block_writes {
        state.world.cancel_simulation_debt(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        });
    }
    let unloaded = state.world.evict_outside(&want);
    state
        .block_ticks
        .evict_flushed(|x, z| state.world.unloaded_clean(x, z));
    state
        .fluid_ticks
        .evict_flushed(|x, z| state.world.unloaded_clean(x, z));
    crate::fluids::hydrate(state, &loaded);
    crate::villager::generated::load(state, &loaded, &mut changes);
    changes.chunks_loaded.extend(loaded);
    changes.chunks_unloaded.extend(unloaded);
    // Block entities ride inside chunk regions: pull newly resident chunks'
    // typed maps into live storage (journal folds apply over them).
    sync_chunk_block_entities(state);
    if sim {
        crate::settlements::autonomy::tick(state, &mut changes);
        crate::conduit::run(state, &mut intents);
        crate::beacon::run(state);
        crate::dolphin::run(state);
    }
    // Immediate respawn retries after off-tick chunk loading; unavailable
    // personal spawn chunks must never turn into a fallback-world respawn.
    if crate::command_rules::boolean(state, crate::command_rules::BoolRule::IMMEDIATE_RESPAWN) {
        for index in 0..state.entities.len() as u32 {
            let body = state.entities.body(EntitySlot(index));
            if !body.alive
                && let EntityKind::Player { connection } = body.kind
            {
                respawn_player(state, connection, &mut changes);
            }
        }
    }
    let chunk_time = chunk_start.elapsed();

    // 2. Entity phase (parallel by data over the fixed pool).
    let entity_start = std::time::Instant::now();
    if sim {
        crate::entity_life::advance(state);
        crate::brushing::run(state);
        crate::piglin::run(state, &mut changes);
        crate::tnt::run(state, &mut intents);
        crate::mob_lanes::snowballs(state);
    }
    let entity_intents = run_entity_phase(state, sim);
    let entity_time = entity_start.elapsed();
    let entity_tail_start = std::time::Instant::now();
    intents.extend(entity_intents);
    if sim {
        crate::special_mobs::raiders::run(state, &mut intents);
    }
    crate::vehicles::run(state, &mut intents, &mut changes);
    crate::mounts::menus::maintain(state, &mut changes);
    if sim {
        crate::end_fight::run(state, &mut intents, &mut changes);
        crate::villager::soldier::attacks(state, &mut intents, &mut changes);
    }
    let entity_tail_time = entity_tail_start.elapsed();

    // 3. Merge (deterministic, fixed order, whole-intent rejection).
    let merge_start = std::time::Instant::now();
    state.merge(&mut intents);
    state.merge_damage.append(&mut state.plugin_damage);
    crate::end_fight::crystal_hits(state);
    let merge_time = merge_start.elapsed();
    let block_prepare_start = std::time::Instant::now();

    // 4. Block phase: ordered merge writes, retained falling lifecycle.
    let mut block_writes = std::mem::take(&mut state.block_phase_writes);
    block_writes.extend_from_slice(&state.merge_block_writes);
    let early_merge_writes = state.merge_block_writes.len();
    let block_prepare_time = block_prepare_start.elapsed();
    let block_start = std::time::Instant::now();
    for (pos, write_state) in &block_writes {
        state.world.set_block(pos.x, pos.y, pos.z, *write_state);
    }
    // Single merge owner: accepted writes win over old chunk time. Catch-up
    // writes are appended only after normal staged writes have been applied.
    crate::abstract_sim::cancel_external_writes(state);
    crate::abstract_sim::run(state, &mut block_writes);
    // Attached blocks whose support went away pop off with their loot.
    let mut popped_drops = Vec::new();
    pop_unsupported(state, &mut block_writes, &mut popped_drops);
    if sim {
        crate::weather::settle(state, &mut block_writes);
        // Random ticks: crops, farmland, saplings, grass.
        crate::farming::run(state, &mut block_writes, &mut popped_drops);
        // Due water/lava ticks, then schedule the fluid around every write.
        crate::fluids::run(state, &mut block_writes, &mut popped_drops);
        // Furnace cooking over live block-entity storage (input/fuel/output).
        tick_furnaces(state, &mut block_writes, None);
        crate::sculk::run(state, &mut block_writes);
        crate::crafter::advance(state, &mut block_writes);
        crate::redstone::run(state, &mut block_writes, &mut popped_drops);
        crate::command_blocks::run(state, &mut changes);
        crate::jukebox::run(state, &mut block_writes, &mut popped_drops);
        crate::brewing::run(state, &mut block_writes, &mut popped_drops);
        crate::crafter::run(state);
    }
    crate::shelf::run(state);
    crate::shulker::run(state);
    // Scheduled ignition and dispenser actions prime after the early merge
    // writes were copied. Their air post-images must commit in this same tick.
    for &(pos, write_state) in &state.merge_block_writes[early_merge_writes..] {
        state.world.set_block(pos.x, pos.y, pos.z, write_state);
        block_writes.push((pos, write_state));
    }
    crate::blocks::run(
        state,
        &mut block_writes,
        &mut popped_drops,
        &mut changes.entities_removed,
    );
    if sim {
        crate::special_mobs::after_blocks(state, &mut block_writes);
        crate::bees::run(state, &mut block_writes, &mut changes);
        crate::armadillo::run(state);
    }
    if sim {
        crate::copper_golem::after_blocks(state, &mut block_writes);
    }
    crate::golems::after_blocks(state, &mut block_writes);
    if sim {
        crate::copper_golem::tick(state, &mut block_writes);
        crate::mob_lanes::snow_trails(state, &mut block_writes);
    }
    crate::portals::nether::after_blocks(state, &mut block_writes);
    crate::sponge::after_blocks(state, &mut block_writes, &mut popped_drops);
    crate::fluids::schedule(state, &block_writes);
    let block_time = block_start.elapsed();

    // 5. Commit: fold blocks, apply entity effects.
    let commit_start = std::time::Instant::now();
    state.world.fold_overlay();
    crate::shrieker::after_blocks(state, &block_writes);
    crate::end_portals::after_blocks(state, &block_writes);
    crate::conduit::after_blocks(state, &block_writes);
    crate::campfire::run(state, &mut popped_drops);
    let commit_write_start = state.merge_block_writes.len();
    #[cfg(test)]
    let effects_start = crate::horde_bench::merge_cost_tests::start();
    let (deaths, mut events, spawned_drops, committed_removals) = state.commit_merge_effects();
    #[cfg(test)]
    crate::horde_bench::merge_cost_tests::record(effects_start, |probe, ns| {
        probe.effects_ns += ns;
    });
    block_writes.extend_from_slice(&state.merge_block_writes[commit_write_start..]);
    if state.merge_block_writes.len() != commit_write_start {
        state.world.fold_overlay();
    }
    crate::shulker::commit_placements(state);
    crate::block_entity_defaults::after_blocks(state, &block_writes);
    crate::trial_spawner::after_blocks(state, &block_writes);
    crate::monster_spawner::after_blocks(state, &block_writes);
    crate::vault::after_blocks(state, &block_writes);
    crate::item_frame::blocks_changed(state, &block_writes, &mut popped_drops);
    crate::painting::blocks_changed(state, &block_writes, &mut popped_drops);
    crate::block_log::after_blocks(state);
    if sim {
        crate::trial_spawner::run(state, &mut block_writes, &mut changes);
        crate::monster_spawner::run(state, &mut changes);
        crate::vault::run(state, &mut block_writes, &mut changes);
    }
    changes.entities_spawned.extend(spawned_drops);
    changes.entities_spawned.extend(popped_drops);
    changes.entities_spawned.append(&mut state.hostile.spawned);
    changes
        .entities_spawned
        .append(&mut state.projectiles.spawned);
    if sim {
        crate::experience_orbs::run(state, &mut changes, &mut events);
    }
    changes.explosions.append(&mut state.hostile.explosions);
    events.append(&mut state.hostile.events);
    changes.entities_removed.append(&mut state.hostile.removed);
    changes
        .entities_removed
        .append(&mut state.projectiles.removed);
    // Committed full-pickup removals journal their tombstones in this same
    // tick (durable pickup: credit + tombstone share one batch).
    changes.entities_removed.extend(committed_removals);
    state.merge_block_writes.clear();
    if sim {
        crate::bees::commit(state, &mut changes);
        crate::villager::conversion::tick(state);
    }
    // Settlements allocator: sole POI authority, immediately before the brain,
    // sharing the global tick budget with its own telemetry phase.
    let allocator_us = if sim {
        let allocator_us = crate::settlements::tick(state, &mut changes);
        crate::villager::brain::tick(state, &mut changes);
        allocator_us
    } else {
        0
    };
    if let Some(log) = phase_log.as_mut() {
        log.allocator_us += allocator_us;
    }
    if sim {
        merchant::maintain(state, &mut changes);
        crate::mounts::tick(state, &mut changes);
        crate::allay::run(state, &mut changes);
        crate::mob_equipment_pickup::run(state, &mut changes);
    }
    crate::entity_life::commit_drops(state, &mut changes.entities_spawned);
    if sim {
        crate::end_fight::commit(state, &mut changes);
        crate::raids::run(state, &mut changes);
    }

    // NOTE: Health(0) + Died emit inside commit_merge_effects (death-screen
    // order, same tick as the drops) — no second emission here; commit is
    // the single owner of the death outbox sequence.
    if crate::command_rules::boolean(state, crate::command_rules::BoolRule::IMMEDIATE_RESPAWN) {
        for entity in deaths {
            if let Some(connection) = state.connections.entity_connection(entity) {
                respawn_player(state, connection, &mut changes);
            }
        }
    }

    state.tick += 1;
    crate::statistics::tick(state, state.tick);
    if sim {
        crate::end_portals::tick(state);
    }
    if crate::tick_control::advances_day(state) {
        state.day_time = state.day_time.wrapping_add(1);
    }
    if !sim {
        // Frozen: the weather clock, sleep and fades hold with the world.
    } else if state.dimension.has_weather() {
        crate::weather::advance_cycle(state);
        crate::sleep::run(state);
        crate::weather::fade_and_publish(state, previous_weather);
    } else {
        state.weather = (crate::commands::Weather::Clear, 0);
        state.rain_level = 0.0;
        state.thunder_level = 0.0;
    }
    // Vanilla `WorldBorder.tick` advances the interpolation extent every
    // game tick, independent of weather sleep or dimension.
    if sim {
        state.border.advance_tick();
    }
    crate::scoreboard::automatic(state, &[], &[]);
    crate::maps::tick(state);
    crate::command_locate::drain(state);
    changes
        .blocks
        .extend(
            block_writes
                .iter()
                .map(|(pos, write_state)| crate::boundary::BlockChange {
                    position: *pos,
                    state: *write_state,
                }),
        );
    block_writes.clear();
    state.block_phase_writes = block_writes;
    // Player inventory is the equipment authority. Project changes through
    // the same appearance path used by all living entities/SetEquipment.
    for (&entity, player) in &state.players {
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        let slots = [
            usize::from(player.inventory.selected.min(8)),
            crate::inventory::OFF_HAND,
            36,
            37,
            38,
            39,
        ];
        let row = state.entities.row_mut(slot);
        let mut changed = false;
        for (index, inventory_slot) in slots.into_iter().enumerate() {
            let stack = &player.inventory.slots[inventory_slot];
            let expected = (!stack.is_empty()).then_some(stack);
            if row.appearance.equipment[index].as_ref() != expected {
                Arc::make_mut(&mut row.appearance).equipment[index] = expected.cloned();
                changed = true;
            }
        }
        if changed {
            state.appearance_changed.push(entity);
        }
    }
    // Effects, commands, sprint changes and joins explicitly invalidate the
    // cache during this tick; publish those without rescanning clean players.
    refresh_player_attributes(state, true);
    changes.entity_events.append(&mut events);
    changes
        .entities_appearance_changed
        .append(&mut state.appearance_changed);

    crate::advancements::run(state, &mut changes);
    changes.entities_spawned.append(&mut state.plugin_spawned);
    let plugin_events = crate::plugin_player::tick(state);
    if let Some(runtime) = &mut state.plugin_runtime {
        runtime.pending_events.extend(plugin_events);
    }
    let commit_time = commit_start.elapsed();
    // 6. Publish: chunk list + entity wire views + per-connection outbox
    // drain. Readers hold the snapshot `Arc`; unchanged chunks share theirs.
    let publish_start = std::time::Instant::now();
    // Native commit-tail owners (including villager work and doors) can write
    // after the main block fold. Publish their actual chunk afterimages, not
    // an older chunk beside newer block deltas.
    // Canonical structure work may run in the resident commit tail, after
    // the early plugin-write drain. Seal those writes with debit/cursor now,
    // using final owner post-images (a door/shape may have evolved meanwhile).
    changes
        .blocks
        .extend(state.plugin_block_writes.drain(..).map(|(position, _)| {
            crate::boundary::BlockChange {
                position,
                state: state
                    .world
                    .block_at(position.x, position.y, position.z)
                    .expect("committed native block remains loaded through tick seal"),
            }
        }));
    state.world.fold_overlay();
    let chunks = state.world.snapshot();
    let snapshot_time = publish_start.elapsed();
    let views_start = std::time::Instant::now();
    let views = state.entities.views();
    let views_time = views_start.elapsed();
    let snapshot = Arc::new(WorldSnapshot {
        tick: state.tick,
        dimension: state.dimension,
        dimensions: Default::default(),
        day_time: state.day_time,
        day_time_rate: if crate::command_rules::boolean(
            state,
            crate::command_rules::BoolRule::ADVANCE_TIME,
        ) {
            1.0
        } else {
            0.0
        },
        raining: state.weather.0 != crate::commands::Weather::Clear,
        rain_level: state.rain_level,
        thunder_level: state.thunder_level,
        border: state.border,
        chunks,
        entities: views,
        raid_bars: crate::raids::bars(state),
        settlement_labels: crate::settlements::presentation::snapshot(state),
    });
    let output = TickOutput {
        tick: state.tick,
        snapshot,
        changes: Arc::new(changes),
        times: TickTimes::default(),
    };
    let mut drains = Vec::new();
    let connections: Vec<ConnectionId> = state.outbox.keys().copied().collect();
    for connection in connections {
        if let Some(queue) = state.outbox.remove(&connection)
            && !queue.is_empty()
        {
            drains.push((connection, queue));
        }
    }
    let publish_tail_time = publish_start
        .elapsed()
        .saturating_sub(snapshot_time)
        .saturating_sub(views_time);
    if let Some(log) = phase_log {
        log.before_return = log.start.elapsed();
        let measured = input_time
            + chunk_time
            + entity_time
            + entity_tail_time
            + merge_time
            + block_prepare_time
            + block_time
            + commit_time
            + snapshot_time
            + views_time
            + publish_tail_time;
        let log_start = std::time::Instant::now();
        eprintln!(
            "phase ms: tick={} input={:.2} chunks={:.2} entity={:.2} merge={:.2} block={:.2} commit={:.2} snapshot={:.2} views={:.2} intents={} dimension={:?} entity_tail={:.2} block_prepare={:.2} publish_tail={:.2} boundary_gaps={:.2} before_return={:.2} allocator={:.2}",
            state.tick,
            input_time.as_secs_f64() * 1000.0,
            chunk_time.as_secs_f64() * 1000.0,
            entity_time.as_secs_f64() * 1000.0,
            merge_time.as_secs_f64() * 1000.0,
            block_time.as_secs_f64() * 1000.0,
            commit_time.as_secs_f64() * 1000.0,
            snapshot_time.as_secs_f64() * 1000.0,
            views_time.as_secs_f64() * 1000.0,
            intents.len(),
            state.dimension,
            entity_tail_time.as_secs_f64() * 1000.0,
            block_prepare_time.as_secs_f64() * 1000.0,
            publish_tail_time.as_secs_f64() * 1000.0,
            log.before_return.saturating_sub(measured).as_secs_f64() * 1000.0,
            log.before_return.as_secs_f64() * 1000.0,
            log.allocator_us as f64 / 1000.0,
        );
        log.logging = log_start.elapsed();
    }
    let times = TickTimes {
        input_us: input_time.as_micros() as u64,
        chunks_us: chunk_time.as_micros() as u64,
        entity_us: (entity_time + entity_tail_time).as_micros() as u64,
        merge_us: merge_time.as_micros() as u64,
        block_us: (block_prepare_time + block_time).as_micros() as u64,
        commit_us: commit_time.as_micros() as u64,
        snapshot_us: snapshot_time.as_micros() as u64,
        views_us: views_time.as_micros() as u64,
        publish_us: publish_tail_time.as_micros() as u64,
        total_us: 0,
        ..Default::default()
    };
    (output, drains, times)
}

/// Apply one [`PlayerEffects`](crate::player::PlayerEffects): reliable
/// changes to the connection, drops as item-spawn intents (merge path).
pub(crate) fn apply_player_effects(
    state: &mut TickState,
    connection: ConnectionId,
    entity: EntityId,
    effects: crate::player::PlayerEffects,
    changes: &mut ChangeSet,
) {
    for change in effects.changes {
        state.push_player(connection, change);
    }
    for (recipe, ingredients) in effects.crafted_recipes {
        let ingredients: Vec<_> = ingredients
            .iter()
            .filter(|item| !item.is_empty())
            .map(|item| crate::advancements::item_snapshot(state, item))
            .collect();
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:recipe_crafted",
            serde_json::json!({"recipe_id":recipe,"ingredients":ingredients}),
        );
    }
    if let Some(item) = &effects.consumed_item {
        let item = crate::advancements::item_snapshot(state, item);
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:consume_item",
            serde_json::json!({"item":item}),
        );
    }
    for potion in effects.brewed_potions {
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:brewed_potion",
            serde_json::json!({"potion":potion}),
        );
    }
    if let Some((item, levels)) = effects.enchanted_item {
        let item = crate::advancements::item_snapshot(state, &item);
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:enchanted_item",
            serde_json::json!({"item":item,"levels":levels}),
        );
    }
    crate::effects::sync_appearance(state, entity);
    if effects.experience_orbs > 0 {
        let pos = state
            .open_containers
            .get(&entity)
            .map(|&(x, y, z)| [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5])
            .unwrap_or_else(|| state.world.world_spawn_position());
        crate::experience_orbs::award(
            state,
            pos,
            effects.experience_orbs,
            &mut changes.entities_spawned,
        );
    }
    if effects.anvil_used {
        stations::damage_anvil(state, entity, changes);
    }
    for (item, count, kind) in effects.crafted {
        crate::plugin::craft_event(state, entity, item, count, kind);
    }
    if effects.dropped.is_empty() {
        return;
    }
    let pos = state
        .entities
        .slot_of(entity)
        .map(|slot| state.entities.pos(slot))
        .unwrap_or_else(|| state.world.world_spawn_position());
    for stack in effects.dropped {
        let id = spawn_item_entity(state, pos, stack);
        changes.entities_spawned.push(id);
    }
}

/// Sneaking with an occupied hand bypasses block-menu activation.
pub(crate) fn secondary_use(state: &TickState, entity: EntityId) -> bool {
    state
        .entities
        .slot_of(entity)
        .is_some_and(|slot| state.entities.row(slot).appearance.shared_flags & SNEAKING_FLAG != 0)
        && state.players.get(&entity).is_some_and(|player| {
            !player.inventory.slots[held_slot_index(player, crate::boundary::Hand::Main)].is_empty()
                || !player.inventory.slots[held_slot_index(player, crate::boundary::Hand::Off)]
                    .is_empty()
        })
}

/// Chunk slot shape -> core item stack.
pub(crate) fn furnace_slot_to_stack(slot: &mc_world::FurnaceSlot) -> mc_data::ItemStack {
    mc_data::ItemStack {
        count: slot.count,
        item_id: slot.item_id,
        damage: slot.damage,
        enchantments: slot.enchantments.clone(),
        custom_name: slot
            .custom_name
            .as_deref()
            .map(str::to_owned)
            .map(String::into_boxed_str),
        item_model: slot.item_model.clone(),
        stew_effects: slot.stew_effects.clone(),
        components: slot.components.clone(),
    }
}

/// Spawn one item entity immediately (input-step effect, not merge: the
/// player already paid the items in `PlayerState`). Returns its entity id.
/// Journaled as an Entity post-image + entities_spawned entry.
pub(crate) fn spawn_item_entity(
    state: &mut TickState,
    pos: [f64; 3],
    stack: mc_data::ItemStack,
) -> EntityId {
    if !state.registry.tags.contains_raw_id(
        "minecraft:item",
        "minecraft:dampens_vibrations",
        stack.item_id as i32,
    ) {
        crate::vibration::emit(state, pos, crate::vibration::GameEvent::EntityPlace, None);
    }
    let id = state.entities.alloc_id();
    let uuid = state.entity_uuid(id);
    let type_id = state.registry.item_type_id;
    let appearance = std::sync::Arc::new(EntityAppearance {
        item: Some(stack.clone()),
        health: 1.0,
        ..Default::default()
    });
    state.entities.spawn(
        id,
        uuid,
        EntityKind::Item { type_id },
        ColdFields {
            pos,
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ground: false,
            health: 1.0,
            breed_cooldown: 0,
            pickup_delay: 10,
            item: Some(crate::entities::StackRow {
                item_id: stack.item_id,
                count: stack.count,
                stack: Some(stack),
            }),
            appearance,
            movement: crate::blueprint::MovementClass::Item,
            half_width: 0.125,
            height: 0.25,
            speed: 0.0,
            attack_damage: 0.0,
            sight_range: 0.0,
            attack_reach: 0.0,
        },
    );
    id
}

/// Shared flags bit of a sneaking entity.
pub(crate) const SNEAKING_FLAG: u8 = 0x02;
/// Vanilla `Pose.CROUCHING` ordinal.
const CROUCHING_POSE: u8 = 5;
