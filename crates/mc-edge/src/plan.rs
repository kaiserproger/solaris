//! Per-viewer tick planning: [`TickOutput`] + [`ViewerState`] -> packets.
//!
//! Retained entity interest consumes publish-wide changes; full held-chunk
//! walks repair window/membership gaps. Canonical dirty work and due/credit
//! debt retain packet order. Exact-baseline movement/head frames are encoded
//! once per publish and shared; bosses use cached candidate ids.
//! Chunk reconciliation follows the same rule: a publish-wide chunk revision
//! delta (`chunk_delta`) lets a viewer that was fully reconciled against the
//! previous publish, has no delivery debt and did not move its window touch
//! only the changed chunks; a skipped publish, moved window, respawn, unsent
//! chunk or overflowing delta falls back to the full held-chunk walk.
//! Chunk traffic is a Chebyshev square, nearest-first (spiral order).
//! Dense admission caches squared distances before partial nearest selection;
//! a full queue builds no spawn list.
//! These are planning-only shortcuts; range, packet credit, movement cadence,
//! per-viewer baselines and the packet encoders remain authoritative.
//!
//! [`plan_player_changes`] (child module `player_changes`) plans one
//! connection's reliable core outbox; the pump calls it when draining the
//! outbox and when replaying a stashed join burst.
//!
//! `Died` maps to a `SystemChat` death message (reliable); the respawn
//! screen itself is driven by the client's own death handling. `Respawned`
//! (same dimension) maps to `ClientboundRespawn` (`data_to_keep = 0`, like
//! vanilla death respawn) followed by a position sync; the viewer's cached
//! chunks/entities are reset so `plan_viewer` resends the world around the
//! new position.

mod player_changes;
pub use player_changes::plan_player_changes;

mod chunk_delta;
mod entity_candidates;
mod entity_interest;
mod entity_type_flags;
mod scratch;
mod shared_moves;
mod tick_index;
use chunk_delta::{ChunkChangeKind, ChunkDelta, ChunkDeltaLog, ChunkSync, chunk_stat};
use scratch::{PlanScratch, ViewWindow};
pub(crate) use tick_index::TickPlanIndex;
use tick_index::{entity_chunk, event_subject};

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

use bytes::BytesMut;
use mc_core::boundary::{
    ConnectionId, EntityAppearance, EntityEvent, EntityId, EntityView, Hand, SnapshotChunk,
    TickOutput, WorldSnapshot,
};
use mc_protocol::frame::{Compression, append_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AGEABLE_ENTITY_DATA_BABY_INDEX, AddEntity, BlockUpdate, ClientboundExplode,
    ClientboundSetEntityData, ClientboundSetEntityEquipment, ClientboundSetTime,
    ClientboundSwingAnimation, ClientboundTakeItemEntity, ENTITY_DATA_POSE_INDEX,
    ENTITY_DATA_SHARED_FLAGS_INDEX, EntityDataValue as WireEntityDataValue, EntityEquipment,
    EntityEvent as WireEntityEvent, EntityPose, EntityPositionSync, EntityRotations, EntityVec3,
    EquipmentSlot, ExplosionBlockParticle, ForgetLevelChunk, ITEM_ENTITY_DATA_ITEM_INDEX,
    LIVING_ENTITY_DATA_EFFECT_AMBIENCE_INDEX, LIVING_ENTITY_DATA_EFFECT_PARTICLES_INDEX,
    LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2, MoveEntityPos, MoveEntityPosRot, MoveEntityRot,
    PLAYER_DATA_ABSORPTION_INDEX, RemoveEntities, RotateHead, SectionBlockChange,
    SectionBlocksUpdate, SetCenterChunk, SetChunkCacheRadius, pack_section_pos,
    pack_section_relative_pos,
};

/// Entity's fourth defineId call uses BOOLEAN (26.1.2 javap Entity static ctor).
const ENTITY_DATA_CUSTOM_NAME_VISIBLE_INDEX: u8 = 3;

/// Cross-viewer framed chunk cache, owned by the pump. A chunk revision
/// includes its authoritative baked light; neighbour light changes are
/// reflected by the core in the affected chunk's own revision. With anti-xray
/// enabled for a dimension, the frame also depends on the four side
/// neighbours (ore enclosure is judged across chunk borders), so their
/// revisions join the key.
#[derive(Debug, Default)]
pub struct ChunkPacketCache {
    entries: HashMap<(mc_core::dimensions::Dimension, (i32, i32)), CachedChunk>,
    anti_xray: Option<Arc<crate::anti_xray::AntiXray>>,
    /// Chunk revision deltas between consecutive publishes of this cache.
    pub(crate) deltas: ChunkDeltaLog,
}

/// What an encoded chunk frame was computed from.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ChunkKey {
    revision: u64,
    /// `-x`, `+x`, `-z`, `+z` neighbour revisions (0 = absent); all 0 when the
    /// frame does not depend on neighbours.
    sides: [u64; 4],
}

#[derive(Debug)]
struct CachedChunk {
    key: ChunkKey,
    /// Final framed (compressed) packet; viewers share it by refcount.
    frame: bytes::Bytes,
}

impl ChunkPacketCache {
    /// Empty cache without anti-xray.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty cache whose chunks are anti-xray processed per `policy`.
    pub fn with_anti_xray(policy: Option<Arc<crate::anti_xray::AntiXray>>) -> Self {
        Self {
            entries: HashMap::default(),
            anti_xray: policy,
            deltas: ChunkDeltaLog::default(),
        }
    }

    /// The anti-xray policy of this cache's chunks, if any dimension has one.
    pub(crate) fn anti_xray(&self) -> Option<&crate::anti_xray::AntiXray> {
        self.anti_xray.as_deref()
    }

    fn key(
        &self,
        dimension: mc_core::dimensions::Dimension,
        chunks: &[SnapshotChunk],
        chunk: &SnapshotChunk,
    ) -> ChunkKey {
        chunk_key(self.anti_xray(), dimension, chunks, chunk)
    }

    fn get(
        &self,
        dimension: mc_core::dimensions::Dimension,
        pos: (i32, i32),
        key: ChunkKey,
    ) -> Option<&bytes::Bytes> {
        let entry = self.entries.get(&(dimension, pos))?;
        (entry.key == key).then_some(&entry.frame)
    }

    /// Drop entries whose chunk is gone or whose content key moved on.
    pub fn retain_resident(&mut self, output: &TickOutput) {
        let policy = self.anti_xray.as_deref();
        self.entries.retain(|(dimension, pos), entry| {
            output
                .in_dimension(*dimension)
                .and_then(|output| {
                    let chunks = &output.snapshot.chunks;
                    find_chunk(chunks, *pos)
                        .map(|chunk| chunk_key(policy, *dimension, chunks, chunk))
                })
                .is_some_and(|key| key == entry.key)
        });
    }
}

/// Content key of `chunk`'s frame: its own revision plus, when anti-xray
/// applies to `dimension`, the revisions of its four side neighbours.
fn chunk_key(
    policy: Option<&crate::anti_xray::AntiXray>,
    dimension: mc_core::dimensions::Dimension,
    chunks: &[SnapshotChunk],
    chunk: &SnapshotChunk,
) -> ChunkKey {
    let mut sides = [0; 4];
    if policy.is_some_and(|policy| policy.enabled(dimension)) {
        let (x, z) = (chunk.pos.x, chunk.pos.z);
        for (side, pos) in sides
            .iter_mut()
            .zip([(x - 1, z), (x + 1, z), (x, z - 1), (x, z + 1)])
        {
            *side = find_chunk(chunks, pos).map_or(0, |resident| resident.revision);
        }
    }
    ChunkKey {
        revision: chunk.revision,
        sides,
    }
}

/// Resident chunk at `pos` (snapshot chunks are sorted by `(x, z)`).
pub(crate) fn find_chunk(chunks: &[SnapshotChunk], pos: (i32, i32)) -> Option<&SnapshotChunk> {
    chunks
        .binary_search_by_key(&pos, |chunk| (chunk.pos.x, chunk.pos.z))
        .ok()
        .map(|i| &chunks[i])
}

/// Encode the chunk packets `plan_viewer` left empty (cache misses): each
/// distinct chunk once, in parallel; cache and fill every placeholder.
/// Returns placeholders whose chunk failed to encode (removed from
/// `planned`), for the caller to mark unsent.
pub fn fill_chunk_packets(
    output: &TickOutput,
    planned: &mut [(ConnectionId, Vec<PlannedPacket>)],
    registries: &PlanRegistries<'_>,
    cache: &mut ChunkPacketCache,
) -> Vec<(ConnectionId, (i32, i32))> {
    use rayon::prelude::*;
    let mut missing: Vec<(mc_core::dimensions::Dimension, (i32, i32))> = planned
        .iter()
        .flat_map(|(_, packets)| packets.iter())
        .filter_map(|packet| match packet.chunk {
            Some(pos) if packet.data.is_empty() => Some((packet.chunk_dimension, pos)),
            _ => None,
        })
        .collect();
    missing.sort_unstable();
    missing.dedup();
    type Encoded = (
        (mc_core::dimensions::Dimension, (i32, i32)),
        ChunkKey,
        Option<bytes::Bytes>,
    );
    let policy = cache.anti_xray.as_deref();
    let encoded: Vec<Encoded> = missing
        .into_par_iter()
        .map(|(dimension, pos)| {
            let Some(output) = output.in_dimension(dimension) else {
                return ((dimension, pos), ChunkKey::default(), None);
            };
            let chunks = &output.snapshot.chunks;
            let key = find_chunk(chunks, pos).map_or_else(ChunkKey::default, |chunk| {
                chunk_key(policy, dimension, chunks, chunk)
            });
            (
                (dimension, pos),
                key,
                encode_chunk(chunks, pos, dimension, registries, policy),
            )
        })
        .collect();
    let mut failed_chunks = HashSet::default();
    for (pos, key, frame) in encoded {
        match frame {
            Some(frame) => {
                cache.entries.insert(pos, CachedChunk { key, frame });
            }
            None => {
                failed_chunks.insert(pos);
            }
        }
    }
    let mut failed = Vec::new();
    for (connection, packets) in planned.iter_mut() {
        packets.retain_mut(|packet| {
            let Some(pos) = packet.chunk else {
                return true;
            };
            if !packet.data.is_empty() {
                return true;
            }
            if failed_chunks.contains(&(packet.chunk_dimension, pos)) {
                failed.push((*connection, pos));
                return false;
            }
            packet.data = cache.entries[&(packet.chunk_dimension, pos)].frame.clone();
            true
        });
    }
    failed
}

/// Encode and frame (compress) one chunk packet, or `None` when the chunk
/// is not resident or does not encode. `policy` hides enclosed ores.
fn encode_chunk(
    chunks: &[SnapshotChunk],
    pos: (i32, i32),
    dimension: mc_core::dimensions::Dimension,
    registries: &PlanRegistries<'_>,
    policy: Option<&crate::anti_xray::AntiXray>,
) -> Option<bytes::Bytes> {
    let centre = find_chunk(chunks, pos)?;
    let hidden = policy.map_or_else(Vec::new, |policy| {
        policy.hide(
            dimension,
            &crate::anti_xray::Surroundings::of(chunks, centre),
        )
    });
    let packet = crate::chunks::encode_chunk_packet_with_sections(
        centre.pos,
        &centre.chunk,
        crate::chunks::EncodeInputs {
            biomes: registries.biomes,
            blocks: registries.blocks,
            items: registries.items,
            block_entity_types: registries.block_entity_types,
        },
        &hidden,
    )
    .ok()?;
    let mut body = Vec::new();
    packet.encode(&mut body).ok()?;
    mc_protocol::frame::encode_frame(
        mc_protocol::packets::play::LevelChunkWithLight::ID,
        &body,
        Compression::Threshold(256),
    )
    .ok()
}

pub struct PlannedPacket {
    pub connection: ConnectionId,
    pub id: i32,
    pub reliable: bool,
    /// Framed wire bytes (length prefix + id + body, threshold-256
    /// compression): the only encoding, routed to the outbound queue as is.
    /// Empty only for a chunk placeholder until [`fill_chunk_packets`].
    pub data: bytes::Bytes,
    /// Chunk position of a `LevelChunkWithLight`; its `data` is empty while
    /// it is a cache-miss placeholder.
    pub chunk: Option<(i32, i32)>,
    pub chunk_dimension: mc_core::dimensions::Dimension,
}

/// Last sent `(position, yaw, pitch, head_yaw)` of an entity: the client's
/// state relative moves are encoded against.
type MoveBaseline = ([f64; 3], f32, f32, f32);

/// Keep the best-effort frames `push` adds only if they fit `credit`
/// (free outbound world-queue slots), so a planned world packet is never
/// dropped and the viewer cache is exactly what the client receives. A
/// group that does not fit is removed and planned again from a newer
/// snapshot.
pub(super) fn try_push(
    out: &mut Vec<PlannedPacket>,
    credit: &mut usize,
    push: impl FnOnce(&mut Vec<PlannedPacket>),
) -> bool {
    if *credit == 0 {
        return false;
    }
    let before = out.len();
    push(out);
    let used = out.len() - before;
    if used > *credit {
        out.truncate(before);
        return false;
    }
    *credit -= used;
    true
}

/// Vanilla world-clock resync period (`MinecraftServer` every 20 ticks).
const TIME_SYNC_INTERVAL_TICKS: u64 = 20;

/// Smallest squared entity move sent to clients (vanilla `ServerEntity`).
const MIN_MOVE_DISTANCE_SQ: f64 = 7.629_394_5e-6;

/// One delivered entity baseline; retaining the appearance Arc makes pointer
/// comparison a sound change detector until the next successful delivery.
#[derive(Debug)]
#[cfg_attr(test, derive(Default))]
pub(crate) struct TrackedEntity {
    pub(crate) type_id: u32,
    baseline: MoveBaseline,
    head_yaw: u8,
    appearance: Arc<EntityAppearance>,
    seen: u64,
}

impl TrackedEntity {
    fn new(entity: &EntityView, seen: u64) -> Self {
        Self {
            type_id: entity.type_id,
            baseline: (entity.position, entity.yaw, entity.pitch, entity.head_yaw),
            head_yaw: pack_degrees(entity.head_yaw),
            appearance: Arc::clone(&entity.appearance),
            seen,
        }
    }
}

/// Per-viewer cached world view. Tracks what this connection already has so
/// planning emits only deltas. Teleport ids are allocated by the core.
#[derive(Debug)]
pub struct ViewerState {
    pub connection: ConnectionId,
    pub entity_id: EntityId,
    pub position: [f64; 3],
    pub dimension: mc_core::dimensions::Dimension,
    /// Negotiated connection radius; None uses the caller's server limit.
    pub(crate) view_distance: Option<i32>,
    /// Registry indices resolved by the edge from its exact vanilla data.
    pub dimension_type_ids: [i32; 3],
    pub advancements: Arc<crate::advancements::AdvancementPackets>,
    /// Chunks the client holds, with the content revision it holds.
    pub loaded_chunks: HashMap<(i32, i32), u64>,
    /// Full repair debt for chunks still held by the client. Kept separately
    /// from initial admission so live edits cannot starve an unseen window.
    chunk_repairs: HashSet<(i32, i32)>,
    /// Public block-entity payloads already routed reliably since the last
    /// snapshot plan; they explain a content revision just like block deltas.
    block_entity_updates: HashSet<(i32, i32)>,
    /// Last delivered baked section layers; shared nibble allocations are COW.
    light_baseline: crate::light::LightBaseline,
    weather_baseline: crate::light::WeatherBaseline,
    boss_baseline: crate::boss_bars::BossBaseline,
    border_baseline: Option<mc_core::command_border::Border>,
    /// Includes the self appearance baseline, but self is never spawned/removed.
    pub(crate) tracked_entities: HashMap<EntityId, TrackedEntity>,
    /// Player-only identity baselines, changed by reliable roster events, not
    /// per-entity tick work. Authentication may replace a provisional UUID.
    pub(crate) player_profiles: HashMap<EntityId, uuid::Uuid>,
    /// Per entity-plan stamp, not snapshot tick: repeated snapshots are normal.
    plan_stamp: u64,
    /// Last evaluated snapshot tick, for reconciling a due phase skipped by the pump.
    entity_tick: Option<u64>,
    entity_interest: entity_interest::EntityInterest,
    /// Positions whose block update is still owed (no credit when the
    /// change happened): batched by section later with the newest state.
    pending_blocks: HashSet<mc_world::chunk::BlockPos>,
    pub center_sent: Option<(i32, i32)>,
    radius_sent: Option<i32>,
    /// The pump skipped outputs since the last plan: their block changes
    /// never reached this viewer, so every chunk whose revision moved on is
    /// sent again.
    pub skipped: bool,
    /// Free outbound world-queue slots, refreshed by the pump before each
    /// publish: the plan emits at most this many best-effort frames.
    pub world_credit: usize,
    /// A same-dimension respawn is pending its forget pass: the vanilla
    /// client keeps its level across it (`ClientPacketListener.handleRespawn`
    /// builds a new `ClientLevel` only on a dimension change), so, like the
    /// vanilla server, everything held is removed/forgotten on the world
    forget_all: bool,
    /// Connection-local settlement label image (virtual negative ids).
    pub(crate) labels: crate::settlement_labels::LabelBaseline,
    /// Presentation-only stair seats keyed by their tracked villager.
    seats: HashMap<EntityId, (i32, mc_core::boundary::BlockPos)>,
    /// Snapshot dimension the label image was built from; a mismatch means
    /// the client's fresh level holds none of these virtual entities.
    pub(crate) label_dimension: Option<mc_core::dimensions::Dimension>,
    /// Last sent `(tick, day_time, rate)`; None forces an authoritative sync.
    clock_baseline: Option<(u64, i64, f32)>,
    scratch: PlanScratch,
    window: ViewWindow,
    /// What the chunk state was last proven against; `None` forces a full walk.
    chunk_sync: Option<ChunkSync>,
    #[cfg(test)]
    pub(crate) chunk_stats: chunk_delta::ChunkPlanStats,
}

impl ViewerState {
    #[must_use]
    pub fn new(connection: ConnectionId, entity_id: EntityId, position: [f64; 3]) -> Self {
        Self {
            connection,
            entity_id,
            position,
            dimension: mc_core::dimensions::Dimension::Overworld,
            view_distance: None,
            dimension_type_ids: [0; 3],
            advancements: Arc::default(),
            loaded_chunks: HashMap::default(),
            chunk_repairs: HashSet::default(),
            block_entity_updates: HashSet::default(),
            light_baseline: crate::light::LightBaseline::default(),
            weather_baseline: crate::light::WeatherBaseline::default(),
            boss_baseline: crate::boss_bars::BossBaseline::default(),
            border_baseline: None,
            tracked_entities: HashMap::default(),
            player_profiles: HashMap::default(),
            plan_stamp: 0,
            entity_tick: None,
            entity_interest: entity_interest::EntityInterest::default(),
            pending_blocks: HashSet::default(),
            center_sent: None,
            radius_sent: None,
            skipped: false,
            world_credit: usize::MAX,
            forget_all: false,
            labels: crate::settlement_labels::LabelBaseline::default(),
            seats: HashMap::default(),
            label_dimension: None,
            clock_baseline: None,
            scratch: PlanScratch::default(),
            window: ViewWindow::default(),
            chunk_sync: None,
            #[cfg(test)]
            chunk_stats: chunk_delta::ChunkPlanStats::default(),
        }
    }

    /// The client never received this chunk's packet: plan it again, and let
    /// the next plan walk the whole window.
    pub(crate) fn chunk_unsent(&mut self, pos: (i32, i32)) {
        self.loaded_chunks.remove(&pos);
        self.chunk_sync = None;
    }
}

/// Remove every held entity and forget every held chunk, within credit.
/// Returns true once the client holds nothing; the cached view is then
/// empty and the next plan sends the whole view (own entity data included).
fn forget_everything(
    viewer: &mut ViewerState,
    out: &mut Vec<PlannedPacket>,
    credit: &mut usize,
    scratch: &mut PlanScratch,
) -> bool {
    if !clear_seats(viewer, out, credit) {
        return false;
    }
    let conn = viewer.connection;
    scratch.removed.clear();
    scratch.removed.extend(
        viewer
            .tracked_entities
            .keys()
            .filter(|id| **id != viewer.entity_id)
            .map(|id| id.0),
    );
    scratch.removed.sort_unstable();
    if !scratch.removed.is_empty() {
        let packet = RemoveEntities {
            entity_ids: std::mem::take(&mut scratch.removed),
        };
        let sent = try_push(out, credit, |out| push_best_effort(out, conn, &packet));
        scratch.removed = packet.entity_ids;
        if !sent {
            return false;
        }
    }
    viewer.tracked_entities.clear();
    viewer.entity_tick = None;
    viewer.entity_interest = entity_interest::EntityInterest::default();
    scratch.chunks.clear();
    scratch.chunks.extend(viewer.loaded_chunks.keys().copied());
    for &chunk in &scratch.chunks {
        if !try_push(out, credit, |out| {
            push_best_effort(
                out,
                conn,
                &ForgetLevelChunk {
                    chunk_x: chunk.0,
                    chunk_z: chunk.1,
                },
            );
        }) {
            return false;
        }
        viewer.loaded_chunks.remove(&chunk);
        viewer.chunk_repairs.remove(&chunk);
        viewer.light_baseline.forget(chunk);
    }
    viewer.pending_blocks.clear();
    viewer.block_entity_updates.clear();
    viewer.center_sent = None;
    viewer.forget_all = false;
    crate::settlement_labels::forget_labels(viewer, out);
    true
}

/// Plan-time chunk encoding inputs (biomes/blocks/items/block-entity
/// types); chunk encode failures skip that chunk rather than failing the
/// tick.
pub struct PlanRegistries<'a> {
    pub biomes: &'a mc_data::Registry,
    pub blocks: &'a mc_world::BlockRegistry,
    pub items: &'a mc_data::items::ItemRegistry,
    pub block_entity_types: &'a mc_data::block_entity_types::BlockEntityTypeRegistry,
}

/// Chunk packets per viewer per publish, nearest-first (vanilla
/// `PlayerChunkSender` paces too). An unpaced first publish encoded a whole
/// view square (289 chunks at view 8: ~11 s in a debug build) on the pump,
/// stalling every viewer.
const CHUNKS_PER_TICK: usize = 16;

/// Plan every world packet for one viewer from one tick output;
/// `view_distance` sizes the Chebyshev chunk square. Player changes ride
/// the core outbox ([`plan_player_changes`]); weather also reconciles against
/// the authoritative snapshot so skipped projections cannot strand a fade.
///
/// Best-effort frames are bounded by [`ViewerState::world_credit`], in
/// priority order: one-shot explosions, chunk forgets, block updates, light
/// updates, chunks (nearest first), entity removals, spawns (nearest first),
/// appearance/moves (rotating), visual events. Explosions have no snapshot
/// image to reconcile, so they precede even pending respawn forgets. Other
/// work that does not fit stays unsent in the viewer
/// cache and is planned again from a newer snapshot.
pub fn plan_viewer(
    output: &TickOutput,
    viewer: &mut ViewerState,
    view_distance: i32,
    chunk_cache: &ChunkPacketCache,
) -> Vec<PlannedPacket> {
    let Some(output) = output.in_dimension(viewer.dimension) else {
        return Vec::new();
    };
    let index = TickPlanIndex::with_previous(output, viewer.entity_interest.previous())
        .with_reveals(output, chunk_cache.anti_xray());
    plan_viewer_indexed(output, viewer, view_distance, chunk_cache, &index)
}

pub(crate) fn plan_viewer_indexed(
    output: &TickOutput,
    viewer: &mut ViewerState,
    view_distance: i32,
    chunk_cache: &ChunkPacketCache,
    index: &TickPlanIndex,
) -> Vec<PlannedPacket> {
    let view_distance = viewer
        .view_distance
        .unwrap_or(view_distance)
        .min(view_distance);
    let mut scratch = std::mem::take(&mut viewer.scratch);
    let packets = plan_viewer_inner(
        output,
        viewer,
        view_distance,
        chunk_cache,
        index,
        &mut scratch,
    );
    viewer.scratch = scratch;
    packets
}

impl ViewerState {
    pub(crate) fn previous_entities(&self) -> Option<&mc_core::boundary::EntityViews> {
        self.entity_interest.previous()
    }
}

fn plan_viewer_inner(
    output: &TickOutput,
    viewer: &mut ViewerState,
    view_distance: i32,
    chunk_cache: &ChunkPacketCache,
    index: &TickPlanIndex,
    scratch: &mut PlanScratch,
) -> Vec<PlannedPacket> {
    let mut out = Vec::new();
    let snapshot = &output.snapshot;
    let changes = &output.changes;
    let conn = viewer.connection;
    let mut credit = viewer.world_credit;
    // Resolve the authoritative anchor before range-checking one-shot effects.
    if let Some(own) = own_entity(snapshot, viewer) {
        viewer.position = own.position;
    }
    // These events have no later snapshot image to reconcile. In particular,
    // dropping one loses its recipient's knockback, not just a visual effect.
    index.explosions_near(viewer.position, &mut scratch.events);
    for &event in &scratch.events {
        let explosion = &changes.explosions[event];
        let [x, y, z] = viewer.position;
        let [cx, cy, cz] = explosion.center;
        if (x - cx).powi(2) + (y - cy).powi(2) + (z - cz).powi(2) <= EXPLOSION_RANGE_SQ
            && !try_push(&mut out, &mut credit, |out| {
                push_best_effort(out, conn, &explode_packet(explosion, viewer.entity_id));
            })
        {
            break;
        }
    }
    // Heartbeats and discontinuities share the reliable FIFO: an older queued
    // heartbeat must never overwrite a time command, rate change or sleep skip.
    // Sync before chunk work, including a credit-blocked respawn forget pass.
    if viewer.clock_baseline.is_none_or(|(at, day_time, rate)| {
        let elapsed = snapshot.tick.saturating_sub(at);
        elapsed >= TIME_SYNC_INTERVAL_TICKS
            || snapshot.day_time_rate != rate
            || snapshot.day_time
                != day_time.saturating_add((elapsed as f64 * f64::from(rate)) as i64)
    }) {
        push_reliable(
            &mut out,
            conn,
            &ClientboundSetTime::overworld(
                i64::try_from(snapshot.tick).unwrap_or(i64::MAX),
                snapshot.day_time,
                snapshot.day_time_rate,
            ),
        );
        viewer.clock_baseline = Some((snapshot.tick, snapshot.day_time, snapshot.day_time_rate));
    }
    #[cfg(test)]
    let mut stats = chunk_delta::ChunkPlanStats::default();
    // Cleared until this plan completes: returning early leaves the next
    // plan a full walk.
    let sync = viewer.chunk_sync.take();
    let delta = index.chunk_delta();
    // A pending forget empties the held set and a skip drops block changes:
    // neither is described by the delta.
    let followed = !viewer.forget_all
        && !viewer.skipped
        && delta.zip(sync).is_some_and(|(delta, sync)| {
            sync.quiet
                && sync.held == viewer.loaded_chunks.len()
                && delta.follows(&sync, snapshot.dimension)
        });
    if !followed {
        chunk_stat!(stats.visited += viewer.light_baseline.len());
        chunk_stat!(stats.probes += viewer.light_baseline.len());
        viewer.light_baseline.retain_held(&viewer.loaded_chunks);
    }
    if viewer.forget_all && !forget_everything(viewer, &mut out, &mut credit, scratch) {
        return out;
    }
    viewer.weather_baseline.snapshot(
        conn,
        snapshot.raining,
        snapshot.rain_level,
        snapshot.thunder_level,
        &mut out,
    );
    if viewer.border_baseline != Some(snapshot.border)
        && try_push(&mut out, &mut credit, |out| {
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundInitializeBorder {
                    center_x: snapshot.border.center_x,
                    center_z: snapshot.border.center_z,
                    old_size: snapshot.border.size,
                    new_size: snapshot.border.lerp_target,
                    lerp_time: snapshot.border.lerp_remaining,
                    absolute_max_size: 29_999_984,
                    warning_blocks: snapshot.border.warning_blocks,
                    // Protocol warning time is seconds; vanilla
                    // ClientboundInitializeBorderPacket carries
                    // getWarningTime (seconds) alongside getLerpTime.
                    warning_time: snapshot.border.warning_time,
                },
            );
        })
    {
        viewer.border_baseline = Some(snapshot.border);
    }
    let center = entity_chunk(viewer.position);

    // Center + radius (reliable: the client discards mis-anchored chunks).
    if viewer.center_sent != Some(center) {
        push_reliable(
            &mut out,
            conn,
            &SetCenterChunk {
                chunk_x: center.0,
                chunk_z: center.1,
            },
        );
        viewer.center_sent = Some(center);
        viewer.radius_sent = None;
    }
    if viewer.radius_sent != Some(view_distance.max(0)) {
        push_reliable(
            &mut out,
            conn,
            &SetChunkCacheRadius {
                radius: view_distance.max(0),
            },
        );
        viewer.radius_sent = Some(view_distance.max(0));
    }

    // Chunk unloads: a chunk left without credit stays held and is
    // forgotten by a later plan.
    let window_moved = viewer.window.update(center, view_distance);
    // The window is unmoved and the viewer held nothing outside it, so only
    // the chunks the publish delta names can have moved on.
    let fast = followed && !window_moved;
    let mut unload_debt = false;
    let mut light_debt = false;
    let mut admission_debt = false;
    let mut admit = !fast;
    chunk_stat!(stats.plans += 1);
    chunk_stat!(stats.delta_plans += u64::from(fast));
    chunk_stat!(stats.full_walks += u64::from(!fast));
    if !fast {
        chunk_stat!(stats.visited += viewer.loaded_chunks.len());
        chunk_stat!(stats.probes += viewer.loaded_chunks.len());
        scratch.chunks.clear();
        scratch.chunks.extend(
            viewer
                .loaded_chunks
                .keys()
                .filter(|pos| !viewer.window.want.contains(pos))
                .copied(),
        );
        for &gone in &scratch.chunks {
            let forgotten = try_push(&mut out, &mut credit, |out| {
                push_best_effort(
                    out,
                    conn,
                    &ForgetLevelChunk {
                        chunk_x: gone.0,
                        chunk_z: gone.1,
                    },
                );
            });
            if forgotten {
                viewer.loaded_chunks.remove(&gone);
                viewer.chunk_repairs.remove(&gone);
                viewer.block_entity_updates.remove(&gone);
                viewer.light_baseline.forget(gone);
            } else {
                unload_debt = true;
            }
        }
    }
    // Reliable public block-entity updates and non-skipped block changes
    // explain content revisions without full resends. A changed baked light
    // baseline covers light-only revisions; skips still require repairs.
    let skipped = std::mem::take(&mut viewer.skipped);
    scratch.touched.clear();
    scratch.touched.extend(viewer.block_entity_updates.drain());
    if let Some(delta) = delta.filter(|_| fast) {
        // Walk the shared change lists, not this viewer's held chunks.
        for (chunk, changes_in_chunk) in &index.blocks {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 1);
            if viewer.loaded_chunks.contains_key(chunk) {
                for &change in changes_in_chunk {
                    viewer
                        .pending_blocks
                        .insert(changes.blocks[change].position);
                }
            }
        }
        for (chunk, revealed) in &index.reveals {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 1);
            if viewer.loaded_chunks.contains_key(chunk) {
                viewer.pending_blocks.extend(revealed.iter().copied());
            }
        }
        scratch.delta_chunks.clear();
        for change in delta.changes() {
            chunk_stat!(stats.visited += 1);
            if change.kind == ChunkChangeKind::Unloaded {
                continue;
            }
            chunk_stat!(stats.probes += 1);
            if !viewer.window.want.contains(&change.pos) {
                continue;
            }
            chunk_stat!(stats.probes += 1);
            if viewer.loaded_chunks.contains_key(&change.pos) {
                scratch.delta_chunks.push(change.pos);
            } else {
                // Newly resident inside the window: admission owns it.
                admit = true;
            }
        }
        for &pos in &scratch.delta_chunks {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 2);
            let Some(held) = viewer.loaded_chunks.get_mut(&pos) else {
                continue;
            };
            let Some(current) = find_chunk(&snapshot.chunks, pos) else {
                continue;
            };
            if *held == current.revision {
                continue;
            }
            chunk_stat!(stats.probes += 2);
            let light_changed = viewer.light_baseline.observe(pos, &current.chunk);
            if !skipped
                && (scratch.touched.contains(&pos)
                    || index.blocks.contains_key(&pos)
                    || light_changed)
            {
                *held = current.revision;
            } else {
                viewer.chunk_repairs.insert(pos);
            }
        }
        admit |= !viewer.chunk_repairs.is_empty();
    } else {
        for chunk in viewer.loaded_chunks.keys() {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 2);
            if let Some(changes_in_chunk) = index.blocks.get(chunk) {
                scratch.touched.insert(*chunk);
                for &change in changes_in_chunk {
                    viewer
                        .pending_blocks
                        .insert(changes.blocks[change].position);
                }
            }
            // Ores a block change next to them just exposed (anti-xray sent the
            // rock stand-in): the change may sit in a neighbouring chunk, so the
            // shared index is keyed by the ore's own chunk.
            if let Some(revealed) = index.reveals.get(chunk) {
                viewer.pending_blocks.extend(revealed.iter().copied());
            }
        }
        for (pos, held) in &mut viewer.loaded_chunks {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 1);
            if !viewer.window.want.contains(pos) {
                continue;
            }
            chunk_stat!(stats.probes += 1);
            let Some(current) = find_chunk(&snapshot.chunks, *pos) else {
                continue;
            };
            if *held == current.revision {
                continue;
            }
            chunk_stat!(stats.probes += 1);
            if viewer.chunk_repairs.contains(pos) {
                continue;
            }
            chunk_stat!(stats.probes += 2);
            let light_changed = viewer.light_baseline.observe(*pos, &current.chunk);
            if !skipped && (scratch.touched.contains(pos) || light_changed) {
                *held = current.revision;
            } else {
                viewer.chunk_repairs.insert(*pos);
            }
        }
        chunk_stat!(stats.visited += viewer.light_baseline.len());
        chunk_stat!(stats.probes += viewer.light_baseline.len());
        viewer.light_baseline.retain_held(&viewer.loaded_chunks);
    }
    // Owed block updates use the newest snapshot state. Like vanilla, one
    // block uses BlockUpdate; multiple blocks in a section share one packet
    // and one queue credit. A full repair already covers its chunk's debt.
    let section_of = |position: &mc_world::chunk::BlockPos| {
        (
            position.x.div_euclid(16),
            position.y.div_euclid(16),
            position.z.div_euclid(16),
        )
    };
    scratch.blocks.clear();
    scratch.blocks.extend(viewer.pending_blocks.drain());
    scratch.blocks.sort_unstable_by_key(|position| {
        (section_of(position), position.x, position.y, position.z)
    });
    let mut update = SectionBlocksUpdate {
        section_pos: 0,
        changes: std::mem::take(&mut scratch.section_blocks),
    };
    let mut start = 0;
    while start < scratch.blocks.len() {
        let section = section_of(&scratch.blocks[start]);
        let mut end = start + 1;
        while end < scratch.blocks.len() && section_of(&scratch.blocks[end]) == section {
            end += 1;
        }
        let positions = &scratch.blocks[start..end];
        start = end;
        chunk_stat!(stats.visited += positions.len() as u64);
        chunk_stat!(stats.probes += 3);
        let chunk = (section.0, section.2);
        if !viewer.loaded_chunks.contains_key(&chunk) || viewer.chunk_repairs.contains(&chunk) {
            continue;
        }
        if credit == 0 {
            viewer.pending_blocks.extend(positions.iter().copied());
            continue;
        }
        let Some(resident) = find_chunk(&snapshot.chunks, chunk) else {
            continue;
        };
        update.section_pos = pack_section_pos(section.0, section.1, section.2);
        update.changes.clear();
        for position in positions {
            if let Some(state) = resident.chunk.get_block(
                position.x.rem_euclid(16) as u8,
                position.y,
                position.z.rem_euclid(16) as u8,
            ) {
                update.changes.push(SectionBlockChange {
                    relative_pos: pack_section_relative_pos(position.x, position.y, position.z),
                    state_id: state.0 as i32,
                });
            }
        }
        if update.changes.is_empty() {
            continue;
        }
        if !try_push(&mut out, &mut credit, |out| {
            if positions.len() == 1 {
                push_block_update(out, conn, positions[0], update.changes[0].state_id as u32);
            } else {
                push_best_effort(out, conn, &update);
            }
        }) {
            viewer.pending_blocks.extend(positions.iter().copied());
        }
    }
    scratch.section_blocks = update.changes;
    // A full repair covers its own newest baked light. Other light debt
    // survives exhausted credit without replacing a chunk admission. Light
    // moves only with a chunk revision, so the delta names every candidate.
    if fast {
        scratch
            .delta_chunks
            .sort_unstable_by_key(|&(x, z)| ((x - center.0).abs().max((z - center.1).abs()), x, z));
        for &pos in &scratch.delta_chunks {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 3);
            if !viewer.chunk_repairs.contains(&pos)
                && let Some(resident) = find_chunk(&snapshot.chunks, pos)
            {
                light_debt |= !viewer.light_baseline.update(
                    pos,
                    &resident.chunk,
                    resident.revision,
                    conn,
                    &mut out,
                    &mut credit,
                );
            }
        }
    } else {
        for &pos in &viewer.window.order {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 1);
            if !viewer.loaded_chunks.contains_key(&pos) {
                continue;
            }
            chunk_stat!(stats.probes += 1);
            if viewer.chunk_repairs.contains(&pos) {
                continue;
            }
            chunk_stat!(stats.probes += 2);
            if let Some(resident) = find_chunk(&snapshot.chunks, pos) {
                light_debt |= !viewer.light_baseline.update(
                    pos,
                    &resident.chunk,
                    resident.revision,
                    conn,
                    &mut out,
                    &mut credit,
                );
            }
        }
    }
    // Labels refer only to chunks held before this plan, never to an unseen
    // chunk admitted below whose packet has not reached the client yet.
    crate::settlement_labels::plan_labels(snapshot, viewer, view_distance, &mut out, &mut credit);

    // Admit unseen chunks first, then repair held chunks. Both share the same
    // bounded send budget; a deferred repair remains owed across publishes.
    let mut chunks_left = CHUNKS_PER_TICK;
    for repairs in [false, true] {
        if !admit {
            break;
        }
        for &pos in &viewer.window.order {
            chunk_stat!(stats.visited += 1);
            chunk_stat!(stats.probes += 1);
            if if repairs {
                !viewer.chunk_repairs.contains(&pos)
            } else {
                viewer.loaded_chunks.contains_key(&pos)
            } {
                continue;
            }
            if chunks_left == 0 || credit == 0 {
                admission_debt = true;
                break;
            }
            chunks_left -= 1;
            chunk_stat!(stats.probes += 1);
            let Some(resident) = find_chunk(&snapshot.chunks, pos) else {
                continue;
            };
            let data = chunk_cache
                .get(
                    snapshot.dimension,
                    pos,
                    chunk_cache.key(snapshot.dimension, &snapshot.chunks, resident),
                )
                .cloned()
                .unwrap_or_default();
            out.push(PlannedPacket {
                connection: conn,
                id: mc_protocol::packets::play::LevelChunkWithLight::ID,
                reliable: false,
                data,
                chunk: Some(pos),
                chunk_dimension: snapshot.dimension,
            });
            credit -= 1;
            viewer.loaded_chunks.insert(pos, resident.revision);
            viewer.chunk_repairs.remove(&pos);
            viewer
                .light_baseline
                .record(pos, &resident.chunk, resident.revision);
        }
    }
    if let Some(delta) = delta {
        let quiet = !unload_debt
            && !light_debt
            && !admission_debt
            && viewer.chunk_repairs.is_empty()
            && viewer.pending_blocks.is_empty();
        viewer.chunk_sync = Some(delta.sync(snapshot.dimension, viewer.loaded_chunks.len(), quiet));
    }
    #[cfg(test)]
    viewer.chunk_stats.add(stats);

    // One-shot visuals of entities this client has; lost without credit.
    // Before entity removals: vanilla sends the pickup (and death) event
    // while the client still holds the entity, then removes it.
    index.events_for(
        &viewer.window.want,
        viewer
            .tracked_entities
            .keys()
            .copied()
            .chain(std::iter::once(viewer.entity_id)),
        &mut scratch.events,
    );
    for &event in &scratch.events {
        let event = &changes.entity_events[event];
        let subject = event_subject(*event);
        if subject != viewer.entity_id && !viewer.tracked_entities.contains_key(&subject) {
            continue;
        }
        if !try_push(&mut out, &mut credit, |out| {
            push_entity_event(out, conn, *event);
        }) {
            break;
        }
    }

    // Entities: remove / spawn / appearance / move, via the chunk index.
    plan_entities(
        snapshot,
        viewer,
        view_distance,
        scratch,
        index,
        &mut out,
        &mut credit,
    );
    plan_seats(viewer, &mut out, &mut credit);
    out
}

/// Direct identity lookup, including teleports and not-yet-admitted chunks.
fn own_entity<'a>(snapshot: &'a WorldSnapshot, viewer: &ViewerState) -> Option<&'a EntityView> {
    snapshot.entities.get(viewer.entity_id)
}

fn push_block_update(
    out: &mut Vec<PlannedPacket>,
    connection: ConnectionId,
    position: mc_world::chunk::BlockPos,
    state: u32,
) {
    push_best_effort(
        out,
        connection,
        &BlockUpdate {
            position: mc_protocol::packets::play::pack_block_pos(
                position.x, position.y, position.z,
            ),
            state_id: state as i32,
        },
    );
}

/// Whether the client lacks `entity`'s current appearance.
fn appearance_stale(viewer: &ViewerState, entity: &EntityView) -> bool {
    viewer
        .tracked_entities
        .get(&entity.id)
        .is_none_or(|sent| !Arc::ptr_eq(&sent.appearance, &entity.appearance))
}

/// Vanilla tracking uses an inclusive horizontal square, not a 3-D sphere.
fn entity_trackable(entity: &EntityView, viewer: &ViewerState, view_distance: i32) -> bool {
    if entity.id == viewer.entity_id {
        return true;
    }
    let Some(contract) =
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(entity.type_id)
    else {
        return false;
    };
    let range = f64::from(i32::from(contract.tracking_range).min(view_distance.max(0)) * 16);
    (entity.position[0] - viewer.position[0]).abs() <= range
        && (entity.position[2] - viewer.position[2]).abs() <= range
}

fn entity_movement_due(entity: &EntityView, tick: u64, previous_tick: Option<u64>) -> bool {
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(entity.type_id).is_some_and(
        |contract| {
            let interval = u64::from(contract.update_interval);
            let phase =
                i64::from(entity.id.0).rem_euclid(i64::from(contract.update_interval)) as u64;
            let position = (tick % interval + phase) % interval;
            let elapsed = previous_tick.map_or(0, |previous| tick.saturating_sub(previous));
            position == 0 || position < elapsed
        },
    )
}

fn clear_seats(viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>, credit: &mut usize) -> bool {
    while let Some((&rider, &(seat, _))) = viewer.seats.iter().next() {
        if !try_push(out, credit, |out| {
            push_best_effort(
                out,
                viewer.connection,
                &mc_protocol::packets::play::ClientboundSetPassengers {
                    vehicle_id: seat,
                    passenger_ids: Vec::new(),
                },
            );
            push_best_effort(
                out,
                viewer.connection,
                &RemoveEntities {
                    entity_ids: vec![seat],
                },
            );
        }) {
            return false;
        }
        viewer.seats.remove(&rider);
    }
    true
}

fn plan_seats(viewer: &mut ViewerState, out: &mut Vec<PlannedPacket>, credit: &mut usize) {
    loop {
        let removed = viewer.seats.iter().find_map(|(&rider, &(id, pos))| {
            (viewer
                .tracked_entities
                .get(&rider)
                .and_then(|tracked| tracked.appearance.villager_seat)
                != Some(pos))
            .then_some((rider, id))
        });
        let Some((rider, id)) = removed else { break };
        if !try_push(out, credit, |out| {
            push_best_effort(
                out,
                viewer.connection,
                &mc_protocol::packets::play::ClientboundSetPassengers {
                    vehicle_id: id,
                    passenger_ids: Vec::new(),
                },
            );
            push_best_effort(
                out,
                viewer.connection,
                &RemoveEntities {
                    entity_ids: vec![id],
                },
            );
        }) {
            return;
        }
        viewer.seats.remove(&rider);
    }
    for (&rider, tracked) in &viewer.tracked_entities {
        let Some(pos) = tracked.appearance.villager_seat else {
            continue;
        };
        if viewer.seats.contains_key(&rider) {
            continue;
        }
        if *credit < 3 {
            return;
        }
        let Some(id) = viewer.labels.alloc() else {
            return;
        };
        let stand_type =
            mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:armor_stand")
                .expect("armor stand entity contract")
                .protocol_id as i32;
        if !try_push(out, credit, |out| {
            push_best_effort(
                out,
                viewer.connection,
                &AddEntity {
                    entity_id: id,
                    uuid: uuid::Uuid::from_u128((u128::from(id as u32) << 64) | 0x73656174),
                    entity_type_id: stand_type,
                    x: f64::from(pos.x) + 0.5,
                    y: f64::from(pos.y) + 0.5,
                    z: f64::from(pos.z) + 0.5,
                    movement: EntityVec3::ZERO,
                    pitch: 0.0,
                    yaw: 0.0,
                    head_yaw: 0.0,
                    data: 0,
                },
            );
            push_best_effort(
                out,
                viewer.connection,
                &ClientboundSetEntityData {
                    entity_id: id,
                    values: vec![
                        WireEntityDataValue::Byte {
                            index: ENTITY_DATA_SHARED_FLAGS_INDEX,
                            value: 0x20,
                        },
                        WireEntityDataValue::Byte {
                            index: 15,
                            value: 0x10,
                        },
                    ],
                },
            );
            push_best_effort(
                out,
                viewer.connection,
                &mc_protocol::packets::play::ClientboundSetPassengers {
                    vehicle_id: id,
                    passenger_ids: vec![rider.0],
                },
            );
        }) {
            return;
        }
        viewer.seats.insert(rider, (id, pos));
    }
}

fn plan_entities(
    snapshot: &WorldSnapshot,
    viewer: &mut ViewerState,
    view_distance: i32,
    scratch: &mut PlanScratch,
    index: &TickPlanIndex,
    out: &mut Vec<PlannedPacket>,
    credit: &mut usize,
) {
    let conn = viewer.connection;
    let previous_tick = viewer.entity_tick.replace(snapshot.tick);
    let mut interest = std::mem::take(&mut viewer.entity_interest);
    interest.prepare(snapshot, viewer, view_distance, index, scratch, *credit);
    viewer.boss_baseline.sync(
        conn,
        scratch
            .bosses
            .iter()
            .map(|&id| snapshot.entities.get(id).expect("visible boss")),
        out,
    );
    viewer
        .boss_baseline
        .sync_raids(conn, viewer.position, &snapshot.raid_bars, out);

    if !scratch.removed.is_empty() && *credit > 0 {
        let packet = RemoveEntities {
            entity_ids: std::mem::take(&mut scratch.removed),
        };
        if try_push(out, credit, |out| push_best_effort(out, conn, &packet)) {
            for &id in &packet.entity_ids {
                let id = EntityId(id);
                viewer.tracked_entities.remove(&id);
                interest.removed(id);
            }
        }
        scratch.removed = packet.entity_ids;
    }

    // The client's own entity gets no AddEntity but still needs its
    // SetEntityData (pose/flags/health), or the client player object never
    // resolves; resent whenever its appearance changes.
    if let Some(own) = own_entity(snapshot, viewer)
        && appearance_stale(viewer, own)
        && try_push(out, credit, |out| {
            // Vanilla sends equipment only to trackers, never to the owner:
            // the client writes MAINHAND into its *currently* selected slot.
            push_appearance_with(
                out,
                conn,
                own.id,
                &own.appearance,
                own.type_id,
                viewer
                    .tracked_entities
                    .get(&own.id)
                    .map(|sent| sent.appearance.as_ref()),
                false,
            )
        })
    {
        viewer
            .tracked_entities
            .insert(own.id, TrackedEntity::new(own, viewer.plan_stamp));
    }
    interest.spawned(viewer.entity_id);

    // Spawns, nearest first, as many as the credit carries.
    let nearest = |a: &(EntityId, f64), b: &(EntityId, f64)| a.1.total_cmp(&b.1);
    if *credit == 0 {
        scratch.spawns.clear();
    } else if scratch.spawns.len() > *credit {
        scratch.spawns.select_nth_unstable_by(*credit, nearest);
        scratch.spawns.truncate(*credit);
    }
    scratch.spawns.sort_unstable_by(nearest);
    for &(id, _) in &scratch.spawns {
        let entity = snapshot.entities.get(id).expect("live spawn");
        let position = entity_spawn_position(entity);
        let spawned = try_push(out, credit, |out| {
            push_best_effort(
                out,
                conn,
                &AddEntity {
                    entity_id: entity.id.0,
                    uuid: entity.uuid,
                    entity_type_id: entity.type_id as i32,
                    x: position[0],
                    y: position[1],
                    z: position[2],
                    movement: per_tick(entity.velocity),
                    pitch: entity.pitch,
                    yaw: entity.yaw,
                    head_yaw: entity.head_yaw,
                    data: entity.appearance.spawn_data,
                },
            );
            push_appearance(out, conn, entity, None);
        });
        if !spawned {
            break;
        }
        viewer
            .tracked_entities
            .insert(entity.id, TrackedEntity::new(entity, viewer.plan_stamp));
        interest.spawned(entity.id);
    }

    // Visit changed rows and retained delivery/cadence debt in canonical
    // chunk/id order, resuming at the first credit-blocked entity.
    let count = scratch.visible.len();
    let start = interest.start(&scratch.visible, snapshot);
    for offset in (start..count).chain(0..start) {
        let id = scratch.visible[offset];
        if id == viewer.entity_id {
            continue;
        }
        let Some(tracked) = viewer.tracked_entities.get_mut(&id) else {
            continue;
        };
        let entity = snapshot.entities.get(id).expect("visible entity");
        let stale = !Arc::ptr_eq(&tracked.appearance, &entity.appearance);
        let due = interest.ready(id) || entity_movement_due(entity, snapshot.tick, previous_tick);
        let movement = if due {
            index.moves.movement(entity, tracked.baseline)
        } else {
            None
        };
        let packed_head = if due {
            pack_degrees(entity.head_yaw)
        } else {
            0
        };
        let turned = due && tracked.head_yaw != packed_head;
        if !stale && movement.is_none() && !turned {
            interest.completed(entity, tracked, snapshot.tick, due);
            continue;
        }
        let sent = try_push(out, credit, |out| {
            if stale {
                push_appearance(out, conn, entity, Some(tracked.appearance.as_ref()));
            }
            if let Some(packet) = &movement {
                out.push(PlannedPacket {
                    connection: conn,
                    id: packet.id,
                    reliable: false,
                    data: packet.data.clone(),
                    chunk: None,
                    chunk_dimension: viewer.dimension,
                });
            }
            if turned {
                let (id, data) = index.moves.head(entity);
                out.push(PlannedPacket {
                    connection: conn,
                    id,
                    reliable: false,
                    data,
                    chunk: None,
                    chunk_dimension: viewer.dimension,
                });
            }
        });
        if !sent {
            interest.blocked(entity);
            viewer.entity_interest = interest;
            return;
        }
        if stale {
            tracked.appearance = Arc::clone(&entity.appearance);
        }
        if let Some(packet) = movement {
            tracked.baseline = packet.next;
        }
        if turned {
            tracked.head_yaw = packed_head;
        }
        interest.completed(entity, tracked, snapshot.tick, due);
    }
    viewer.entity_interest = interest;
}

/// Local 26.1.2 javap: leash knots and item frames publish their BlockPos,
/// not the offset internal rendering position.
fn entity_spawn_position(entity: &EntityView) -> [f64; 3] {
    if entity_type_flags::is_leash_knot(entity.type_id)
        || entity_type_flags::is_item_frame(entity.type_id)
    {
        entity.position.map(f64::floor)
    } else {
        entity.position
    }
}

/// One entity movement packet against the client's baseline.
enum MovePacket {
    Pos(MoveEntityPos),
    Rot(MoveEntityRot),
    PosRot(MoveEntityPosRot),
    Sync(EntityPositionSync),
}

#[cfg(test)]
impl MovePacket {
    fn push(&self, out: &mut Vec<PlannedPacket>, conn: ConnectionId) {
        match self {
            Self::Pos(packet) => push_best_effort(out, conn, packet),
            Self::Rot(packet) => push_best_effort(out, conn, packet),
            Self::PosRot(packet) => push_best_effort(out, conn, packet),
            Self::Sync(packet) => push_best_effort(out, conn, packet),
        }
    }
}

/// The move `entity` needs relative to `baseline`, with the baseline the
/// client holds once it is delivered; `None` below vanilla `ServerEntity`
/// thresholds (a move under ~0.003 blocks or a turn within one packed angle
/// step accumulates against the unchanged baseline until it is sent).
fn entity_movement(
    entity: &EntityView,
    baseline: MoveBaseline,
) -> Option<(MovePacket, MoveBaseline)> {
    let (last_pos, last_yaw, last_pitch, _) = baseline;
    let dx = entity.position[0] - last_pos[0];
    let dy = entity.position[1] - last_pos[1];
    let dz = entity.position[2] - last_pos[2];
    let moved = dx * dx + dy * dy + dz * dz >= MIN_MOVE_DISTANCE_SQ;
    let rotated = pack_degrees(entity.yaw) != pack_degrees(last_yaw)
        || pack_degrees(entity.pitch) != pack_degrees(last_pitch);
    if !moved && !rotated {
        return None;
    }
    let rel = (delta_to_short(dx), delta_to_short(dy), delta_to_short(dz));
    let fits = rel.0.is_some() && rel.1.is_some() && rel.2.is_some();
    let packet = match (moved, rotated, fits) {
        (true, false, true) => MovePacket::Pos(MoveEntityPos {
            entity_id: entity.id.0,
            delta_x: rel.0.unwrap_or(0),
            delta_y: rel.1.unwrap_or(0),
            delta_z: rel.2.unwrap_or(0),
            on_ground: entity.on_ground,
        }),
        (false, true, _) => MovePacket::Rot(MoveEntityRot {
            entity_id: entity.id.0,
            yaw: pack_degrees(entity.yaw),
            pitch: pack_degrees(entity.pitch),
            on_ground: entity.on_ground,
        }),
        (true, true, true) => MovePacket::PosRot(MoveEntityPosRot {
            entity_id: entity.id.0,
            delta_x: rel.0.unwrap_or(0),
            delta_y: rel.1.unwrap_or(0),
            delta_z: rel.2.unwrap_or(0),
            yaw: pack_degrees(entity.yaw),
            pitch: pack_degrees(entity.pitch),
            on_ground: entity.on_ground,
        }),
        _ => MovePacket::Sync(EntityPositionSync {
            entity_id: entity.id.0,
            position: EntityVec3 {
                x: entity.position[0],
                y: entity.position[1],
                z: entity.position[2],
            },
            yaw: entity.yaw,
            pitch: entity.pitch,
            on_ground: entity.on_ground,
        }),
    };
    // The baseline is exactly what the packet tells the client: a
    // position-only move keeps the old rotation and vice versa; an absolute
    // sync carries both.
    let synced = moved && !fits;
    let position = if moved { entity.position } else { last_pos };
    let (yaw, pitch) = if rotated || synced {
        (entity.yaw, entity.pitch)
    } else {
        (last_yaw, last_pitch)
    };
    Some((packet, (position, yaw, pitch, entity.head_yaw)))
}

fn push_entity_event(out: &mut Vec<PlannedPacket>, conn: ConnectionId, event: EntityEvent) {
    match event {
        EntityEvent::Status { entity, event } => {
            push_best_effort(
                out,
                conn,
                &WireEntityEvent {
                    entity_id: entity.0,
                    event_id: event,
                },
            );
        }
        EntityEvent::SweepAttack {
            entity,
            position,
            yaw,
        } => {
            let angle = f64::from(yaw).to_radians();
            let dx = -angle.sin();
            let dz = angle.cos();
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundSoundEntity {
                    sound: mc_protocol::codec::Identifier::parse(
                        "minecraft:entity.player.attack.sweep",
                    )
                    .expect("sweep sound identifier"),
                    source: 7,
                    entity_id: entity.0,
                    fixed_range: None,
                    volume: 1.0,
                    pitch: 1.0,
                    seed: i64::from(entity.0),
                },
            );
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundSweepParticle {
                    position,
                    direction: [dx as f32, 0.0, dz as f32],
                },
            );
        }
        EntityEvent::ShieldBlock { entity, disabled } => {
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundSoundEntity {
                    sound: mc_protocol::codec::Identifier::parse(if disabled {
                        "minecraft:item.shield.break"
                    } else {
                        "minecraft:item.shield.block"
                    })
                    .expect("shield sound identifier"),
                    source: 7, // SoundSource.PLAYERS
                    entity_id: entity.0,
                    fixed_range: None,
                    volume: if disabled { 0.8 } else { 1.0 },
                    pitch: 1.0,
                    seed: i64::from(entity.0),
                },
            );
        }
        EntityEvent::GoldenDandelion { entity, locked } => {
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundSoundEntity {
                    sound: mc_protocol::codec::Identifier::parse(if locked {
                        "minecraft:item.golden_dandelion.use"
                    } else {
                        "minecraft:item.golden_dandelion.unuse"
                    })
                    .expect("golden dandelion sound identifier"),
                    source: 7,
                    entity_id: entity.0,
                    fixed_range: None,
                    volume: 1.0,
                    pitch: 1.0,
                    seed: i64::from(entity.0),
                },
            );
        }
        EntityEvent::Bottle {
            entity,
            empty,
            cauldron,
        } => {
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundSoundEntity {
                    sound: mc_protocol::codec::Identifier::parse(if empty {
                        "minecraft:item.bottle.empty"
                    } else {
                        "minecraft:item.bottle.fill"
                    })
                    .expect("bottle sound identifier"),
                    source: if cauldron { 4 } else { 6 },
                    entity_id: entity.0,
                    fixed_range: None,
                    volume: 1.0,
                    pitch: 1.0,
                    seed: i64::from(entity.0),
                },
            );
        }
        EntityEvent::Swing { entity, hand } => {
            push_best_effort(
                out,
                conn,
                &ClientboundSwingAnimation::default_swing(entity.0, matches!(hand, Hand::Off)),
            );
        }
        EntityEvent::Damage {
            entity,
            source_type_id,
            cause,
            direct,
            source_position,
        } => {
            push_best_effort(
                out,
                conn,
                &mc_protocol::packets::play::ClientboundDamageEvent {
                    entity_id: entity.0,
                    source_type_id,
                    source_cause_id: cause.map_or(-1, |id| id.0),
                    source_direct_id: direct.map_or(-1, |id| id.0),
                    source_position,
                },
            );
        }
        EntityEvent::Hurt { entity } => {
            push_best_effort(
                out,
                conn,
                &WireEntityEvent {
                    entity_id: entity.0,
                    event_id: 2,
                },
            );
        }
        EntityEvent::Death { entity } => {
            push_best_effort(
                out,
                conn,
                &WireEntityEvent {
                    entity_id: entity.0,
                    event_id: 3,
                },
            );
        }
        EntityEvent::Pickup {
            item,
            collector,
            count,
        } => {
            push_best_effort(
                out,
                conn,
                &ClientboundTakeItemEntity {
                    item_entity_id: item.0,
                    player_entity_id: collector.0,
                    amount: i32::from(count),
                },
            );
        }
    }
}

/// Squared distance within which players get an explosion (vanilla 64).
const EXPLOSION_RANGE_SQ: f64 = 64.0 * 64.0;

/// The explode packet of a blast, as the vanilla TNT fixture shapes it
/// (26.3 ids: particle explosion_emitter 29, sound reference
/// entity.generic.explode 703, poof 69 / smoke 72 block particles):
/// `viewer`'s own knockback rides it.
fn explode_packet(
    explosion: &mc_core::boundary::ExplosionEvent,
    viewer: EntityId,
) -> ClientboundExplode {
    let [x, y, z] = explosion.center;
    ClientboundExplode {
        center: EntityVec3 { x, y, z },
        radius: explosion.radius,
        block_count: i32::try_from(explosion.blocks).unwrap_or(i32::MAX),
        knockback: explosion
            .knockback
            .iter()
            .find(|(entity, _)| *entity == viewer)
            .map(|(_, [x, y, z])| EntityVec3 {
                x: *x,
                y: *y,
                z: *z,
            }),
        explosion_particle_id: 29,
        sound_reference_id: 703,
        block_particles: vec![
            ExplosionBlockParticle {
                particle_id: 69,
                scaling: 0.5,
                speed: 1.0,
                weight: 1,
            },
            ExplosionBlockParticle {
                particle_id: 72,
                scaling: 1.0,
                speed: 1.0,
                weight: 1,
            },
        ],
        play_sound: true,
    }
}

/// Core velocities are blocks per second; the wire carries blocks per tick.
fn per_tick(velocity: [f64; 3]) -> EntityVec3 {
    EntityVec3 {
        x: velocity[0] / 20.0,
        y: velocity[1] / 20.0,
        z: velocity[2] / 20.0,
    }
}

/// Appearance -> SetEntityData (+ equipment + item-entity stack).
fn push_appearance(
    out: &mut Vec<PlannedPacket>,
    conn: ConnectionId,
    entity: &EntityView,
    previous: Option<&EntityAppearance>,
) {
    push_appearance_for(
        out,
        conn,
        entity.id,
        &entity.appearance,
        entity.type_id,
        previous,
    );
}

/// Living metadata is an exact entity contract property, not a growing list of
/// projectile exceptions. Sending living index 9 to a cloud/rocket is fatal.
fn has_living_metadata(type_id: u32) -> bool {
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).is_some_and(
        |contract| contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living,
    )
}

/// 26.1.2 accessor inheritance: Animal -> AgeableMob starts at 16.
/// Hoglin inherits Animal too; Piglin instead adds its baby accessor after
/// AbstractPiglin's immunity accessor (17). Always send the adult false image
/// so growth and skipped snapshots clear a previously visible child.
fn baby_metadata_index(type_id: u32) -> Option<u8> {
    let contract = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)?;
    match contract.behavior.metadata_schema.as_str() {
        "Armadillo" | "Axolotl" | "Bee" | "Camel" | "CamelHusk" | "Cat" | "Chicken" | "Cow"
        | "Donkey" | "Fox" | "Frog" | "Goat" | "HappyGhast" | "Hoglin" | "Horse" | "Llama"
        | "Mule" | "MushroomCow" | "Nautilus" | "Ocelot" | "Panda" | "Parrot" | "Pig"
        | "PolarBear" | "Rabbit" | "Sheep" | "SkeletonHorse" | "Sniffer" | "Strider"
        | "TraderLlama" | "Turtle" | "Villager" | "Wolf" | "ZombieHorse" | "ZombieNautilus" => {
            Some(AGEABLE_ENTITY_DATA_BABY_INDEX)
        }
        "Zombie" | "Husk" | "Drowned" | "ZombieVillager" | "ZombifiedPiglin" | "Zoglin" => Some(16),
        "Piglin" => Some(17),
        _ => None,
    }
}

fn player_entity_type_id() -> u32 {
    use std::sync::LazyLock;
    static ID: LazyLock<u32> = LazyLock::new(|| {
        mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:player").expect("static id parses"))
            .expect("player entity type present")
    });
    *ID
}

/// Item entities carry no LivingEntity accessors (no health float at index
/// 9): sending it hard-fails the vanilla client (`undefined/mismatched
/// entity accessor minecraft:item index 9`, session dead). Their stack
/// rides `DATA_ITEM` (index 8) via the dedicated arm below instead.
pub(crate) fn push_appearance_for(
    out: &mut Vec<PlannedPacket>,
    conn: ConnectionId,
    id: EntityId,
    appearance: &EntityAppearance,
    type_id: u32,
    previous: Option<&EntityAppearance>,
) {
    push_appearance_with(out, conn, id, appearance, type_id, previous, true);
}

fn push_appearance_with(
    out: &mut Vec<PlannedPacket>,
    conn: ConnectionId,
    id: EntityId,
    appearance: &EntityAppearance,
    type_id: u32,
    previous: Option<&EntityAppearance>,
    send_equipment: bool,
) {
    if previous.is_none_or(|old| old.passengers != appearance.passengers)
        && (previous.is_some() || !appearance.passengers.is_empty())
    {
        push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundSetPassengers {
                vehicle_id: id.0,
                passenger_ids: appearance.passengers.iter().map(|id| id.0).collect(),
            },
        );
    }
    if previous.is_none_or(|old| old.leash_holder != appearance.leash_holder)
        && (previous.is_some() || appearance.leash_holder.is_some())
    {
        push_reliable(
            out,
            conn,
            &mc_protocol::packets::play::ClientboundSetEntityLeash {
                source_entity_id: id.0,
                holder_entity_id: appearance.leash_holder.map(|id| id.0),
            },
        );
    }
    let mut values: Vec<WireEntityDataValue> = Vec::new();
    if let Some((value, _)) = appearance.experience_orb {
        values.push(WireEntityDataValue::Int { index: 8, value });
    }
    values.push(WireEntityDataValue::Byte {
        index: ENTITY_DATA_SHARED_FLAGS_INDEX,
        value: appearance.shared_flags as i8,
    });
    if has_living_metadata(type_id) {
        values.push(WireEntityDataValue::Float {
            index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
            value: appearance.health,
        });
        values.push(WireEntityDataValue::EffectParticles {
            index: LIVING_ENTITY_DATA_EFFECT_PARTICLES_INDEX,
            particles: appearance
                .effect_particles
                .iter()
                .map(|particle| {
                    use mc_core::boundary::EffectParticle as CoreParticle;
                    use mc_protocol::packets::play::EffectParticle as WireParticle;
                    match *particle {
                        CoreParticle::EntityEffect { argb } => WireParticle::EntityEffect { argb },
                        CoreParticle::TrialOmen => WireParticle::TrialOmen,
                        CoreParticle::RaidOmen => WireParticle::RaidOmen,
                    }
                })
                .collect(),
        });
        values.push(WireEntityDataValue::Boolean {
            index: LIVING_ENTITY_DATA_EFFECT_AMBIENCE_INDEX,
            value: appearance.effects_ambient,
        });
        // LivingEntity DATA_ARROW_COUNT_ID / DATA_STINGER_COUNT_ID, INT serializers.
        values.push(WireEntityDataValue::Int {
            index: 12,
            value: appearance.arrow_count,
        });
        values.push(WireEntityDataValue::Int {
            index: 13,
            value: appearance.stinger_count,
        });
    }
    if type_id == player_entity_type_id() {
        values.push(WireEntityDataValue::Float {
            index: PLAYER_DATA_ABSORPTION_INDEX,
            value: appearance.absorption,
        });
    }
    if previous.is_none_or(|old| old.custom_name != appearance.custom_name)
        && (previous.is_some() || appearance.custom_name.is_some())
    {
        values.push(WireEntityDataValue::OptionalComponent {
            index: 2,
            value: appearance.custom_name.clone(),
        });
    }
    if previous.is_none_or(|old| old.custom_name_visible != appearance.custom_name_visible) {
        values.push(WireEntityDataValue::Boolean {
            index: ENTITY_DATA_CUSTOM_NAME_VISIBLE_INDEX,
            value: appearance.custom_name_visible,
        });
    }
    values.push(WireEntityDataValue::Pose {
        index: ENTITY_DATA_POSE_INDEX,
        pose: pose_of(appearance.pose),
    });
    if let Some(index) = baby_metadata_index(type_id) {
        values.push(WireEntityDataValue::Boolean {
            index,
            value: appearance.baby,
        });
    }
    for (index, value) in appearance.extra.iter() {
        values.push(map_extra(*index, value));
    }
    if values.len() > 64 {
        values.truncate(64);
    }
    push_best_effort(
        out,
        conn,
        &ClientboundSetEntityData {
            entity_id: id.0,
            values,
        },
    );
    let equipment_changed = previous.map_or_else(
        || {
            appearance
                .equipment
                .iter()
                .chain([&appearance.body_armor, &appearance.saddle])
                .any(|stack| stack.as_ref().is_some_and(|stack| !stack.is_empty()))
        },
        |old| {
            old.equipment != appearance.equipment
                || old.body_armor != appearance.body_armor
                || old.saddle != appearance.saddle
        },
    );
    if send_equipment && has_living_metadata(type_id) && equipment_changed {
        let equipment = appearance
            .equipment
            .iter()
            .chain([&appearance.body_armor, &appearance.saddle])
            .enumerate()
            .map(|(slot, stack)| EntityEquipment {
                slot: equipment_slot_of(slot),
                item: stack.clone().unwrap_or_default(),
            })
            .collect();
        push_best_effort(
            out,
            conn,
            &ClientboundSetEntityEquipment {
                entity_id: id.0,
                equipment,
            },
        );
    }
    if let Some(stack) = appearance.item.as_ref() {
        push_best_effort(
            out,
            conn,
            &ClientboundSetEntityData {
                entity_id: id.0,
                values: vec![WireEntityDataValue::ItemStack {
                    index: ITEM_ENTITY_DATA_ITEM_INDEX,
                    stack: stack.clone(),
                }],
            },
        );
    }
}

// The core writes sheep wool at the protocol's own index.
const _: () = assert!(
    mc_core::blueprint::SHEEP_WOOL_DATA_INDEX
        == mc_protocol::packets::play::SHEEP_ENTITY_DATA_WOOL_INDEX
);

// The core writes the sleeping position at the protocol's own index.
const _: () = assert!(
    mc_core::sleep::SLEEPING_POS_DATA_INDEX
        == mc_protocol::packets::play::LIVING_ENTITY_DATA_SLEEPING_POS_INDEX
);

fn map_extra(index: u8, value: &mc_core::boundary::EntityDataValue) -> WireEntityDataValue {
    match value.clone() {
        mc_core::boundary::EntityDataValue::Direction(value) => {
            use mc_protocol::packets::play::EntityDirection;
            let value = match value {
                0 => EntityDirection::Down,
                1 => EntityDirection::Up,
                2 => EntityDirection::North,
                3 => EntityDirection::South,
                4 => EntityDirection::West,
                _ => EntityDirection::East,
            };
            WireEntityDataValue::Direction { index, value }
        }
        mc_core::boundary::EntityDataValue::PaintingVariant(value) => {
            WireEntityDataValue::PaintingVariant { index, value }
        }
        mc_core::boundary::EntityDataValue::WeatheringCopper(value) => {
            WireEntityDataValue::WeatheringCopper { index, value }
        }
        mc_core::boundary::EntityDataValue::CopperGolemState(value) => {
            WireEntityDataValue::CopperGolemState { index, value }
        }
        mc_core::boundary::EntityDataValue::VillagerData {
            kind,
            profession,
            level,
        } => WireEntityDataValue::VillagerData {
            index,
            villager_type: kind,
            profession,
            level,
        },
        mc_core::boundary::EntityDataValue::Byte(v) => {
            WireEntityDataValue::Byte { index, value: v }
        }
        mc_core::boundary::EntityDataValue::VarInt(v) => {
            WireEntityDataValue::Int { index, value: v }
        }
        mc_core::boundary::EntityDataValue::VarLong(v) => {
            WireEntityDataValue::Long { index, value: v }
        }
        mc_core::boundary::EntityDataValue::Float(v) => {
            WireEntityDataValue::Float { index, value: v }
        }
        mc_core::boundary::EntityDataValue::Rotations([x, y, z]) => {
            WireEntityDataValue::Rotations {
                index,
                value: EntityRotations { x, y, z },
            }
        }
        mc_core::boundary::EntityDataValue::Boolean(v) => {
            WireEntityDataValue::Boolean { index, value: v }
        }
        mc_core::boundary::EntityDataValue::String(v) => {
            WireEntityDataValue::String { index, value: v }
        }
        mc_core::boundary::EntityDataValue::BlockState(v) => WireEntityDataValue::BlockState {
            index,
            value: mc_protocol::packets::play::BlockStateId::new(v).unwrap_or(
                mc_protocol::packets::play::BlockStateId::new(0).expect("state 0 is valid"),
            ),
        },
        mc_core::boundary::EntityDataValue::Item(v) => WireEntityDataValue::ItemStack {
            index,
            stack: v.unwrap_or(mc_data::item_stack::ItemStack::EMPTY),
        },
        mc_core::boundary::EntityDataValue::EntityEffectParticle(argb) => {
            WireEntityDataValue::EntityEffectParticle { index, argb }
        }
        mc_core::boundary::EntityDataValue::OptionalUnsignedInt(value) => {
            WireEntityDataValue::OptionalUnsignedInt { index, value }
        }
        mc_core::boundary::EntityDataValue::OptionalLivingEntityReference(value) => {
            WireEntityDataValue::OptionalLivingEntityReference { index, value }
        }
        mc_core::boundary::EntityDataValue::OptionalBlockPos(v) => {
            WireEntityDataValue::OptionalBlockPosition {
                index,
                value: v.map(|[x, y, z]| mc_protocol::packets::play::pack_block_pos(x, y, z)),
            }
        }
    }
}

fn pose_of(pose: u8) -> EntityPose {
    match pose {
        1 => EntityPose::FallFlying,
        2 => EntityPose::Sleeping,
        3 => EntityPose::Swimming,
        4 => EntityPose::SpinAttack,
        5 => EntityPose::Crouching,
        6 => EntityPose::LongJumping,
        7 => EntityPose::Dying,
        11 => EntityPose::Roaring,
        12 => EntityPose::Sniffing,
        13 => EntityPose::Emerging,
        14 => EntityPose::Digging,
        _ => EntityPose::Standing,
    }
}

fn equipment_slot_of(slot: usize) -> EquipmentSlot {
    match slot {
        0 => EquipmentSlot::MainHand,
        1 => EquipmentSlot::OffHand,
        2 => EquipmentSlot::Feet,
        3 => EquipmentSlot::Legs,
        4 => EquipmentSlot::Chest,
        5 => EquipmentSlot::Head,
        6 => EquipmentSlot::Body,
        _ => EquipmentSlot::Saddle,
    }
}

/// Nearest-first spiral of the Chebyshev square around `center`.
fn spiral_square(center: (i32, i32), view_distance: i32) -> Vec<(i32, i32)> {
    let radius = view_distance.max(0);
    let mut cells: Vec<(i32, i32, i32)> = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let ring = dx.abs().max(dz.abs());
            cells.push((center.0 + dx, center.1 + dz, ring));
        }
    }
    cells.sort_by_key(|(_, _, ring)| *ring);
    cells.into_iter().map(|(x, z, _)| (x, z)).collect()
}

/// Vanilla `Mth.packDegrees`: floor(yaw * 256 / 360) as a byte.
fn pack_degrees(degrees: f32) -> u8 {
    (degrees.mul_add(256.0, 0.0) / 360.0).floor() as i32 as u8
}

/// `Mth.floor(delta * 4096)` clamped to `i16`, or `None` when the delta
/// exceeds relative-move range (caller falls back to absolute sync).
fn delta_to_short(delta: f64) -> Option<i16> {
    let scaled = (delta * 4096.0).floor();
    if scaled < f64::from(i16::MIN) || scaled > f64::from(i16::MAX) {
        return None;
    }
    Some(scaled as i16)
}

/// Frame `body` once (threshold-256 compression) and push it; nothing is
/// pushed when it does not frame (oversize), like an encode failure.
fn push_framed(
    out: &mut Vec<PlannedPacket>,
    connection: ConnectionId,
    id: i32,
    body: &[u8],
    reliable: bool,
) {
    let mut data = BytesMut::new();
    if append_frame(&mut data, id, body, Compression::Threshold(256)).is_err() {
        return;
    }
    out.push(PlannedPacket {
        connection,
        id,
        reliable,
        data: data.freeze(),
        chunk: None,
        chunk_dimension: mc_core::dimensions::Dimension::Overworld,
    });
}

pub(crate) fn push_reliable<P: Packet>(
    out: &mut Vec<PlannedPacket>,
    connection: ConnectionId,
    packet: &P,
) {
    let mut body = Vec::new();
    if crate::vanilla_items::encode(connection, packet, &mut body).is_ok() {
        push_framed(out, connection, P::ID, &body, true);
    }
}

pub(crate) fn push_best_effort<P: Packet>(
    out: &mut Vec<PlannedPacket>,
    connection: ConnectionId,
    packet: &P,
) {
    let mut body = Vec::new();
    if crate::vanilla_items::encode(connection, packet, &mut body).is_ok() {
        push_framed(out, connection, P::ID, &body, false);
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod plan_tests;

#[cfg(test)]
#[path = "mounts_wire_tests.rs"]
mod mounts_wire_tests;
#[cfg(test)]
#[path = "projectile_wire_tests.rs"]
mod projectile_wire_tests;
