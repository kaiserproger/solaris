//! Lazy world storage on top of the Anvil codec.
//!
//! Opens a vanilla world directory (the one containing
//! `dimensions/minecraft/overworld/region/` or, on older saves,
//! `region/` directly), and serves block queries by loading the
//! covering region file on demand. Chunk and decoded-region LRUs keep
//! recent data resident; dirty chunks are flushed back through region
//! planning/write/commit paths.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use thiserror::Error;

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;

use crate::anvil::{
    ChunkNbtError, ChunkPayload, RegionError, chunk_from_nbt_with_items,
    chunk_to_payload_with_items_at_tick, read_region, write_region_create_new,
};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockPos, ChestBlockEntity, Chunk, ChunkGenerator, ChunkPos, FurnaceBlockEntity,
    HopperBlockEntity, ScheduledBlockTick, ScheduledFluidTick,
};
use crate::light::ChunkLight;
use crate::resident::{ResidentChunkStore, WorldMutationView};
use crate::section::SECTION_DIM;

const REGION_AXIS_CHUNKS: i32 = 32;
const DEFAULT_LRU_CAPACITY: usize = 16;
/// How many decoded regions (`.mca` files with per-chunk payloads
/// already decompressed) we hold resident at once. Each entry is on
/// the order of tens of MB for a dense overworld region; four is a
/// pragmatic default that covers the M3.e view-distance ring around
/// a single player without growing unboundedly.
const DEFAULT_REGION_LRU_CAPACITY: usize = 4;
const READ_VIEW_REGION_AXIS_CHUNKS: i32 = 8;
const READ_VIEW_SHARD_COUNT: usize = 64;
static REGION_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A decoded region: per-chunk payload bytes ready for
/// `mc_nbt::read_named`, keyed by the chunk's local-to-region
/// coordinates `(local_x, local_z)`. Wrapped in an `Arc` in
/// [`WorldStorage`] so the per-chunk lookup can clone a handle
/// without copying the payload bytes.
type DecodedRegion = HashMap<(u8, u8), ChunkPayload>;

#[derive(Debug, Error)]
pub enum WorldError {
    #[error("world directory not found: {0}")]
    Missing(PathBuf),
    #[error("region read: {0}")]
    Region(#[from] RegionError),
    #[error("chunk decode: {0}")]
    ChunkNbt(#[from] ChunkNbtError),
    #[error("NBT parse: {0}")]
    Nbt(#[from] mc_nbt::NbtError),
    #[error("region changed before replace: {0}")]
    StaleRegion(PathBuf),
}

/// Handle to a world's chunk data, generated chunks, and dirty flush state.
pub struct WorldStorage {
    world_root: Option<PathBuf>,
    region_root: PathBuf,
    registry: Arc<BlockRegistry>,
    /// Canonical resident chunks, partitioned into independently locked 8x8 regions.
    resident: ResidentChunkStore,
    /// Immutable block snapshots published for hot readers. Block mutations
    /// replace the affected `Arc<Chunk>` before the writer operation returns.
    read_view: WorldReadView,
    /// Per-chunk scheduled-work hints published by queue mutations. The
    /// simulation loop reads these without taking the world storage mutex.
    scheduled_tick_view: ScheduledTickView,
    /// MRU at the back, LRU at the front. On `get_chunk` we move
    /// the accessed key to the back.
    lru: VecDeque<ChunkPos>,
    capacity: usize,
    /// LRU of *decoded* region files, keyed by region coordinates.
    /// Each entry maps `(local_x, local_z)` → already-decompressed
    /// chunk payload (raw NBT bytes ready for `mc_nbt::read_named`).
    /// This eliminates the per-chunk re-open + re-decompress of the
    /// same `.mca` when many chunks in the same region are touched
    /// in quick succession — the M2 follow-up #1 noted at close-out.
    regions: HashMap<(i32, i32), Arc<DecodedRegion>>,
    region_lru: VecDeque<(i32, i32)>,
    region_capacity: usize,
    item_registry: Option<Arc<ItemRegistry>>,
    /// M7: optional fallback that materialises chunks for positions
    /// not covered by an `.mca` slot. Generated chunks come back
    /// dirty so the M6 flush pipeline persists them; subsequent
    /// reads hit the region file, not the generator.
    generator: Option<Arc<dyn ChunkGenerator>>,
    generator_available: Arc<AtomicBool>,
    /// Keeps compatibility for APIs that return a borrow from `&mut self`.
    /// This is one snapshot handle, not a second resident authority.
    borrowed_chunk: Option<ChunkSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldStorageStats {
    pub chunk_cache_len: usize,
    pub chunk_cache_capacity: usize,
    pub region_cache_len: usize,
    pub region_cache_capacity: usize,
    pub dirty_chunks: usize,
    pub dirty_chunk_cache_saturated: bool,
}

#[derive(Clone)]
pub struct DirtyFlushPlan {
    regions: Vec<DirtyFlushRegionPlan>,
    chunks: usize,
    registry: Arc<BlockRegistry>,
    item_registry: Option<Arc<ItemRegistry>>,
    unix_time: u32,
    #[cfg(test)]
    payload_encode_count: Arc<AtomicU64>,
}

const DIRTY_FLUSH_STALE_REGION_RETRIES: usize = 3;

#[derive(Debug, Clone)]
struct DirtyFlushRegionPlan {
    region: (i32, i32),
    region_path: PathBuf,
    expected_version: Option<RegionFileVersion>,
    dirty_payloads: Vec<PlannedChunkPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionFileVersion {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
struct PlannedChunkPayload {
    pos: ChunkPos,
    current_tick: u64,
    dirty_generation: u64,
    snapshot: ChunkSnapshot,
    #[cfg(test)]
    snapshot_token: ChunkSnapshotToken,
}

#[derive(Debug, Clone)]
pub struct DirtyFlushCommit {
    regions: Vec<DirtyFlushRegionCommit>,
}

#[derive(Debug, Clone)]
struct DirtyFlushRegionCommit {
    region: (i32, i32),
    chunks: Vec<CommittedChunkPayload>,
}

#[derive(Debug, Clone)]
struct CommittedChunkPayload {
    pos: ChunkPos,
    current_tick: u64,
    dirty_generation: u64,
    snapshot: ChunkSnapshot,
    #[cfg(test)]
    snapshot_token: ChunkSnapshotToken,
    #[cfg(test)]
    payload_digest: u64,
    uncompressed_nbt: Vec<u8>,
}

pub type ChunkSnapshot = Arc<Chunk>;

#[cfg(test)]
struct TestChunkMutation {
    resident: ResidentChunkStore,
    position: ChunkPos,
    chunk: ChunkSnapshot,
}

#[cfg(test)]
impl std::ops::Deref for TestChunkMutation {
    type Target = Chunk;

    fn deref(&self) -> &Self::Target {
        &self.chunk
    }
}

#[cfg(test)]
impl std::ops::DerefMut for TestChunkMutation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        make_cached_chunk_mut(&mut self.chunk)
    }
}

#[cfg(test)]
impl Drop for TestChunkMutation {
    fn drop(&mut self) {
        self.resident
            .replace_for_test(self.position, Arc::clone(&self.chunk));
    }
}
type FurnaceSnapshotsByChunk = HashMap<ChunkPos, Arc<HashMap<BlockPos, FurnaceBlockEntity>>>;
type PublishedChunkShard = RwLock<HashMap<ChunkPos, ChunkSnapshot>>;
type FurnaceSnapshotShard = RwLock<FurnaceSnapshotsByChunk>;
pub type DirtyHighWaterNotifier = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub struct WorldReadView {
    chunks: Arc<[PublishedChunkShard; READ_VIEW_SHARD_COUNT]>,
    furnaces: Arc<[FurnaceSnapshotShard; READ_VIEW_SHARD_COUNT]>,
    resident_chunks: Arc<AtomicUsize>,
    dirty_chunks: Arc<AtomicUsize>,
    capacity: usize,
    dirty_saturated: Arc<AtomicBool>,
    dirty_high_water_notifier: Arc<RwLock<Option<DirtyHighWaterNotifier>>>,
}

#[derive(Clone, Default)]
pub struct WorldReadSnapshot {
    chunks: HashMap<ChunkPos, ChunkSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPrepareSource {
    Resident,
    RegionFile,
    Generator,
    Absent,
}

#[derive(Clone)]
pub struct ChunkSourceView {
    resident: WorldReadView,
    region_root: Arc<PathBuf>,
    disk_backed: bool,
    generator_available: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct ScheduledTickView {
    chunks: Arc<RwLock<HashMap<ChunkPos, ScheduledTickHint>>>,
}

#[derive(Clone, Copy, Default)]
struct ScheduledTickHint {
    next_block_tick: Option<u64>,
    next_fluid_tick: Option<u64>,
    hopper_backfill_required: bool,
}

impl WorldReadView {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            chunks: Arc::new(std::array::from_fn(|_| RwLock::new(HashMap::new()))),
            furnaces: Arc::new(std::array::from_fn(|_| RwLock::new(HashMap::new()))),
            resident_chunks: Arc::new(AtomicUsize::new(0)),
            dirty_chunks: Arc::new(AtomicUsize::new(0)),
            capacity: capacity.max(1),
            dirty_saturated: Arc::new(AtomicBool::new(false)),
            dirty_high_water_notifier: Arc::new(RwLock::new(None)),
        }
    }

    #[must_use]
    pub fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        let cpos = chunk_pos_of(pos);
        let chunks = self.chunks[read_view_shard(cpos)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = chunks.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, pos.y, local_z)
    }

    #[must_use]
    pub fn block_mutation_token(&self, pos: BlockPos) -> Option<crate::BlockMutationToken> {
        let cpos = chunk_pos_of(pos);
        let chunks = self.chunks[read_view_shard(cpos)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = chunks.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.block_mutation_token(local_x, pos.y, local_z)
    }

    #[must_use]
    pub fn block_mutation_snapshot(
        &self,
        pos: BlockPos,
    ) -> Option<(BlockStateId, crate::BlockMutationToken)> {
        let cpos = chunk_pos_of(pos);
        let chunks = self.chunks[read_view_shard(cpos)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = chunks.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        Some((
            chunk.get_block(local_x, pos.y, local_z)?,
            chunk.block_mutation_token(local_x, pos.y, local_z)?,
        ))
    }

    #[must_use]
    pub fn snapshot_chunks(&self, positions: &[ChunkPos]) -> WorldReadSnapshot {
        let mut snapshots = HashMap::with_capacity(positions.len());
        for &position in positions {
            let chunks = self.chunks[read_view_shard(position)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(chunk) = chunks.get(&position) {
                snapshots
                    .entry(position)
                    .or_insert_with(|| Arc::clone(chunk));
            }
        }
        WorldReadSnapshot { chunks: snapshots }
    }

    #[must_use]
    pub fn furnace_snapshots(&self, positions: &[ChunkPos]) -> Vec<(BlockPos, FurnaceBlockEntity)> {
        let mut snapshots = Vec::new();
        for position in positions {
            let furnaces = self.furnaces[read_view_shard(*position)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(chunk_furnaces) = furnaces.get(position) {
                snapshots.extend(
                    chunk_furnaces
                        .iter()
                        .map(|(&position, furnace)| (position, furnace.clone())),
                );
            }
        }
        snapshots
    }

    /// Report whether a new chunk can enter the cache without waiting for the
    /// mutable storage owner. The final insert rechecks the same condition.
    #[must_use]
    pub fn can_cache_new_chunk(&self, position: ChunkPos) -> bool {
        if !self.dirty_saturated.load(Ordering::Acquire) {
            return true;
        }
        let chunks = self.chunks[read_view_shard(position)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        chunks.contains_key(&position)
    }

    fn contains_chunk(&self, position: ChunkPos) -> bool {
        self.chunks[read_view_shard(position)]
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&position)
    }

    pub(crate) fn publish_chunk(&self, position: ChunkPos, chunk: ChunkSnapshot) {
        {
            let mut chunks = self.chunks[read_view_shard(position)]
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = chunks.insert(position, Arc::clone(&chunk));
            self.record_replacement(previous.as_deref(), Some(&chunk));
        }
        self.publish_furnaces(position, &chunk);
    }

    pub(crate) fn update_chunk<R>(
        &self,
        position: ChunkPos,
        chunk: &mut ChunkSnapshot,
        update: impl FnOnce(&mut Chunk) -> R,
    ) -> R {
        self.update_chunk_snapshot(position, chunk, |chunk| {
            update(make_cached_chunk_mut(chunk))
        })
    }

    pub(crate) fn update_chunk_snapshot<R>(
        &self,
        position: ChunkPos,
        chunk: &mut ChunkSnapshot,
        update: impl FnOnce(&mut ChunkSnapshot) -> R,
    ) -> R {
        let mut published = self.chunks[read_view_shard(position)]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = published.remove(&position);
        let previous_present = previous.is_some();
        let previous_dirty = previous.as_ref().is_some_and(|chunk| chunk.dirty);
        let previous_dirty_generation = previous
            .as_ref()
            .filter(|chunk| chunk.dirty)
            .map(|chunk| chunk.dirty_generation);
        drop(previous);
        let result = update(chunk);
        published.insert(position, Arc::clone(chunk));
        self.record_replacement_state(
            previous_present,
            previous_dirty,
            previous_dirty_generation,
            Some(chunk),
        );
        result
    }

    pub(crate) fn remove_chunk(&self, position: ChunkPos) {
        {
            let mut chunks = self.chunks[read_view_shard(position)]
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = chunks.remove(&position);
            self.record_replacement(previous.as_deref(), None);
        }
        self.remove_furnaces(position);
    }

    fn remove_furnaces(&self, position: ChunkPos) {
        self.furnaces[read_view_shard(position)]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&position);
    }

    pub(crate) fn publish_furnaces(&self, position: ChunkPos, chunk: &Chunk) {
        let mut furnaces = self.furnaces[read_view_shard(position)]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if chunk.furnaces.is_empty() {
            furnaces.remove(&position);
        } else {
            furnaces.insert(position, Arc::new(chunk.furnaces.clone()));
        }
    }

    pub(crate) fn resident_len(&self) -> usize {
        self.resident_chunks.load(Ordering::Acquire)
    }

    pub(crate) fn dirty_len(&self) -> usize {
        self.dirty_chunks.load(Ordering::Acquire)
    }

    fn record_replacement(&self, previous: Option<&Chunk>, current: Option<&Chunk>) {
        self.record_replacement_state(
            previous.is_some(),
            previous.is_some_and(|chunk| chunk.dirty),
            previous
                .filter(|chunk| chunk.dirty)
                .map(|chunk| chunk.dirty_generation),
            current,
        );
    }

    fn record_replacement_state(
        &self,
        previous_present: bool,
        previous_dirty: bool,
        previous_dirty_generation: Option<u64>,
        current: Option<&Chunk>,
    ) {
        match (previous_present, current.is_some()) {
            (false, true) => {
                self.resident_chunks.fetch_add(1, Ordering::AcqRel);
            }
            (true, false) => {
                self.resident_chunks.fetch_sub(1, Ordering::AcqRel);
            }
            _ => {}
        }
        match (previous_dirty, current.is_some_and(|chunk| chunk.dirty)) {
            (false, true) => {
                self.dirty_chunks.fetch_add(1, Ordering::AcqRel);
            }
            (true, false) => {
                self.dirty_chunks.fetch_sub(1, Ordering::AcqRel);
            }
            _ => {}
        }
        let resident = self.resident_chunks.load(Ordering::Acquire);
        let dirty_saturated =
            resident >= self.capacity && self.dirty_chunks.load(Ordering::Acquire) == resident;
        self.dirty_saturated
            .store(dirty_saturated, Ordering::Release);
        let dirty_state_changed = current.is_some_and(|chunk| {
            chunk.dirty && previous_dirty_generation != Some(chunk.dirty_generation)
        });
        if dirty_saturated && dirty_state_changed {
            self.notify_dirty_flush();
        }
    }

    pub(crate) fn notify_dirty_flush(&self) {
        let notify = self
            .dirty_high_water_notifier
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    fn set_dirty_high_water_notifier(&self, notifier: DirtyHighWaterNotifier) {
        *self
            .dirty_high_water_notifier
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(notifier);
    }

    #[cfg(test)]
    fn lock_chunk_shard_for_test(
        &self,
        position: ChunkPos,
    ) -> std::sync::RwLockWriteGuard<'_, HashMap<ChunkPos, ChunkSnapshot>> {
        self.chunks[read_view_shard(position)]
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn read_view_shard(position: ChunkPos) -> usize {
    let region_x = position.x.div_euclid(READ_VIEW_REGION_AXIS_CHUNKS) as u32;
    let region_z = position.z.div_euclid(READ_VIEW_REGION_AXIS_CHUNKS) as u32;
    (region_x.wrapping_mul(31) ^ region_z) as usize & (READ_VIEW_SHARD_COUNT - 1)
}

impl Default for WorldReadView {
    fn default() -> Self {
        Self::with_capacity(1)
    }
}

impl ChunkSourceView {
    #[must_use]
    pub fn source_for(&self, position: ChunkPos) -> ChunkPrepareSource {
        if self.resident.contains_chunk(position) {
            return ChunkPrepareSource::Resident;
        }
        let (rx, rz) = region_of(position);
        if self.disk_backed && self.region_root.join(format!("r.{rx}.{rz}.mca")).is_file() {
            return ChunkPrepareSource::RegionFile;
        }
        if self.generator_available.load(Ordering::Acquire) {
            ChunkPrepareSource::Generator
        } else {
            ChunkPrepareSource::Absent
        }
    }
}

impl ScheduledTickView {
    #[must_use]
    pub fn block_due(&self, position: ChunkPos, world_tick: u64) -> bool {
        self.chunks
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&position)
            .is_some_and(|hint| {
                hint.hopper_backfill_required
                    || hint
                        .next_block_tick
                        .is_some_and(|trigger_tick| trigger_tick <= world_tick)
            })
    }

    #[must_use]
    pub fn fluid_due(&self, position: ChunkPos, world_tick: u64) -> bool {
        self.chunks
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&position)
            .and_then(|hint| hint.next_fluid_tick)
            .is_some_and(|trigger_tick| trigger_tick <= world_tick)
    }

    pub(crate) fn publish_chunk(
        &self,
        position: ChunkPos,
        chunk: &Chunk,
        registry: &BlockRegistry,
    ) {
        let hint = ScheduledTickHint {
            next_block_tick: chunk
                .scheduled_block_ticks()
                .first()
                .map(|tick| tick.trigger_tick),
            next_fluid_tick: chunk
                .scheduled_fluid_ticks()
                .first()
                .map(|tick| tick.trigger_tick),
            hopper_backfill_required: chunk.hoppers.keys().copied().any(|pos| {
                let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
                let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
                let Some(state_id) = chunk.get_block(local_x, pos.y, local_z) else {
                    return false;
                };
                let Some(state) = registry.by_id(state_id) else {
                    return false;
                };
                state.block.id.path() == "hopper"
                    && !chunk
                        .scheduled_block_ticks()
                        .iter()
                        .any(|tick| tick.pos == pos && tick.block == state.block.id)
            }),
        };
        self.chunks
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(position, hint);
    }

    pub(crate) fn remove_chunk(&self, position: ChunkPos) {
        self.chunks
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&position);
    }
}

impl WorldReadSnapshot {
    #[must_use]
    pub fn chunk(&self, position: ChunkPos) -> Option<ChunkSnapshot> {
        self.chunks.get(&position).map(Arc::clone)
    }

    #[must_use]
    pub fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        let cpos = chunk_pos_of(pos);
        let chunk = self.chunks.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, pos.y, local_z)
    }

    #[must_use]
    pub fn block_mutation_token(&self, pos: BlockPos) -> Option<crate::BlockMutationToken> {
        let cpos = chunk_pos_of(pos);
        let chunk = self.chunks.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.block_mutation_token(local_x, pos.y, local_z)
    }
}

#[cfg(test)]
type ChunkSnapshotToken = usize;

#[cfg(test)]
fn chunk_snapshot_token(chunk: &ChunkSnapshot) -> ChunkSnapshotToken {
    Arc::as_ptr(chunk) as ChunkSnapshotToken
}

#[cfg(test)]
fn payload_digest(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
    payload_encode_count: &AtomicU64,
) -> Result<ChunkPayload, WorldError> {
    payload_encode_count.fetch_add(1, Ordering::Relaxed);
    chunk_to_payload_with_items_at_tick(chunk, registry, item_registry, now, current_tick)
        .map_err(WorldError::from)
}

#[cfg(not(test))]
fn encode_dirty_flush_chunk_payload(
    chunk: &Chunk,
    registry: &BlockRegistry,
    item_registry: Option<&ItemRegistry>,
    now: u32,
    current_tick: u64,
) -> Result<ChunkPayload, WorldError> {
    chunk_to_payload_with_items_at_tick(chunk, registry, item_registry, now, current_tick)
        .map_err(WorldError::from)
}

fn can_fast_clean_chunk(
    chunk: &ChunkSnapshot,
    planned_generation: u64,
    planned_snapshot: &ChunkSnapshot,
) -> bool {
    planned_generation != 0
        && chunk.dirty_generation == planned_generation
        && Arc::ptr_eq(chunk, planned_snapshot)
}

pub(crate) fn make_cached_chunk_mut(chunk: &mut ChunkSnapshot) -> &mut Chunk {
    let invalidate_planned_flush = chunk.dirty && Arc::strong_count(chunk) > 1;
    let chunk = Arc::make_mut(chunk);
    if invalidate_planned_flush {
        chunk.mark_dirty();
    }
    chunk
}

pub enum ChunkSnapshotPlan {
    Cached(ChunkSnapshot),
    Load(ChunkDiskLoadPlan),
}

pub struct ChunkDiskLoadPlan {
    local: (u8, u8),
    region_path: PathBuf,
    disk_backed: bool,
    cached_region: Option<Arc<DecodedRegion>>,
    registry: Arc<BlockRegistry>,
    item_registry: Option<Arc<ItemRegistry>>,
}

impl ChunkDiskLoadPlan {
    #[must_use]
    pub fn has_load_source(&self) -> bool {
        self.cached_region
            .as_ref()
            .is_some_and(|region| region.contains_key(&self.local))
            || (self.disk_backed && self.region_path.is_file())
    }

    pub fn load(self) -> Result<Option<Chunk>, WorldError> {
        let payload = if let Some(region) = self.cached_region {
            region.get(&self.local).cloned()
        } else if self.disk_backed && self.region_path.is_file() {
            read_region(&self.region_path)?
                .into_iter()
                .find(|payload| (payload.local_x, payload.local_z) == self.local)
        } else {
            None
        };

        let Some(payload) = payload else {
            return Ok(None);
        };
        let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cursor)?;
        chunk_from_nbt_with_items(&root, &self.registry, self.item_registry.as_deref())
            .map(Some)
            .map_err(WorldError::from)
    }
}

impl DirtyFlushPlan {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks == 0
    }

    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks
    }

    pub fn write(self) -> Result<DirtyFlushCommit, WorldError> {
        let DirtyFlushPlan {
            regions,
            registry,
            item_registry,
            unix_time,
            #[cfg(test)]
            payload_encode_count,
            ..
        } = self;
        let mut commits = Vec::with_capacity(regions.len());
        for region in regions {
            if region_file_version(&region.region_path)?.as_ref()
                != region.expected_version.as_ref()
            {
                return Err(WorldError::StaleRegion(region.region_path));
            }
            let mut by_slot: HashMap<(u8, u8), ChunkPayload> = if region.expected_version.is_some()
            {
                read_region(&region.region_path)?
                    .into_iter()
                    .map(|p| ((p.local_x, p.local_z), p))
                    .collect()
            } else {
                HashMap::new()
            };

            let mut committed_chunks = Vec::with_capacity(region.dirty_payloads.len());
            for planned in region.dirty_payloads {
                #[cfg(test)]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                    &payload_encode_count,
                )?;
                #[cfg(not(test))]
                let payload = encode_dirty_flush_chunk_payload(
                    &planned.snapshot,
                    &registry,
                    item_registry.as_deref(),
                    unix_time,
                    planned.current_tick,
                )?;
                by_slot.insert((payload.local_x, payload.local_z), payload.clone());
                committed_chunks.push(CommittedChunkPayload {
                    pos: planned.pos,
                    current_tick: planned.current_tick,
                    dirty_generation: planned.dirty_generation,
                    snapshot: planned.snapshot,
                    #[cfg(test)]
                    snapshot_token: planned.snapshot_token,
                    #[cfg(test)]
                    payload_digest: payload_digest(&payload.uncompressed_nbt),
                    uncompressed_nbt: payload.uncompressed_nbt,
                });
            }

            let mut payloads: Vec<ChunkPayload> = by_slot.into_values().collect();
            payloads.sort_by_key(|p| (p.local_z, p.local_x));

            replace_region_file(
                &region.region_path,
                &payloads,
                region.expected_version.as_ref(),
            )?;

            commits.push(DirtyFlushRegionCommit {
                region: region.region,
                chunks: committed_chunks,
            });
        }

        Ok(DirtyFlushCommit { regions: commits })
    }

    #[cfg(test)]
    fn payload_encode_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.payload_encode_count)
    }
}

fn replace_region_file(
    region_path: &Path,
    payloads: &[ChunkPayload],
    expected_version: Option<&RegionFileVersion>,
) -> Result<(), WorldError> {
    if region_file_version(region_path)?.as_ref() != expected_version {
        return Err(WorldError::StaleRegion(region_path.to_path_buf()));
    }

    #[cfg(windows)]
    if expected_version.is_some() {
        return Err(WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: std::io::Error::new(
                ErrorKind::Unsupported,
                "atomic replacement of existing region files is unsupported on Windows",
            ),
        }));
    }

    let tmp_path = write_unique_region_tmp(region_path, payloads)?;
    if expected_version.is_some() {
        install_existing_region_file(region_path, &tmp_path, expected_version)?;
    } else {
        install_new_region_file(region_path, &tmp_path)?;
    }
    sync_parent_dir(region_path)?;
    Ok(())
}

fn install_existing_region_file(
    region_path: &Path,
    tmp_path: &Path,
    expected_version: Option<&RegionFileVersion>,
) -> Result<(), WorldError> {
    if region_file_version(region_path)?.as_ref() != expected_version {
        let _ = std::fs::remove_file(tmp_path);
        return Err(WorldError::StaleRegion(region_path.to_path_buf()));
    }

    std::fs::rename(tmp_path, region_path).map_err(|e| {
        let _ = std::fs::remove_file(tmp_path);
        WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: e,
        })
    })
}

fn install_new_region_file(region_path: &Path, tmp_path: &Path) -> Result<(), WorldError> {
    let result = std::fs::hard_link(tmp_path, region_path);
    let _ = std::fs::remove_file(tmp_path);
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            Err(WorldError::StaleRegion(region_path.to_path_buf()))
        }
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: region_path.to_path_buf(),
            source: e,
        })),
    }
}

fn region_file_version(path: &Path) -> Result<Option<RegionFileVersion>, WorldError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(RegionFileVersion {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: path.to_path_buf(),
            source: e,
        })),
    }
}

fn write_unique_region_tmp(
    region_path: &Path,
    payloads: &[ChunkPayload],
) -> Result<PathBuf, WorldError> {
    for _ in 0..16 {
        let tmp_path = unique_region_tmp_path(region_path);
        match write_region_create_new(&tmp_path, payloads) {
            Ok(()) => return Ok(tmp_path),
            Err(RegionError::Io { source, .. }) if source.kind() == ErrorKind::AlreadyExists => {}
            Err(err) => {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(WorldError::from(err));
            }
        }
    }
    Err(WorldError::Region(RegionError::Io {
        path: region_path.to_path_buf(),
        source: std::io::Error::new(
            ErrorKind::AlreadyExists,
            "could not create unique region temp file",
        ),
    }))
}

fn unique_region_tmp_path(region_path: &Path) -> PathBuf {
    let seq = REGION_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = region_path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "region.mca".into());
    region_path.with_file_name(format!(".{file_name}.tmp.{pid}.{seq}"))
}

fn sync_parent_dir(path: &Path) -> Result<(), WorldError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = match std::fs::File::open(parent) {
        Ok(dir) => dir,
        Err(e) if is_unsupported_dir_sync_error(e.kind()) => {
            return Ok(());
        }
        Err(e) => {
            return Err(WorldError::Region(RegionError::Io {
                path: parent.to_path_buf(),
                source: e,
            }));
        }
    };
    match dir.sync_all() {
        Ok(()) => Ok(()),
        Err(e) if is_unsupported_dir_sync_error(e.kind()) => Ok(()),
        Err(e) => Err(WorldError::Region(RegionError::Io {
            path: parent.to_path_buf(),
            source: e,
        })),
    }
}

fn is_unsupported_dir_sync_error(kind: ErrorKind) -> bool {
    kind == ErrorKind::Unsupported || cfg!(windows) && kind == ErrorKind::PermissionDenied
}

impl WorldStorage {
    /// Open a world directory. Tries the 1.20+ layout
    /// (`dimensions/minecraft/overworld/region/`) first, falls back
    /// to the pre-1.20 flat layout (`region/`). The caller supplies
    /// the block registry by `Arc` so the same registry can be shared
    /// with the rest of the runtime without re-parsing `blocks.json`
    /// for each subsystem.
    pub fn open(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacity(world_dir, registry, DEFAULT_LRU_CAPACITY)
    }

    pub fn open_with_capacity(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        capacity: usize,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacities(world_dir, registry, capacity, DEFAULT_REGION_LRU_CAPACITY)
    }

    pub fn open_with_region_capacity(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        region_capacity: usize,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacities(world_dir, registry, DEFAULT_LRU_CAPACITY, region_capacity)
    }

    pub fn open_with_capacities(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        capacity: usize,
        region_capacity: usize,
    ) -> Result<Self, WorldError> {
        let dir = world_dir.as_ref();
        if !dir.is_dir() {
            return Err(WorldError::Missing(dir.to_path_buf()));
        }
        let candidate_modern = dir
            .join("dimensions")
            .join("minecraft")
            .join("overworld")
            .join("region");
        let candidate_legacy = dir.join("region");
        let region_root = if candidate_modern.is_dir() {
            candidate_modern
        } else if candidate_legacy.is_dir() {
            candidate_legacy
        } else {
            return Err(WorldError::Missing(candidate_modern));
        };

        let capacity = capacity.max(1);
        let read_view = WorldReadView::with_capacity(capacity);
        let scheduled_tick_view = ScheduledTickView::default();
        let resident = ResidentChunkStore::new(
            read_view.clone(),
            scheduled_tick_view.clone(),
            Arc::clone(&registry),
        );
        Ok(Self {
            world_root: Some(dir.to_path_buf()),
            region_root,
            registry,
            resident,
            read_view,
            scheduled_tick_view,
            lru: VecDeque::new(),
            capacity,
            regions: HashMap::new(),
            region_lru: VecDeque::new(),
            region_capacity: region_capacity.max(1),
            item_registry: None,
            generator: None,
            generator_available: Arc::new(AtomicBool::new(false)),
            borrowed_chunk: None,
        })
    }

    /// Build storage with no backing region directory. Missing chunks
    /// resolve only through an attached generator and dirty chunks stay
    /// resident until flushed into a real storage opened on disk.
    #[must_use]
    pub fn in_memory(registry: Arc<BlockRegistry>) -> Self {
        Self::in_memory_with_capacity(registry, DEFAULT_LRU_CAPACITY)
    }

    #[must_use]
    pub fn in_memory_with_capacity(registry: Arc<BlockRegistry>, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let read_view = WorldReadView::with_capacity(capacity);
        let scheduled_tick_view = ScheduledTickView::default();
        let resident = ResidentChunkStore::new(
            read_view.clone(),
            scheduled_tick_view.clone(),
            Arc::clone(&registry),
        );
        Self {
            world_root: None,
            region_root: PathBuf::new(),
            registry,
            resident,
            read_view,
            scheduled_tick_view,
            lru: VecDeque::new(),
            capacity,
            regions: HashMap::new(),
            region_lru: VecDeque::new(),
            region_capacity: DEFAULT_REGION_LRU_CAPACITY,
            item_registry: None,
            generator: None,
            generator_available: Arc::new(AtomicBool::new(false)),
            borrowed_chunk: None,
        }
    }

    #[must_use]
    pub fn with_item_registry(mut self, item_registry: Arc<ItemRegistry>) -> Self {
        self.item_registry = Some(item_registry);
        self
    }

    /// Builder: attach a chunk generator. Slots not present on disk
    /// will now resolve to a freshly-generated chunk instead of
    /// `None`. Generated chunks are inserted as dirty so the M6
    /// flush path persists them before the cache evicts them.
    #[must_use]
    pub fn with_generator(mut self, generator: Arc<dyn ChunkGenerator>) -> Self {
        self.generator_available.store(true, Ordering::Release);
        self.generator = Some(generator);
        self
    }

    /// Convenience for the `mc-server` startup path: swap a generator
    /// in after the fact. Returns the previous generator (if any).
    pub fn set_generator(
        &mut self,
        generator: Option<Arc<dyn ChunkGenerator>>,
    ) -> Option<Arc<dyn ChunkGenerator>> {
        self.generator_available
            .store(generator.is_some(), Ordering::Release);
        std::mem::replace(&mut self.generator, generator)
    }

    #[must_use]
    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
    }

    /// Hand out a shared handle to the block registry so callers
    /// outside `mc-world` can keep it alive (and look up palettes)
    /// without holding the `WorldStorage` itself.
    #[must_use]
    pub fn registry_arc(&self) -> Arc<BlockRegistry> {
        Arc::clone(&self.registry)
    }

    #[must_use]
    pub fn read_view(&self) -> WorldReadView {
        self.read_view.clone()
    }

    #[must_use]
    pub fn mutation_view(&self) -> WorldMutationView {
        self.resident.mutation_view()
    }

    /// Install the server-owned push boundary for dirty-cache high water.
    pub fn set_dirty_high_water_notifier(&self, notifier: DirtyHighWaterNotifier) {
        self.read_view.set_dirty_high_water_notifier(notifier)
    }

    #[must_use]
    pub fn chunk_source_view(&self) -> ChunkSourceView {
        ChunkSourceView {
            resident: self.read_view(),
            region_root: Arc::new(self.region_root.clone()),
            disk_backed: self.world_root.is_some(),
            generator_available: Arc::clone(&self.generator_available),
        }
    }

    #[must_use]
    pub fn scheduled_tick_view(&self) -> ScheduledTickView {
        self.scheduled_tick_view.clone()
    }

    #[must_use]
    pub fn world_root(&self) -> Option<&Path> {
        self.world_root.as_deref()
    }

    /// Look up the block at an absolute world position. Returns
    /// `None` for empty chunk slots, for `y` outside the column,
    /// and for regions whose `.mca` file isn't present.
    pub fn get_block(&mut self, pos: BlockPos) -> Result<Option<BlockStateId>, WorldError> {
        let cpos = chunk_pos_of(pos);
        let chunk = match self.ensure_chunk(cpos)? {
            Some(c) => c,
            None => return Ok(None),
        };
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        Ok(chunk.get_block(local_x, pos.y, local_z))
    }

    /// Read a block only from the resident chunk cache. Unlike `get_block`, this
    /// never loads, decodes, or generates chunks, so background simulation can
    /// sample collision without stalling the shared world lock.
    pub fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        let cpos = chunk_pos_of(pos);
        let chunk = self.resident.snapshot(cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, pos.y, local_z)
    }

    #[must_use]
    pub fn block_mutation_token(&self, pos: BlockPos) -> Option<crate::BlockMutationToken> {
        let cpos = chunk_pos_of(pos);
        let chunk = self.resident.snapshot(cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.block_mutation_token(local_x, pos.y, local_z)
    }

    /// Borrow a cached chunk; loads its region on demand.
    pub fn get_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        self.borrowed_chunk = self.ensure_chunk(cpos)?;
        Ok(self.borrowed_chunk.as_deref())
    }

    /// Clone a chunk if it is already resident or present on disk, but do not
    /// invoke the fallback generator. Background chunk streaming uses this to
    /// keep expensive terrain generation outside the shared world mutex.
    pub fn get_chunk_without_generation(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<ChunkSnapshot>, WorldError> {
        self.ensure_chunk_loaded(cpos, false)?;
        Ok(self.resident.snapshot(cpos))
    }

    pub fn plan_chunk_snapshot_without_generation(&self, cpos: ChunkPos) -> ChunkSnapshotPlan {
        if let Some(chunk) = self.resident.snapshot(cpos) {
            return ChunkSnapshotPlan::Cached(chunk);
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        ChunkSnapshotPlan::Load(ChunkDiskLoadPlan {
            local: (local_x, local_z),
            region_path: self.region_root.join(format!("r.{rx}.{rz}.mca")),
            disk_backed: self.world_root.is_some(),
            cached_region: self.regions.get(&(rx, rz)).cloned(),
            registry: Arc::clone(&self.registry),
            item_registry: self.item_registry.clone(),
        })
    }

    /// Visit every chunk already present in region files without invoking the
    /// fallback generator or mutating the chunk/region caches.
    pub fn visit_existing_chunks_without_generation<F>(
        &self,
        mut visit: F,
    ) -> Result<usize, WorldError>
    where
        F: FnMut(ChunkPos, &Chunk),
    {
        let entries = match std::fs::read_dir(&self.region_root) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(0),
            Err(err) => {
                return Err(RegionError::Io {
                    path: self.region_root.clone(),
                    source: err,
                }
                .into());
            }
        };
        let mut regions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|err| RegionError::Io {
                path: self.region_root.clone(),
                source: err,
            })?;
            let Some((rx, rz)) = parse_region_file_name(&entry.file_name()) else {
                continue;
            };
            regions.push((rx, rz, entry.path()));
        }
        regions.sort_by_key(|(rx, rz, _)| (*rx, *rz));

        let mut visited = 0usize;
        for (rx, rz, path) in regions {
            for payload in read_region(&path)? {
                let cpos = ChunkPos {
                    x: rx * REGION_AXIS_CHUNKS + i32::from(payload.local_x),
                    z: rz * REGION_AXIS_CHUNKS + i32::from(payload.local_z),
                };
                let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
                let (_, root) = mc_nbt::read_named(&mut cursor)?;
                let chunk = chunk_from_nbt_with_items(
                    &root,
                    &self.registry,
                    self.item_registry.as_deref(),
                )?;
                visit(cpos, &chunk);
                visited += 1;
            }
        }
        Ok(visited)
    }

    pub fn commit_chunk_snapshot(
        &mut self,
        cpos: ChunkPos,
        chunk: Chunk,
    ) -> Result<ChunkSnapshot, WorldError> {
        if !self.resident.contains(cpos) {
            self.insert_chunk(cpos, chunk)?;
        } else {
            self.touch(cpos);
        }
        Ok(self
            .resident
            .snapshot(cpos)
            .expect("chunk snapshot commit leaves chunk cached"))
    }

    pub fn replay_journal_chunk(&mut self, mut chunk: Chunk) -> Result<bool, WorldError> {
        let position = chunk.pos;
        let journal_lsn = chunk.world_journal_lsn();
        if self
            .ensure_chunk_loaded(position, false)?
            .is_some_and(|current| current.world_journal_lsn() >= journal_lsn)
        {
            return Ok(false);
        }
        chunk.mark_dirty();
        while !self.resident.contains(position)
            && self.resident.len() >= self.capacity
            && self.evict_clean_chunk()
        {}
        self.resident.replace(position, Arc::new(chunk));
        self.lru.retain(|cached| *cached != position);
        self.lru.push_back(position);
        let region = (position.x.div_euclid(32), position.z.div_euclid(32));
        self.regions.remove(&region);
        self.region_lru.retain(|cached| *cached != region);
        Ok(true)
    }

    pub fn restore_journal_chunk(&mut self, chunk: Chunk) -> Result<(), WorldError> {
        self.replay_journal_chunk(chunk).map(|_| ())
    }

    pub fn try_commit_chunk_snapshot(
        &mut self,
        cpos: ChunkPos,
        chunk: Chunk,
    ) -> Result<Option<ChunkSnapshot>, WorldError> {
        if !self.can_cache_new_chunk(cpos) {
            return Ok(None);
        }
        self.commit_chunk_snapshot(cpos, chunk).map(Some)
    }

    #[must_use]
    pub fn can_cache_new_chunk(&self, cpos: ChunkPos) -> bool {
        self.resident.contains(cpos) || !self.dirty_chunk_cache_saturated()
    }

    /// Clone a resident chunk without disk IO or generation.
    #[must_use]
    pub fn cached_chunk(&self, cpos: ChunkPos) -> Option<Chunk> {
        self.resident
            .snapshot(cpos)
            .map(|chunk| chunk.as_ref().clone())
    }

    /// Return a resident chunk snapshot without disk IO, generation, or full chunk cloning.
    #[must_use]
    pub fn cached_chunk_snapshot(&self, cpos: ChunkPos) -> Option<ChunkSnapshot> {
        self.resident.snapshot(cpos)
    }

    /// Return every resident chunk snapshot without disk IO or LRU mutation.
    #[must_use]
    pub fn resident_chunk_snapshots(&self) -> Vec<(ChunkPos, ChunkSnapshot)> {
        self.resident.snapshots()
    }

    pub fn stamp_cached_chunks_for_world_journal(
        &self,
        decision_id: u64,
        positions: &[ChunkPos],
    ) -> crate::JournalStampResult {
        self.resident
            .stamp_world_journal_conditionally(decision_id, positions)
    }

    #[must_use]
    pub fn generator(&self) -> Option<Arc<dyn ChunkGenerator>> {
        self.generator.as_ref().map(Arc::clone)
    }

    /// Apply a block change at world-space `pos`, refreshing every
    /// heightmap currently attached to the affected chunk. Returns
    /// the previous state, or `None` if the chunk is genuinely
    /// absent (no region file, or the slot is empty in the `.mca`).
    /// Used by the M5.d / M5.e interaction handlers.
    pub fn set_block_at(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Result<Option<BlockStateId>, WorldError> {
        self.set_block_at_inner(pos, state, true)
    }

    /// Apply a block mutation while retaining baked light. The caller must
    /// prove that the old and new block states have identical light behavior.
    pub fn set_block_at_preserving_light(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Result<Option<BlockStateId>, WorldError> {
        self.set_block_at_inner(pos, state, false)
    }

    fn set_block_at_inner(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        clear_baked_light: bool,
    ) -> Result<Option<BlockStateId>, WorldError> {
        let cpos = chunk_pos_of(pos);
        let air = self
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|b| b.default)
            .unwrap_or(BlockStateId(0));
        let registry = Arc::clone(&self.registry);
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        let (prev, removed_furnace) = self
            .resident
            .mutate(cpos, |chunk| {
                let prev = if clear_baked_light {
                    chunk.set_block_and_update(local_x, pos.y, local_z, state, air)
                } else {
                    chunk.set_block_and_update_preserving_light(local_x, pos.y, local_z, state, air)
                };
                let removed_furnace = prev.is_some_and(|prev| prev != state)
                    && prune_incompatible_block_entities(chunk, pos, &registry, state);
                (prev, removed_furnace)
            })
            .expect("ensured chunk remains resident");
        if removed_furnace {
            self.refresh_furnace_snapshots(cpos);
        }
        self.refresh_scheduled_tick_hint(cpos);
        Ok(prev)
    }

    pub fn update_highest_opaque_at(
        &mut self,
        pos: BlockPos,
        table: &BlockLightTable,
    ) -> Result<(), WorldError> {
        let cpos = chunk_pos_of(pos);
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(());
        }
        self.resident
            .mutate(cpos, |chunk| {
                chunk.update_highest_opaque_column(local_x, local_z, table);
            })
            .expect("ensured chunk remains resident");
        Ok(())
    }

    /// Store baked light and publish the replacement chunk snapshot before returning.
    pub fn set_baked_light(
        &mut self,
        cpos: ChunkPos,
        light: &ChunkLight,
    ) -> Result<bool, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        self.touch(cpos);
        self.resident
            .mutate(cpos, |chunk| chunk.set_baked_light(light))
            .expect("ensured chunk remains resident");
        Ok(true)
    }

    #[cfg(test)]
    fn get_chunk_mut(&mut self, cpos: ChunkPos) -> Result<Option<TestChunkMutation>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        self.touch(cpos);
        Ok(self.resident.snapshot(cpos).map(|mut chunk| {
            make_cached_chunk_mut(&mut chunk);
            self.resident.replace_for_test(cpos, Arc::clone(&chunk));
            TestChunkMutation {
                resident: self.resident.clone(),
                position: cpos,
                chunk,
            }
        }))
    }

    pub fn furnace_block_entity(
        &mut self,
        pos: BlockPos,
    ) -> Result<Option<FurnaceBlockEntity>, WorldError> {
        let cpos = chunk_pos_of(pos);
        let Some(chunk) = self.ensure_chunk(cpos)? else {
            return Ok(None);
        };
        Ok(Some(chunk.furnaces.get(&pos).cloned().unwrap_or_default()))
    }

    pub fn set_furnace_block_entity(
        &mut self,
        pos: BlockPos,
        furnace: FurnaceBlockEntity,
    ) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        let changed = self
            .resident
            .mutate(cpos, |chunk| {
                if chunk.furnaces.get(&pos) == Some(&furnace) {
                    return false;
                }
                chunk.furnaces.insert(pos, furnace);
                chunk.mark_dirty();
                true
            })
            .expect("ensured chunk remains resident");
        if changed {
            self.refresh_furnace_snapshots(cpos);
        }
        Ok(true)
    }

    pub fn chest_block_entity(
        &mut self,
        pos: BlockPos,
    ) -> Result<Option<ChestBlockEntity>, WorldError> {
        let cpos = chunk_pos_of(pos);
        let Some(chunk) = self.ensure_chunk(cpos)? else {
            return Ok(None);
        };
        Ok(Some(chunk.chests.get(&pos).cloned().unwrap_or_default()))
    }

    pub fn set_chest_block_entity(
        &mut self,
        pos: BlockPos,
        chest: ChestBlockEntity,
    ) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        self.resident
            .mutate(cpos, |chunk| {
                if chunk.chests.get(&pos) != Some(&chest) {
                    chunk.chests.insert(pos, chest);
                    chunk.mark_dirty();
                }
            })
            .expect("ensured chunk remains resident");
        Ok(true)
    }

    pub fn hopper_block_entity(
        &mut self,
        pos: BlockPos,
    ) -> Result<Option<HopperBlockEntity>, WorldError> {
        let cpos = chunk_pos_of(pos);
        let Some(chunk) = self.ensure_chunk(cpos)? else {
            return Ok(None);
        };
        Ok(Some(chunk.hoppers.get(&pos).cloned().unwrap_or_default()))
    }

    pub fn set_hopper_block_entity(
        &mut self,
        pos: BlockPos,
        hopper: HopperBlockEntity,
    ) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        self.resident
            .mutate(cpos, |chunk| {
                if chunk.hoppers.get(&pos) != Some(&hopper) {
                    chunk.hoppers.insert(pos, hopper);
                    chunk.mark_dirty();
                }
            })
            .expect("ensured chunk remains resident");
        self.refresh_scheduled_tick_hint(cpos);
        Ok(true)
    }

    pub fn set_opaque_block_entity(
        &mut self,
        pos: BlockPos,
        bytes: Vec<u8>,
    ) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        self.resident
            .mutate(cpos, |chunk| {
                if chunk.block_entities.get(&pos) != Some(&bytes) {
                    chunk.block_entities.insert(pos, bytes);
                    chunk.mark_dirty();
                }
            })
            .expect("ensured chunk remains resident");
        Ok(true)
    }

    pub fn scheduled_block_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledBlockTick]>, WorldError> {
        self.borrowed_chunk = self.ensure_chunk(cpos)?;
        let Some(chunk) = self.borrowed_chunk.as_deref() else {
            return Ok(None);
        };
        Ok(Some(chunk.scheduled_block_ticks()))
    }

    pub fn schedule_block_tick(&mut self, tick: ScheduledBlockTick) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(tick.pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        let scheduled = self
            .resident
            .mutate(cpos, |chunk| chunk.schedule_block_tick(tick))
            .expect("ensured chunk remains resident");
        if scheduled {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(scheduled)
    }

    pub fn remove_scheduled_block_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledBlockTick>, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let removed = self
            .resident
            .mutate(cpos, |chunk| chunk.remove_scheduled_block_ticks_at(pos))
            .expect("ensured chunk remains resident");
        if !removed.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(removed)
    }

    pub fn drain_due_block_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Result<Vec<ScheduledBlockTick>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let due = self
            .resident
            .mutate(cpos, |chunk| {
                chunk.drain_due_block_ticks(world_tick, max_ticks)
            })
            .expect("ensured chunk remains resident");
        if !due.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(due)
    }

    pub fn drain_due_cached_block_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledBlockTick> {
        let Some(chunk) = self.resident.snapshot(cpos) else {
            return Vec::new();
        };
        if max_ticks == 0
            || chunk
                .scheduled_block_ticks()
                .first()
                .is_none_or(|tick| tick.trigger_tick > world_tick)
        {
            return Vec::new();
        }
        let due = self
            .resident
            .mutate(cpos, |chunk| {
                chunk.drain_due_block_ticks(world_tick, max_ticks)
            })
            .expect("snapshotted chunk remains resident");
        if !due.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        due
    }

    pub fn scheduled_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledFluidTick]>, WorldError> {
        self.borrowed_chunk = self.ensure_chunk(cpos)?;
        let Some(chunk) = self.borrowed_chunk.as_deref() else {
            return Ok(None);
        };
        Ok(Some(chunk.scheduled_fluid_ticks()))
    }

    pub fn schedule_fluid_tick(&mut self, tick: ScheduledFluidTick) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(tick.pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        let scheduled = self
            .resident
            .mutate(cpos, |chunk| chunk.schedule_fluid_tick(tick))
            .expect("ensured chunk remains resident");
        if scheduled {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(scheduled)
    }

    pub fn remove_scheduled_fluid_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledFluidTick>, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let removed = self
            .resident
            .mutate(cpos, |chunk| chunk.remove_scheduled_fluid_ticks_at(pos))
            .expect("ensured chunk remains resident");
        if !removed.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(removed)
    }

    pub fn drain_due_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Result<Vec<ScheduledFluidTick>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let due = self
            .resident
            .mutate(cpos, |chunk| {
                chunk.drain_due_fluid_ticks(world_tick, max_ticks)
            })
            .expect("ensured chunk remains resident");
        if !due.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        Ok(due)
    }

    pub fn drain_due_cached_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledFluidTick> {
        let Some(chunk) = self.resident.snapshot(cpos) else {
            return Vec::new();
        };
        if max_ticks == 0
            || chunk
                .scheduled_fluid_ticks()
                .first()
                .is_none_or(|tick| tick.trigger_tick > world_tick)
        {
            return Vec::new();
        }
        let due = self
            .resident
            .mutate(cpos, |chunk| {
                chunk.drain_due_fluid_ticks(world_tick, max_ticks)
            })
            .expect("snapshotted chunk remains resident");
        if !due.is_empty() {
            self.refresh_scheduled_tick_hint(cpos);
        }
        due
    }

    /// Insert a freshly generated chunk through the same cache/LRU path
    /// as the lazy generator fallback. Existing cached chunks win.
    pub fn insert_generated_chunk(
        &mut self,
        cpos: ChunkPos,
        mut chunk: Chunk,
    ) -> Result<(), WorldError> {
        chunk.mark_dirty();
        self.insert_chunk(cpos, chunk)
    }

    pub fn try_insert_generated_chunk(
        &mut self,
        cpos: ChunkPos,
        mut chunk: Chunk,
    ) -> Result<bool, WorldError> {
        if !self.can_cache_new_chunk(cpos) {
            return Ok(false);
        }
        chunk.mark_dirty();
        self.insert_chunk(cpos, chunk)?;
        Ok(true)
    }

    fn ensure_chunk(&mut self, cpos: ChunkPos) -> Result<Option<ChunkSnapshot>, WorldError> {
        self.ensure_chunk_loaded(cpos, true)
    }

    fn ensure_chunk_loaded(
        &mut self,
        cpos: ChunkPos,
        allow_generation: bool,
    ) -> Result<Option<ChunkSnapshot>, WorldError> {
        if self.resident.contains(cpos) {
            self.touch(cpos);
            return Ok(self.resident.snapshot(cpos));
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;

        let region = self.ensure_region(rx, rz)?;
        let payload = region.and_then(|r| r.get(&(local_x, local_z)).cloned());

        if let Some(payload) = payload {
            let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
            let (_, root) = mc_nbt::read_named(&mut cursor)?;
            let chunk =
                chunk_from_nbt_with_items(&root, &self.registry, self.item_registry.as_deref())?;
            self.insert_chunk(cpos, chunk)?;
            return Ok(self.resident.snapshot(cpos));
        }

        // M7: no on-disk chunk → ask the generator (if any).
        if allow_generation && let Some(generator) = self.generator.as_ref().map(Arc::clone) {
            let mut chunk = generator.generate(cpos);
            chunk.mark_dirty(); // belt-and-braces; generator already sets this
            self.insert_chunk(cpos, chunk)?;
            return Ok(self.resident.snapshot(cpos));
        }
        Ok(None)
    }

    /// Bring the region at `(rx, rz)` into the region cache and return
    /// a shared handle to its per-chunk payload map. Returns `None`
    /// when the underlying `.mca` file doesn't exist on disk.
    fn ensure_region(
        &mut self,
        rx: i32,
        rz: i32,
    ) -> Result<Option<Arc<DecodedRegion>>, WorldError> {
        let key = (rx, rz);
        if let Some(region) = self.regions.get(&key) {
            let region = Arc::clone(region);
            self.touch_region(key);
            return Ok(Some(region));
        }
        let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
        if !region_path.is_file() {
            return Ok(None);
        }
        let payloads = read_region(&region_path)?;
        let map: HashMap<(u8, u8), ChunkPayload> = payloads
            .into_iter()
            .map(|p| ((p.local_x, p.local_z), p))
            .collect();
        let arc = Arc::new(map);
        self.insert_region(key, Arc::clone(&arc));
        Ok(Some(arc))
    }

    fn insert_region(&mut self, key: (i32, i32), region: Arc<DecodedRegion>) {
        while self.regions.len() >= self.region_capacity {
            if let Some(evict) = self.region_lru.pop_front() {
                self.regions.remove(&evict);
            } else {
                break;
            }
        }
        self.regions.insert(key, region);
        self.region_lru.push_back(key);
    }

    fn touch_region(&mut self, key: (i32, i32)) {
        if let Some(pos) = self.region_lru.iter().position(|&p| p == key) {
            self.region_lru.remove(pos);
            self.region_lru.push_back(key);
        }
    }

    fn insert_chunk(&mut self, cpos: ChunkPos, chunk: Chunk) -> Result<(), WorldError> {
        if self.resident.contains(cpos) {
            self.touch(cpos);
            return Ok(());
        }
        // Dirty chunks are never evicted here: flushing them can rewrite
        // region files while callers hold the shared world mutex. If every
        // resident chunk is dirty, the cache grows until the save pipeline
        // commits them clean.
        while self.resident.len() >= self.capacity && self.evict_clean_chunk() {}
        self.resident.insert_if_absent(cpos, chunk);
        self.lru.push_back(cpos);
        Ok(())
    }

    fn evict_clean_chunk(&mut self) -> bool {
        let scan_len = self.lru.len();
        for _ in 0..scan_len {
            let Some(evict) = self.lru.pop_front() else {
                return false;
            };
            if self
                .resident
                .snapshot(evict)
                .is_some_and(|chunk| chunk.dirty)
            {
                self.lru.push_back(evict);
                continue;
            }
            if self.resident.remove_if_clean(evict) {
                return true;
            }
        }
        false
    }

    fn refresh_scheduled_tick_hint(&self, cpos: ChunkPos) {
        if let Some(chunk) = self.resident.snapshot(cpos) {
            self.scheduled_tick_view
                .publish_chunk(cpos, &chunk, &self.registry);
        } else {
            self.scheduled_tick_view.remove_chunk(cpos);
        }
    }

    fn refresh_furnace_snapshots(&self, cpos: ChunkPos) {
        if let Some(chunk) = self.resident.snapshot(cpos) {
            self.read_view.publish_furnaces(cpos, &chunk);
        } else {
            self.read_view.remove_furnaces(cpos);
        }
    }

    fn touch(&mut self, cpos: ChunkPos) {
        if let Some(pos) = self.lru.iter().position(|&p| p == cpos) {
            self.lru.remove(pos);
            self.lru.push_back(cpos);
        }
    }

    /// How many chunks are currently resident. Useful for tests and
    /// startup logging.
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.resident.len()
    }

    /// How many decoded regions are currently resident. Tests and
    /// the M3.f bench use this to confirm the region cache fires.
    #[must_use]
    pub fn region_cache_len(&self) -> usize {
        self.regions.len()
    }

    #[must_use]
    pub fn region_cache_capacity(&self) -> usize {
        self.region_capacity
    }

    #[must_use]
    pub fn stats(&self) -> WorldStorageStats {
        WorldStorageStats {
            chunk_cache_len: self.resident.len(),
            chunk_cache_capacity: self.capacity,
            region_cache_len: self.regions.len(),
            region_cache_capacity: self.region_capacity,
            dirty_chunks: self.dirty_count(),
            dirty_chunk_cache_saturated: self.dirty_chunk_cache_saturated(),
        }
    }

    #[must_use]
    pub fn dirty_chunk_cache_saturated(&self) -> bool {
        self.resident.len() >= self.capacity && self.resident.dirty_count() == self.resident.len()
    }

    /// Build a dirty chunk flush plan. The plan owns dirty chunk snapshots and
    /// the region versions observed while planning so callers can encode and
    /// write region files after releasing any outer world mutex without
    /// replacing a newer region snapshot.
    pub fn plan_dirty_flush(&self) -> Result<DirtyFlushPlan, WorldError> {
        self.plan_dirty_flush_at_tick(0)
    }

    pub fn plan_dirty_flush_at_tick(
        &self,
        current_tick: u64,
    ) -> Result<DirtyFlushPlan, WorldError> {
        self.plan_dirty_flush_at_tick_bounded(current_tick, usize::MAX)
    }

    /// Build one bounded pressure-flush batch. This fast path caps the retained
    /// plan and encoding/write work; full checkpoints use the unbounded planner.
    pub fn plan_dirty_flush_at_tick_bounded(
        &self,
        current_tick: u64,
        max_chunks: usize,
    ) -> Result<DirtyFlushPlan, WorldError> {
        let mut dirty_snapshots: Vec<(ChunkPos, ChunkSnapshot)> = self
            .resident
            .flushable_snapshots()
            .into_iter()
            .filter(|(_, chunk)| chunk.dirty)
            .collect();
        dirty_snapshots.sort_by_key(|(pos, _)| {
            (
                pos.x.div_euclid(REGION_AXIS_CHUNKS),
                pos.z.div_euclid(REGION_AXIS_CHUNKS),
                pos.z,
                pos.x,
            )
        });
        dirty_snapshots.truncate(max_chunks);
        if dirty_snapshots.is_empty() {
            return Ok(DirtyFlushPlan {
                regions: Vec::new(),
                chunks: 0,
                registry: Arc::clone(&self.registry),
                item_registry: self.item_registry.as_ref().map(Arc::clone),
                unix_time: 0,
                #[cfg(test)]
                payload_encode_count: Arc::new(AtomicU64::new(0)),
            });
        }
        let mut by_region: HashMap<(i32, i32), Vec<(ChunkPos, ChunkSnapshot)>> = HashMap::new();
        for (pos, chunk) in dirty_snapshots {
            by_region
                .entry(region_of(pos))
                .or_default()
                .push((pos, chunk));
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let mut regions = Vec::with_capacity(by_region.len());
        let mut chunks = 0usize;
        for ((rx, rz), mut snapshots) in by_region {
            snapshots.sort_by_key(|(pos, _)| (pos.z, pos.x));
            let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
            let expected_version = region_file_version(&region_path)?;
            let mut dirty_payloads = Vec::with_capacity(snapshots.len());
            for (cpos, chunk) in snapshots {
                dirty_payloads.push(PlannedChunkPayload {
                    pos: cpos,
                    current_tick,
                    dirty_generation: chunk.dirty_generation,
                    snapshot: Arc::clone(&chunk),
                    #[cfg(test)]
                    snapshot_token: chunk_snapshot_token(&chunk),
                });
                chunks += 1;
            }
            regions.push(DirtyFlushRegionPlan {
                region: (rx, rz),
                region_path,
                expected_version,
                dirty_payloads,
            });
        }

        Ok(DirtyFlushPlan {
            regions,
            chunks,
            registry: Arc::clone(&self.registry),
            item_registry: self.item_registry.as_ref().map(Arc::clone),
            unix_time: now,
            #[cfg(test)]
            payload_encode_count: Arc::new(AtomicU64::new(0)),
        })
    }

    #[must_use]
    pub fn has_flushable_dirty_chunks(&self) -> bool {
        self.resident.has_flushable_dirty()
    }

    /// Commit a written flush plan. Chunks are marked clean only if their dirty
    /// generation still permits the comparison and the encoded payload still
    /// matches the payload that was written. Chunks changed after planning
    /// remain dirty.
    pub fn commit_dirty_flush(&mut self, commit: DirtyFlushCommit) -> Result<usize, WorldError> {
        let mut cleaned = 0usize;
        let mut written_regions = Vec::new();
        for region in commit.regions {
            written_regions.push(region.region);
            for planned in region.chunks {
                let CommittedChunkPayload {
                    pos,
                    current_tick,
                    dirty_generation,
                    snapshot,
                    uncompressed_nbt,
                    ..
                } = planned;
                let registry = Arc::clone(&self.registry);
                let item_registry = self.item_registry.clone();
                let cleaned_chunk = self
                    .resident
                    .mutate_snapshot(pos, move |chunk| {
                        if !chunk.dirty {
                            return Ok(false);
                        }
                        if dirty_generation != 0 && chunk.dirty_generation != dirty_generation {
                            return Ok(false);
                        }
                        let matches = if can_fast_clean_chunk(chunk, dirty_generation, &snapshot) {
                            true
                        } else {
                            let current = chunk_to_payload_with_items_at_tick(
                                chunk,
                                &registry,
                                item_registry.as_deref(),
                                0,
                                current_tick,
                            )?;
                            current.uncompressed_nbt == uncompressed_nbt
                        };
                        if matches {
                            drop(snapshot);
                            make_cached_chunk_mut(chunk).dirty = false;
                        }
                        Ok::<_, WorldError>(matches)
                    })
                    .transpose()?
                    .unwrap_or(false);
                cleaned += usize::from(cleaned_chunk);
            }
        }
        for region in written_regions {
            self.regions.remove(&region);
            self.region_lru.retain(|&k| k != region);
        }

        Ok(cleaned)
    }

    /// M6.b: write every dirty chunk in the cache back to its
    /// `.mca` region file. Returns the number of chunks flushed.
    /// Groups dirty chunks by region so each `r.X.Z.mca` is rewritten
    /// at most once per call.
    pub fn flush_dirty(&mut self) -> Result<usize, WorldError> {
        self.flush_dirty_at_tick(0)
    }

    pub fn flush_dirty_at_tick(&mut self, current_tick: u64) -> Result<usize, WorldError> {
        self.flush_dirty_at_tick_with_pre_write_hook(current_tick, |_| {})
    }

    fn flush_dirty_at_tick_with_pre_write_hook(
        &mut self,
        current_tick: u64,
        mut pre_write: impl FnMut(&DirtyFlushPlan),
    ) -> Result<usize, WorldError> {
        let mut stale_retries = 0usize;
        loop {
            let plan = self.plan_dirty_flush_at_tick(current_tick)?;
            if plan.is_empty() {
                return Ok(0);
            }
            pre_write(&plan);
            match plan.write() {
                Ok(commit) => return self.commit_dirty_flush(commit),
                Err(WorldError::StaleRegion(_))
                    if stale_retries < DIRTY_FLUSH_STALE_REGION_RETRIES =>
                {
                    stale_retries += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Number of dirty chunks currently in the cache. Used by tests
    /// and the Ctrl-C shutdown log.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.resident.dirty_count()
    }
}

pub(crate) fn prune_incompatible_block_entities(
    chunk: &mut Chunk,
    pos: BlockPos,
    registry: &BlockRegistry,
    state: BlockStateId,
) -> bool {
    let path = registry.by_id(state).map(|state| state.block.id.path());
    let keeps_chest = path.is_some_and(|path| matches!(path, "chest" | "barrel"));
    let keeps_furnace =
        path.is_some_and(|path| matches!(path, "furnace" | "blast_furnace" | "smoker"));
    let keeps_hopper = path.is_some_and(|path| path == "hopper");
    let keeps_opaque = path.is_some_and(block_path_may_have_opaque_block_entity);

    let removed_chest = !keeps_chest && chunk.chests.remove(&pos).is_some();
    let removed_furnace = !keeps_furnace && chunk.furnaces.remove(&pos).is_some();
    let removed_hopper = !keeps_hopper && chunk.hoppers.remove(&pos).is_some();
    let removed_opaque = !keeps_opaque && chunk.block_entities.remove(&pos).is_some();
    let removed = removed_chest | removed_furnace | removed_hopper | removed_opaque;
    if removed {
        chunk.mark_dirty();
    }
    removed_furnace
}

fn block_path_may_have_opaque_block_entity(path: &str) -> bool {
    path.ends_with("_sign")
        || path.ends_with("_hanging_sign")
        || path.ends_with("_banner")
        || path.ends_with("_head")
        || path.ends_with("_skull")
        || path.ends_with("_shulker_box")
        || matches!(
            path,
            "beacon"
                | "bed"
                | "bell"
                | "brewing_stand"
                | "campfire"
                | "command_block"
                | "comparator"
                | "conduit"
                | "daylight_detector"
                | "decorated_pot"
                | "enchanting_table"
                | "ender_chest"
                | "end_gateway"
                | "flower_pot"
                | "jigsaw"
                | "jukebox"
                | "lectern"
                | "mob_spawner"
                | "moving_piston"
                | "piston_head"
                | "sculk_sensor"
                | "sculk_shrieker"
                | "soul_campfire"
                | "structure_block"
                | "trapped_chest"
                | "trial_spawner"
                | "vault"
        )
}

fn chunk_pos_of(pos: BlockPos) -> ChunkPos {
    ChunkPos {
        x: pos.x.div_euclid(SECTION_DIM as i32),
        z: pos.z.div_euclid(SECTION_DIM as i32),
    }
}

fn region_of(cpos: ChunkPos) -> (i32, i32) {
    (
        cpos.x.div_euclid(REGION_AXIS_CHUNKS),
        cpos.z.div_euclid(REGION_AXIS_CHUNKS),
    )
}

fn parse_region_file_name(name: &OsStr) -> Option<(i32, i32)> {
    let name = name.to_str()?;
    let name = name.strip_prefix("r.")?.strip_suffix(".mca")?;
    let (rx, rz) = name.split_once('.')?;
    if rz.contains('.') {
        return None;
    }
    Some((rx.parse().ok()?, rz.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{MAX_Y, MIN_Y};
    use mc_nbt::Tag;
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
    }

    fn top_non_air_y(world: &mut WorldStorage, x: i32, z: i32, air: BlockStateId) -> Option<i32> {
        (MIN_Y..MAX_Y)
            .rev()
            .find(|&y| world.get_block(BlockPos { x, y, z }).ok().flatten() != Some(air))
    }

    fn air_state_id(registry: &BlockRegistry) -> BlockStateId {
        registry
            .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
            .map(|b| b.default)
            .unwrap()
    }

    fn single_air_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            }])
            .unwrap(),
        )
    }

    fn air_stone_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    fn air_stone_chest_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:chest").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    fn air_stone_furnace_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:furnace").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    fn air_stone_hopper_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:hopper").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    #[test]
    fn region_of_handles_negative_coordinates() {
        assert_eq!(region_of(ChunkPos { x: 0, z: 0 }), (0, 0));
        assert_eq!(region_of(ChunkPos { x: 31, z: 31 }), (0, 0));
        assert_eq!(region_of(ChunkPos { x: 32, z: 0 }), (1, 0));
        assert_eq!(region_of(ChunkPos { x: -1, z: -1 }), (-1, -1));
        assert_eq!(region_of(ChunkPos { x: -32, z: 0 }), (-1, 0));
        assert_eq!(region_of(ChunkPos { x: -33, z: 0 }), (-2, 0));
    }

    #[test]
    fn chunk_pos_of_handles_negative_coordinates() {
        assert_eq!(
            chunk_pos_of(BlockPos { x: 0, y: 0, z: 0 }),
            ChunkPos { x: 0, z: 0 }
        );
        assert_eq!(
            chunk_pos_of(BlockPos { x: 15, y: 0, z: 15 }),
            ChunkPos { x: 0, z: 0 }
        );
        assert_eq!(
            chunk_pos_of(BlockPos { x: 16, y: 0, z: 0 }),
            ChunkPos { x: 1, z: 0 }
        );
        assert_eq!(
            chunk_pos_of(BlockPos { x: -1, y: 0, z: -1 }),
            ChunkPos { x: -1, z: -1 }
        );
        assert_eq!(
            chunk_pos_of(BlockPos { x: -16, y: 0, z: 0 }),
            ChunkPos { x: -1, z: 0 }
        );
    }

    #[test]
    fn open_with_region_capacity_sets_region_lru_capacity() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));

        let world = WorldStorage::open_with_region_capacity(tmp.path(), registry, 7).unwrap();

        assert_eq!(world.region_cache_capacity(), 7);
    }

    #[test]
    fn chunk_source_view_tracks_generator_and_resident_chunks() {
        struct StubGenerator;

        impl ChunkGenerator for StubGenerator {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                Chunk::empty(
                    pos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                )
            }
        }

        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let source = world.chunk_source_view();
        let position = ChunkPos { x: 2, z: -3 };
        assert_eq!(source.source_for(position), ChunkPrepareSource::Absent);

        world.set_generator(Some(Arc::new(StubGenerator)));
        assert_eq!(source.source_for(position), ChunkPrepareSource::Generator);

        world
            .insert_generated_chunk(position, StubGenerator.generate(position))
            .unwrap();
        assert_eq!(source.source_for(position), ChunkPrepareSource::Resident);
    }

    #[test]
    fn chunk_source_view_recognizes_region_file() {
        let tmp = tempfile::tempdir().unwrap();
        let region = tmp.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        std::fs::write(region.join("r.0.0.mca"), []).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = WorldStorage::open(tmp.path(), registry).unwrap();

        assert_eq!(
            world
                .chunk_source_view()
                .source_for(ChunkPos { x: 17, z: 4 }),
            ChunkPrepareSource::RegionFile
        );
    }

    #[test]
    fn storage_stats_report_cache_and_dirty_pressure() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 2);
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();

        let stats = world.stats();

        assert_eq!(stats.chunk_cache_len, 1);
        assert_eq!(stats.chunk_cache_capacity, 2);
        assert_eq!(stats.region_cache_len, 0);
        assert_eq!(stats.region_cache_capacity, 4);
        assert_eq!(stats.dirty_chunks, 1);
        assert!(!stats.dirty_chunk_cache_saturated);
    }

    #[test]
    fn block_mutation_version_advances_on_changes_and_detects_aba() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();
        let pos = BlockPos { x: 1, y: 0, z: 1 };

        let initial = world.block_mutation_token(pos).expect("initial token");
        assert_eq!(initial.version, 0);
        world.set_block_at(pos, BlockStateId(1)).unwrap();
        let first = world.block_mutation_token(pos).expect("first token");
        assert_eq!(first.chunk_instance_id, initial.chunk_instance_id);
        assert_eq!(first.version, 1);
        world.set_block_at(pos, BlockStateId(1)).unwrap();
        assert_eq!(world.block_mutation_token(pos), Some(first));
        world.set_block_at(pos, BlockStateId(0)).unwrap();
        world.set_block_at(pos, BlockStateId(1)).unwrap();

        assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(1)));
        assert_eq!(
            world.block_mutation_token(pos).expect("ABA token").version,
            3
        );
    }

    #[test]
    fn read_view_publishes_immutable_chunk_edits() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let read_view = world.read_view();
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 0, z: 1 };
        world
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    mc_data::Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();

        let before = read_view.snapshot_chunks(&[cpos]);
        assert_eq!(before.get_cached_block(pos), Some(BlockStateId(0)));
        assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(0)));
        let before_token = before.block_mutation_token(pos).unwrap();
        let view_token = read_view.block_mutation_token(pos).unwrap();

        world.set_block_at(pos, BlockStateId(1)).unwrap();

        let after = read_view.snapshot_chunks(&[cpos]);
        assert_eq!(after.get_cached_block(pos), Some(BlockStateId(1)));
        assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(1)));
        assert_eq!(before.get_cached_block(pos), Some(BlockStateId(0)));
        assert_eq!(before_token.version, 0);
        assert_eq!(view_token.version, 0);
        assert_eq!(after.block_mutation_token(pos).unwrap().version, 1);
        assert_eq!(read_view.block_mutation_token(pos).unwrap().version, 1);
    }

    #[test]
    fn read_view_writer_does_not_block_an_independent_region() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let world = WorldStorage::in_memory(registry);
        let read_view = world.read_view();
        let held_region = ChunkPos { x: 0, z: 0 };
        let independent_region = ChunkPos { x: 8, z: 0 };
        let held = read_view.lock_chunk_shard_for_test(held_region);
        let worker_view = read_view.clone();
        let (completed, observed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let block = worker_view.get_cached_block(BlockPos {
                x: independent_region.x * SECTION_DIM as i32,
                y: 0,
                z: independent_region.z * SECTION_DIM as i32,
            });
            completed.send(block).expect("reader completion");
        });

        let result = observed.recv_timeout(std::time::Duration::from_secs(1));
        drop(held);
        worker.join().expect("independent reader");

        assert_eq!(result, Ok(None));
    }

    #[test]
    fn resident_mutation_is_canonical_and_independent_between_regions() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let held_region = ChunkPos { x: 0, z: 0 };
        let target_chunk = ChunkPos { x: 8, z: 0 };
        for position in [held_region, target_chunk] {
            world
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        let target = BlockPos {
            x: target_chunk.x * SECTION_DIM as i32,
            y: 0,
            z: target_chunk.z * SECTION_DIM as i32,
        };
        let expected_token = world.block_mutation_token(target).expect("target token");
        let read_view = world.read_view();
        let mutation = world.mutation_view();
        let held = read_view.lock_chunk_shard_for_test(held_region);
        let (completed, observed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = mutation.set_block_if_current(
                target,
                BlockStateId(0),
                expected_token,
                BlockStateId(1),
                false,
            );
            completed.send(result).expect("mutation completion");
        });

        let result = observed.recv_timeout(std::time::Duration::from_secs(1));
        drop(held);
        worker.join().expect("regional mutation worker");

        assert_eq!(
            result,
            Ok(crate::ResidentBlockMutation::Applied(BlockStateId(0)))
        );
        assert_eq!(
            world
                .cached_chunk_snapshot(target_chunk)
                .unwrap()
                .get_block(0, 0, 0),
            Some(BlockStateId(1))
        );
    }

    #[test]
    fn resident_batch_rejects_stale_precondition_without_partial_mutation() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let first = BlockPos { x: 1, y: 0, z: 1 };
        let stale = BlockPos { x: 2, y: 0, z: 1 };
        let first_token = world.block_mutation_token(first).unwrap();
        let mut stale_token = world.block_mutation_token(stale).unwrap();
        stale_token.version += 1;

        let result = world.mutation_view().apply_block_edits_conditionally(
            &[
                crate::ResidentBlockEdit {
                    pos: first,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
                crate::ResidentBlockEdit {
                    pos: stale,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
            ],
            &[
                crate::ResidentBlockPrecondition {
                    pos: first,
                    expected_state: BlockStateId(0),
                    expected_token: first_token,
                },
                crate::ResidentBlockPrecondition {
                    pos: stale,
                    expected_state: BlockStateId(0),
                    expected_token: stale_token,
                },
            ],
            &[],
            None,
            None,
        );

        assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
        assert_eq!(world.get_cached_block(first), Some(BlockStateId(0)));
        assert_eq!(world.get_cached_block(stale), Some(BlockStateId(0)));
        assert_eq!(world.block_mutation_token(first), Some(first_token));
    }

    #[test]
    fn resident_fluid_tick_commit_consumes_edits_and_reschedules_atomically() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let source = BlockPos { x: 1, y: 0, z: 1 };
        let target = BlockPos { x: 1, y: 1, z: 1 };
        let water = Identifier::parse("minecraft:water").unwrap();
        world.set_block_at(target, BlockStateId(1)).unwrap();
        let due = ScheduledFluidTick::new(source, water.clone(), 10, 0);
        world.schedule_fluid_tick(due.clone()).unwrap();
        let target_token = world.block_mutation_token(target).unwrap();
        let follow_up = ScheduledFluidTick::new(target, water, 15, 0);

        let (result, touched) = world
            .mutation_view()
            .apply_fluid_tick_plan_conditionally_journaled(
                7,
                &crate::ResidentFluidTickPlan {
                    consumed_ticks: &[due],
                    edits: &[crate::ResidentBlockEdit {
                        pos: target,
                        new_state: BlockStateId(2),
                        preserve_light: false,
                    }],
                    preconditions: &[crate::ResidentBlockPrecondition {
                        pos: target,
                        expected_state: BlockStateId(1),
                        expected_token: target_token,
                    }],
                    scheduled_ticks: std::slice::from_ref(&follow_up),
                    light_table: None,
                    leaf_trigger_tick: None,
                },
            );

        assert!(matches!(
            result,
            crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
        ));
        assert_eq!(touched, vec![chunk_pos]);
        let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
        assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(2)));
        let scheduled = chunk.scheduled_fluid_ticks();
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, follow_up.pos);
        assert_eq!(scheduled[0].fluid, follow_up.fluid);
        assert_eq!(scheduled[0].trigger_tick, follow_up.trigger_tick);
        assert_eq!(scheduled[0].priority, follow_up.priority);
        assert_eq!(scheduled[0].sequence(), 1);
        assert_eq!(chunk.world_journal_lsn(), 7);
    }

    #[test]
    fn resident_fluid_tick_stale_plan_keeps_due_tick_and_block() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let source = BlockPos { x: 1, y: 0, z: 1 };
        let target = BlockPos { x: 1, y: 1, z: 1 };
        let due =
            ScheduledFluidTick::new(source, Identifier::parse("minecraft:water").unwrap(), 10, 0);
        world.schedule_fluid_tick(due.clone()).unwrap();
        let mut stale_token = world.block_mutation_token(target).unwrap();
        stale_token.version += 1;

        let result = world.mutation_view().apply_fluid_tick_plan_conditionally(
            &crate::ResidentFluidTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[crate::ResidentBlockEdit {
                    pos: target,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                }],
                preconditions: &[crate::ResidentBlockPrecondition {
                    pos: target,
                    expected_state: BlockStateId(0),
                    expected_token: stale_token,
                }],
                scheduled_ticks: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );

        assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
        let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
        assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(0)));
        assert_eq!(chunk.scheduled_fluid_ticks(), &[due]);
    }

    #[test]
    fn resident_scheduled_block_tick_commit_consumes_and_edits_atomically() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let position = BlockPos { x: 1, y: 1, z: 1 };
        world.set_block_at(position, BlockStateId(1)).unwrap();
        let due = ScheduledBlockTick::new(
            position,
            Identifier::parse("minecraft:stone").unwrap(),
            10,
            0,
        );
        world.schedule_block_tick(due.clone()).unwrap();
        let token = world.block_mutation_token(position).unwrap();

        let (result, touched) = world
            .mutation_view()
            .apply_scheduled_block_tick_plan_conditionally_journaled(
                8,
                &crate::ResidentScheduledBlockTickPlan {
                    consumed_ticks: &[due],
                    edits: &[crate::ResidentBlockEdit {
                        pos: position,
                        new_state: BlockStateId(2),
                        preserve_light: false,
                    }],
                    preconditions: &[crate::ResidentBlockPrecondition {
                        pos: position,
                        expected_state: BlockStateId(1),
                        expected_token: token,
                    }],
                    light_table: None,
                    leaf_trigger_tick: None,
                },
            );

        assert!(matches!(
            result,
            crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
        ));
        assert_eq!(touched, vec![chunk_pos]);
        let chunk = world.cached_chunk_snapshot(chunk_pos).unwrap();
        assert_eq!(chunk.get_block(1, 1, 1), Some(BlockStateId(2)));
        assert!(chunk.scheduled_block_ticks().is_empty());
        assert_eq!(chunk.world_journal_lsn(), 8);
    }

    #[test]
    fn resident_scheduled_block_tick_stale_plan_keeps_due_tick() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let position = BlockPos { x: 1, y: 1, z: 1 };
        let due = ScheduledBlockTick::new(
            position,
            Identifier::parse("minecraft:stone").unwrap(),
            10,
            0,
        );
        world.schedule_block_tick(due.clone()).unwrap();
        let mut stale_token = world.block_mutation_token(position).unwrap();
        stale_token.version += 1;

        let result = world
            .mutation_view()
            .apply_scheduled_block_tick_plan_conditionally(
                &crate::ResidentScheduledBlockTickPlan {
                    consumed_ticks: std::slice::from_ref(&due),
                    edits: &[crate::ResidentBlockEdit {
                        pos: position,
                        new_state: BlockStateId(1),
                        preserve_light: false,
                    }],
                    preconditions: &[crate::ResidentBlockPrecondition {
                        pos: position,
                        expected_state: BlockStateId(0),
                        expected_token: stale_token,
                    }],
                    light_table: None,
                    leaf_trigger_tick: None,
                },
            );

        assert_eq!(result, crate::ResidentBlockEditBatchResult::Stale);
        assert_eq!(
            world
                .cached_chunk_snapshot(chunk_pos)
                .unwrap()
                .scheduled_block_ticks(),
            &[due]
        );
    }

    #[test]
    fn resident_hopper_tick_backfill_is_idempotent() {
        let registry = air_stone_hopper_registry();
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let position = BlockPos { x: 1, y: 1, z: 1 };
        world.set_block_at(position, BlockStateId(2)).unwrap();
        world
            .set_hopper_block_entity(position, HopperBlockEntity::default())
            .unwrap();
        let mutation = world.mutation_view();

        assert_eq!(mutation.backfill_hopper_ticks(&[chunk_pos], 20), 1);
        assert_eq!(mutation.backfill_hopper_ticks(&[chunk_pos], 20), 0);
        let ticks = world
            .cached_chunk_snapshot(chunk_pos)
            .unwrap()
            .scheduled_block_ticks()
            .to_vec();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].pos, position);
        assert_eq!(ticks[0].block.as_str(), "minecraft:hopper");
        assert_eq!(ticks[0].trigger_tick, 20);
    }

    #[test]
    fn resident_batch_schedules_tick_only_for_applied_position() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let changed = BlockPos { x: 1, y: 0, z: 1 };
        let unchanged = BlockPos { x: 2, y: 0, z: 1 };
        let changed_token = world.block_mutation_token(changed).unwrap();
        let unchanged_token = world.block_mutation_token(unchanged).unwrap();
        let changed_tick =
            ScheduledBlockTick::new(changed, Identifier::parse("minecraft:air").unwrap(), 20, 0);
        let unchanged_tick = ScheduledBlockTick::new(
            unchanged,
            Identifier::parse("minecraft:air").unwrap(),
            20,
            0,
        );

        let result = world.mutation_view().apply_block_edits_conditionally(
            &[
                crate::ResidentBlockEdit {
                    pos: changed,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
                crate::ResidentBlockEdit {
                    pos: unchanged,
                    new_state: BlockStateId(0),
                    preserve_light: false,
                },
            ],
            &[
                crate::ResidentBlockPrecondition {
                    pos: changed,
                    expected_state: BlockStateId(0),
                    expected_token: changed_token,
                },
                crate::ResidentBlockPrecondition {
                    pos: unchanged,
                    expected_state: BlockStateId(0),
                    expected_token: unchanged_token,
                },
            ],
            &[changed_tick.clone(), unchanged_tick],
            None,
            None,
        );

        let crate::ResidentBlockEditBatchResult::Applied(applied) = result else {
            panic!("resident batch did not commit");
        };
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].pos, changed);
        assert_eq!(world.get_cached_block(changed), Some(BlockStateId(1)));
        assert_eq!(
            world.scheduled_block_ticks(chunk_pos).unwrap().unwrap(),
            &[changed_tick]
        );
    }

    #[test]
    fn resident_batch_rejects_cross_region_before_mutation() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let first_chunk = ChunkPos { x: 0, z: 0 };
        let other_chunk = ChunkPos { x: 8, z: 0 };
        for chunk_pos in [first_chunk, other_chunk] {
            world
                .insert_generated_chunk(
                    chunk_pos,
                    Chunk::empty(chunk_pos, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        let first = BlockPos { x: 1, y: 0, z: 1 };
        let other = BlockPos {
            x: other_chunk.x * SECTION_DIM as i32,
            y: 0,
            z: 1,
        };

        let result = world.mutation_view().apply_block_edits_conditionally(
            &[
                crate::ResidentBlockEdit {
                    pos: first,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
                crate::ResidentBlockEdit {
                    pos: other,
                    new_state: BlockStateId(1),
                    preserve_light: false,
                },
            ],
            &[],
            &[],
            None,
            None,
        );

        assert_eq!(result, crate::ResidentBlockEditBatchResult::CrossRegion);
        assert_eq!(world.get_cached_block(first), Some(BlockStateId(0)));
        assert_eq!(world.get_cached_block(other), Some(BlockStateId(0)));
    }

    #[test]
    fn resident_batch_updates_highest_opaque_inside_commit() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        world
            .set_baked_light(chunk_pos, &ChunkLight::filled(15, 0))
            .unwrap();
        let pos = BlockPos { x: 1, y: 0, z: 1 };
        let token = world.block_mutation_token(pos).unwrap();
        let light = BlockLightTable::from_arrays(
            "resident batch test",
            vec![0, 0],
            vec![0, 15],
            vec![true, false],
        );

        let result = world.mutation_view().apply_block_edits_conditionally(
            &[crate::ResidentBlockEdit {
                pos,
                new_state: BlockStateId(1),
                preserve_light: false,
            }],
            &[crate::ResidentBlockPrecondition {
                pos,
                expected_state: BlockStateId(0),
                expected_token: token,
            }],
            &[],
            Some(&light),
            None,
        );

        let crate::ResidentBlockEditBatchResult::Applied(applied) = result else {
            panic!("resident light-changing batch did not commit");
        };
        assert_eq!(applied.len(), 1);
        assert!(applied[0].previous_light.is_some());
        assert_eq!(
            world
                .cached_chunk_snapshot(chunk_pos)
                .unwrap()
                .highest_opaque_y(1, 1),
            Some(0)
        );
    }

    #[test]
    fn resident_batch_schedules_adjacent_leaf_inside_commit() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:oak_leaves").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        );
        let mut world = WorldStorage::in_memory(registry);
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(chunk_pos, Chunk::empty(chunk_pos, BlockStateId(0), biome))
            .unwrap();
        let trunk = BlockPos { x: 1, y: 1, z: 1 };
        let leaf = BlockPos { x: 2, y: 1, z: 1 };
        world.set_block_at(trunk, BlockStateId(1)).unwrap();
        world.set_block_at(leaf, BlockStateId(2)).unwrap();
        let token = world.block_mutation_token(trunk).unwrap();

        let result = world.mutation_view().apply_block_edits_conditionally(
            &[crate::ResidentBlockEdit {
                pos: trunk,
                new_state: BlockStateId(0),
                preserve_light: false,
            }],
            &[crate::ResidentBlockPrecondition {
                pos: trunk,
                expected_state: BlockStateId(1),
                expected_token: token,
            }],
            &[],
            None,
            Some(12),
        );

        assert!(matches!(
            result,
            crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
        ));
        assert_eq!(
            world.scheduled_block_ticks(chunk_pos).unwrap().unwrap(),
            &[ScheduledBlockTick::new(
                leaf,
                Identifier::parse("minecraft:oak_leaves").unwrap(),
                12,
                0,
            )]
        );
    }

    #[test]
    fn baked_light_replaces_published_chunk_snapshot() {
        let registry = single_air_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let read_view = world.read_view();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let before = read_view.snapshot_chunks(&[cpos]);
        assert!(
            ChunkLight::from_section_lights(&before.chunk(cpos).unwrap().section_lights).is_none()
        );

        let baked = ChunkLight::filled(15, 0);
        assert!(world.set_baked_light(cpos, &baked).unwrap());

        let after = read_view.snapshot_chunks(&[cpos]);
        assert!(
            ChunkLight::from_section_lights(&after.chunk(cpos).unwrap().section_lights).is_some()
        );
        assert!(
            ChunkLight::from_section_lights(&before.chunk(cpos).unwrap().section_lights).is_none(),
            "an already-issued immutable snapshot must not change"
        );
    }

    #[test]
    fn stale_regional_baked_light_publish_is_all_or_nothing() {
        let registry = single_air_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let read_view = world.read_view();
        let mutation_view = world.mutation_view();
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos {
            x: crate::resident::WORLD_REGION_AXIS_CHUNKS,
            z: 0,
        };
        for position in [first, second] {
            world
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        let expected = [first, second]
            .into_iter()
            .map(|position| {
                let snapshot = read_view.snapshot_chunks(&[position]);
                (position, snapshot.chunk(position))
            })
            .collect::<HashMap<_, _>>();

        let newer_second_light = ChunkLight::filled(7, 3);
        world.set_baked_light(second, &newer_second_light).unwrap();
        let proposed = ChunkLight::filled(15, 0);

        assert!(!mutation_view.publish_baked_light_conditionally(
            &expected,
            [(first, &proposed), (second, &proposed)],
        ));
        let after = read_view.snapshot_chunks(&[first, second]);
        let first_after = after.chunk(first).unwrap();
        let second_after = after.chunk(second).unwrap();
        assert!(ChunkLight::from_section_lights(&first_after.section_lights).is_none());
        assert_eq!(
            ChunkLight::from_section_lights(&second_after.section_lights),
            Some(newer_second_light)
        );
    }

    #[test]
    fn read_snapshot_returns_shared_chunk_by_position() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();

        let snapshot = world.read_view().snapshot_chunks(&[cpos]);
        let chunk = snapshot.chunk(cpos).expect("published chunk is present");

        assert_eq!(chunk.pos, cpos);
        assert!(Arc::ptr_eq(
            &chunk,
            &world.cached_chunk_snapshot(cpos).unwrap()
        ));
    }

    #[test]
    fn journal_restore_replaces_resident_chunk_and_publishes_scheduled_state() {
        let registry = single_air_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 2, z: -3 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(position, BlockStateId(0), biome.clone()),
            )
            .unwrap();
        let before = world.cached_chunk_snapshot(position).unwrap();
        let tick_position = BlockPos {
            x: position.x * 16,
            y: 64,
            z: position.z * 16,
        };
        let mut restored = Chunk::empty(position, BlockStateId(0), biome);
        restored.schedule_block_tick(ScheduledBlockTick::new(
            tick_position,
            Identifier::parse("minecraft:air").unwrap(),
            7,
            0,
        ));
        restored.set_world_journal_lsn(1);

        world.restore_journal_chunk(restored).unwrap();

        let after = world.cached_chunk_snapshot(position).unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert!(after.dirty);
        assert_eq!(
            world.scheduled_block_ticks(position).unwrap().unwrap(),
            &[ScheduledBlockTick::new(
                tick_position,
                Identifier::parse("minecraft:air").unwrap(),
                7,
                0,
            )]
        );
        assert_eq!(world.stats().dirty_chunks, 1);
    }

    #[test]
    fn read_view_removes_evicted_clean_chunks() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
        let read_view = world.read_view();
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos { x: 1, z: 0 };
        world
            .commit_chunk_snapshot(first, Chunk::empty(first, BlockStateId(0), biome.clone()))
            .unwrap();
        assert_eq!(
            read_view
                .snapshot_chunks(&[first])
                .get_cached_block(BlockPos { x: 0, y: 0, z: 0 }),
            Some(BlockStateId(0))
        );

        world
            .commit_chunk_snapshot(second, Chunk::empty(second, BlockStateId(0), biome))
            .unwrap();

        assert!(
            read_view
                .snapshot_chunks(&[first])
                .get_cached_block(BlockPos { x: 0, y: 0, z: 0 })
                .is_none()
        );
    }

    #[test]
    fn saturated_dirty_state_changes_notify_without_a_new_high_water_edge() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
        let position = ChunkPos { x: 0, z: 0 };
        let block = BlockPos { x: 1, y: 64, z: 1 };
        let notifications = Arc::new(AtomicUsize::new(0));
        world.set_dirty_high_water_notifier({
            let notifications = Arc::clone(&notifications);
            Arc::new(move || {
                notifications.fetch_add(1, Ordering::SeqCst);
            })
        });
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert_eq!(notifications.load(Ordering::SeqCst), 1);

        assert_eq!(
            world
                .mutation_view()
                .schedule_fluid_ticks(&[ScheduledFluidTick::new(
                    block,
                    Identifier::parse("minecraft:water").unwrap(),
                    20,
                    0,
                )]),
            1
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 2);

        assert_eq!(
            world
                .mutation_view()
                .schedule_fluid_ticks(&[ScheduledFluidTick::new(
                    block,
                    Identifier::parse("minecraft:water").unwrap(),
                    21,
                    0,
                ),]),
            1
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn unchanged_resident_mutation_does_not_notify_flush_consumer() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(registry);
        let position = ChunkPos { x: 0, z: 0 };
        let block = BlockPos { x: 1, y: 64, z: 1 };
        let tick =
            ScheduledFluidTick::new(block, Identifier::parse("minecraft:water").unwrap(), 20, 0);
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert_eq!(
            world
                .mutation_view()
                .schedule_fluid_ticks(std::slice::from_ref(&tick)),
            1
        );
        let notifications = Arc::new(AtomicUsize::new(0));
        world.set_dirty_high_water_notifier({
            let notifications = Arc::clone(&notifications);
            Arc::new(move || {
                notifications.fetch_add(1, Ordering::SeqCst);
            })
        });

        assert_eq!(world.mutation_view().schedule_fluid_ticks(&[tick]), 0);
        assert_eq!(notifications.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dirty_pressure_try_insert_defers_without_growth() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();

        world
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome.clone()),
            )
            .unwrap();
        assert!(world.stats().dirty_chunk_cache_saturated);

        let inserted = world
            .try_insert_generated_chunk(
                ChunkPos { x: 1, z: 0 },
                Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();

        assert!(!inserted);
        assert_eq!(world.cache_len(), 1);
        assert!(world.cached_chunk(ChunkPos { x: 1, z: 0 }).is_none());
    }

    #[test]
    fn dirty_pressure_try_commit_defers_loaded_chunk_without_growth() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();

        world
            .insert_generated_chunk(
                ChunkPos { x: 0, z: 0 },
                Chunk::empty(ChunkPos { x: 0, z: 0 }, BlockStateId(0), biome.clone()),
            )
            .unwrap();

        let committed = world
            .try_commit_chunk_snapshot(
                ChunkPos { x: 1, z: 0 },
                Chunk::empty(ChunkPos { x: 1, z: 0 }, BlockStateId(0), biome),
            )
            .unwrap();

        assert!(committed.is_none());
        assert_eq!(world.cache_len(), 1);
        assert!(world.cached_chunk(ChunkPos { x: 1, z: 0 }).is_none());
    }

    #[test]
    fn cached_chunk_snapshots_are_shared_and_copy_on_write() {
        let air = mc_data::blocks::BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        };
        let stone = mc_data::blocks::BlockReport {
            id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        };
        let registry = Arc::new(BlockRegistry::from_report(&[air, stone]).unwrap());
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();

        let before = world.cached_chunk_snapshot(cpos).unwrap();
        assert_eq!(before.get_block(1, 0, 1), Some(BlockStateId(0)));

        assert_eq!(
            world
                .set_block_at(BlockPos { x: 1, y: 0, z: 1 }, BlockStateId(1))
                .unwrap(),
            Some(BlockStateId(0))
        );
        let after = world.cached_chunk_snapshot(cpos).unwrap();

        assert_eq!(before.get_block(1, 0, 1), Some(BlockStateId(0)));
        assert_eq!(after.get_block(1, 0, 1), Some(BlockStateId(1)));
        assert!(!Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn furnace_block_entities_are_chunk_scoped_runtime_state() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        let mut furnace = world.furnace_block_entity(pos).unwrap().unwrap();
        assert!(furnace.slots[0].is_empty());
        furnace.slots[0] = crate::chunk::FurnaceSlot {
            count: 2,
            item_id: 42,
            damage: Some(7),
            enchantments: Vec::new(),
        };
        furnace.cook_progress = 11;

        assert!(
            world
                .set_furnace_block_entity(pos, furnace.clone())
                .unwrap()
        );
        assert_eq!(world.dirty_count(), 1);
        assert_eq!(world.furnace_block_entity(pos).unwrap(), Some(furnace));
    }

    #[test]
    fn resident_double_chest_commit_preflights_before_any_write() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let positions = [BlockPos { x: 1, y: 2, z: 3 }, BlockPos { x: 2, y: 2, z: 3 }];
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let mut initial = [ChestBlockEntity::default(), ChestBlockEntity::default()];
        initial[0].slots[0].item_id = 41;
        initial[0].slots[0].count = 1;
        initial[1].slots[0].item_id = 42;
        initial[1].slots[0].count = 2;
        for (&position, chest) in positions.iter().zip(&initial) {
            world
                .set_chest_block_entity(position, chest.clone())
                .unwrap();
        }
        let mutation = world.mutation_view();
        let mut updated = initial.clone();
        updated[0].slots[0].count = 3;
        updated[1].slots[0].count = 4;
        let mut stale = initial.clone();
        stale[1].slots[0].count = 99;

        assert!(matches!(
            mutation.commit_chests_conditionally(&positions, &stale, &updated),
            crate::ResidentChestCommitResult::Rejected(authoritative)
                if authoritative == initial
        ));
        for (&position, chest) in positions.iter().zip(&initial) {
            assert_eq!(
                world.chest_block_entity(position).unwrap(),
                Some(chest.clone())
            );
        }

        assert_eq!(
            mutation.commit_chests_conditionally(&positions, &initial, &updated),
            crate::ResidentChestCommitResult::Applied
        );
        for (&position, chest) in positions.iter().zip(&updated) {
            assert_eq!(
                world.chest_block_entity(position).unwrap(),
                Some(chest.clone())
            );
        }
    }

    #[test]
    fn resident_furnace_commit_rejects_stale_before_write() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let position = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let mut initial = FurnaceBlockEntity::default();
        initial.slots[0].item_id = 41;
        initial.slots[0].count = 2;
        world
            .set_furnace_block_entity(position, initial.clone())
            .unwrap();
        let mutation = world.mutation_view();
        let mut stale = initial.clone();
        stale.slots[0].count = 99;
        let mut updated = initial.clone();
        updated.slots[0].count = 1;

        assert!(matches!(
            mutation.commit_furnace_conditionally(position, &stale, &updated),
            crate::ResidentFurnaceCommitResult::Rejected(authoritative)
                if authoritative == initial
        ));
        assert_eq!(
            world.furnace_block_entity(position).unwrap(),
            Some(initial.clone())
        );

        assert_eq!(
            mutation.commit_furnace_conditionally(position, &initial, &updated),
            crate::ResidentFurnaceCommitResult::Applied
        );
        assert_eq!(world.furnace_block_entity(position).unwrap(), Some(updated));
    }

    #[test]
    fn resident_furnace_tick_commit_rejects_stale_burn_state() {
        let registry = air_stone_furnace_registry();
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let position = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(position, BlockStateId(2)).unwrap();
        let initial = FurnaceBlockEntity {
            burn_remaining: 10,
            burn_total: 10,
            ..FurnaceBlockEntity::default()
        };
        world
            .set_furnace_block_entity(position, initial.clone())
            .unwrap();
        let mutation = world.mutation_view();
        let mut current = initial.clone();
        current.burn_remaining = 9;
        world
            .set_furnace_block_entity(position, current.clone())
            .unwrap();
        let mut stale_update = initial.clone();
        stale_update.burn_remaining = 8;

        assert_eq!(
            mutation.commit_furnace_tick_conditionally(
                position,
                BlockStateId(2),
                &initial,
                &stale_update,
            ),
            crate::ResidentFurnaceTickCommitResult::Stale
        );
        assert_eq!(
            mutation.furnace_tick_snapshot(position),
            Some((BlockStateId(2), current))
        );
    }

    #[test]
    fn resident_hopper_transfer_rejects_stale_endpoint_without_partial_write() {
        let registry = air_stone_hopper_registry();
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let hopper_position = BlockPos { x: 1, y: 2, z: 3 };
        let chest_position = BlockPos { x: 2, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world
            .set_block_at(hopper_position, BlockStateId(2))
            .unwrap();
        world.set_block_at(chest_position, BlockStateId(1)).unwrap();
        let mut hopper = HopperBlockEntity::default();
        hopper.slots[0].item_id = 42;
        hopper.slots[0].count = 1;
        world
            .set_hopper_block_entity(hopper_position, hopper.clone())
            .unwrap();
        let chest = ChestBlockEntity::default();
        world
            .set_chest_block_entity(chest_position, chest.clone())
            .unwrap();
        let mut updated_hopper = hopper.clone();
        updated_hopper.slots[0] = crate::FurnaceSlot::EMPTY;
        updated_hopper.transfer_cooldown = 8;
        let mut updated_chest = chest.clone();
        updated_chest.slots[0].item_id = 42;
        updated_chest.slots[0].count = 1;
        let next_tick = ScheduledBlockTick::new(
            hopper_position,
            Identifier::parse("minecraft:hopper").unwrap(),
            21,
            0,
        );
        let plan = crate::ResidentHopperTransferPlan {
            expected_states: vec![
                (hopper_position, BlockStateId(2)),
                (chest_position, BlockStateId(1)),
            ],
            hoppers: vec![crate::ResidentBlockEntityChange {
                position: hopper_position,
                expected: hopper.clone(),
                updated: updated_hopper.clone(),
            }],
            chests: vec![crate::ResidentBlockEntityChange {
                position: chest_position,
                expected: chest.clone(),
                updated: updated_chest.clone(),
            }],
            furnaces: Vec::new(),
            scheduled_block_ticks: vec![next_tick.clone()],
        };
        let mutation = world.mutation_view();
        let mut stale = plan.clone();
        stale.chests[0].expected.slots[0].count = 99;

        assert_eq!(
            mutation.commit_hopper_transfer_conditionally(&stale),
            crate::ResidentHopperTransferCommitResult::Stale
        );
        assert_eq!(
            world.hopper_block_entity(hopper_position).unwrap(),
            Some(hopper)
        );
        assert_eq!(
            world.chest_block_entity(chest_position).unwrap(),
            Some(chest)
        );
        assert!(
            world
                .scheduled_block_ticks(cpos)
                .unwrap()
                .unwrap()
                .is_empty()
        );

        assert_eq!(
            mutation.commit_hopper_transfer_conditionally(&plan),
            crate::ResidentHopperTransferCommitResult::Applied
        );
        assert_eq!(
            world.hopper_block_entity(hopper_position).unwrap(),
            Some(updated_hopper)
        );
        assert_eq!(
            world.chest_block_entity(chest_position).unwrap(),
            Some(updated_chest)
        );
        assert_eq!(
            world.scheduled_block_ticks(cpos).unwrap().unwrap(),
            &[next_tick]
        );
    }

    #[test]
    fn resident_scheduled_hopper_transfer_consumes_due_tick_and_sets_journal_fence() {
        let registry = air_stone_hopper_registry();
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let hopper_position = BlockPos { x: 1, y: 2, z: 3 };
        let chest_position = BlockPos { x: 2, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world
            .set_block_at(hopper_position, BlockStateId(2))
            .unwrap();
        world.set_block_at(chest_position, BlockStateId(1)).unwrap();
        let mut hopper = HopperBlockEntity::default();
        hopper.slots[0].item_id = 42;
        hopper.slots[0].count = 1;
        world
            .set_hopper_block_entity(hopper_position, hopper.clone())
            .unwrap();
        let chest = ChestBlockEntity::default();
        world
            .set_chest_block_entity(chest_position, chest.clone())
            .unwrap();
        let due = ScheduledBlockTick::new(
            hopper_position,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        );
        world.schedule_block_tick(due.clone()).unwrap();
        let mut updated_hopper = hopper;
        updated_hopper.slots[0] = crate::FurnaceSlot::EMPTY;
        updated_hopper.transfer_cooldown = 8;
        let mut updated_chest = chest;
        updated_chest.slots[0].item_id = 42;
        updated_chest.slots[0].count = 1;
        let next_tick = ScheduledBlockTick::new(
            hopper_position,
            Identifier::parse("minecraft:hopper").unwrap(),
            21,
            0,
        );
        let plan = crate::ResidentHopperTransferPlan {
            expected_states: vec![
                (hopper_position, BlockStateId(2)),
                (chest_position, BlockStateId(1)),
            ],
            hoppers: vec![crate::ResidentBlockEntityChange {
                position: hopper_position,
                expected: world.hopper_block_entity(hopper_position).unwrap().unwrap(),
                updated: updated_hopper.clone(),
            }],
            chests: vec![crate::ResidentBlockEntityChange {
                position: chest_position,
                expected: world.chest_block_entity(chest_position).unwrap().unwrap(),
                updated: updated_chest.clone(),
            }],
            furnaces: Vec::new(),
            scheduled_block_ticks: vec![next_tick.clone()],
        };
        let mutation = world.mutation_view();

        let (result, touched) =
            mutation.commit_scheduled_hopper_transfer_conditionally_journaled(7, &[due], &plan);

        assert_eq!(result, crate::ResidentHopperTransferCommitResult::Applied);
        assert_eq!(touched, vec![cpos]);
        assert_eq!(
            world.hopper_block_entity(hopper_position).unwrap(),
            Some(updated_hopper)
        );
        assert_eq!(
            world.chest_block_entity(chest_position).unwrap(),
            Some(updated_chest)
        );
        let scheduled = world.scheduled_block_ticks(cpos).unwrap().unwrap();
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pos, next_tick.pos);
        assert_eq!(scheduled[0].block, next_tick.block);
        assert_eq!(scheduled[0].trigger_tick, next_tick.trigger_tick);
        assert_eq!(scheduled[0].priority, next_tick.priority);
        let snapshot = world.cached_chunk_snapshot(cpos).unwrap();
        assert_eq!(snapshot.world_journal_lsn(), 7);
        assert!(world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(mutation.clear_journal_pending_conditionally(7, &[cpos]), 1);
        assert_eq!(world.plan_dirty_flush().unwrap().regions.len(), 1);
    }

    #[test]
    fn resident_opaque_block_entity_commit_rejects_stale_token() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(registry);
        let cpos = ChunkPos { x: 0, z: 0 };
        let position = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(position, BlockStateId(1)).unwrap();
        let stale_token = world.block_mutation_token(position).unwrap();
        world.set_block_at(position, BlockStateId(0)).unwrap();
        world.set_block_at(position, BlockStateId(1)).unwrap();
        let current_token = world.block_mutation_token(position).unwrap();
        let mutation = world.mutation_view();
        let bytes = vec![10, 0, 0, 0];

        assert_eq!(
            mutation.commit_opaque_block_entity_conditionally(
                position,
                BlockStateId(1),
                stale_token,
                bytes.clone(),
            ),
            crate::ResidentOpaqueBlockEntityCommitResult::Stale
        );
        assert!(
            !world
                .cached_chunk(cpos)
                .unwrap()
                .block_entities
                .contains_key(&position)
        );

        assert_eq!(
            mutation.commit_opaque_block_entity_conditionally(
                position,
                BlockStateId(1),
                current_token,
                bytes.clone(),
            ),
            crate::ResidentOpaqueBlockEntityCommitResult::Applied
        );
        assert_eq!(
            world
                .cached_chunk(cpos)
                .unwrap()
                .block_entities
                .get(&position),
            Some(&bytes)
        );
    }

    #[test]
    fn read_view_tracks_furnace_set_and_block_replacement() {
        let registry = air_stone_furnace_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let read_view = world.read_view();
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(pos, BlockStateId(2)).unwrap();
        let furnace = FurnaceBlockEntity {
            burn_remaining: 10,
            burn_total: 10,
            ..FurnaceBlockEntity::default()
        };
        world
            .set_furnace_block_entity(pos, furnace.clone())
            .unwrap();

        assert_eq!(read_view.furnace_snapshots(&[cpos]), vec![(pos, furnace)]);

        world.set_block_at(pos, BlockStateId(1)).unwrap();

        assert!(read_view.furnace_snapshots(&[cpos]).is_empty());
    }

    #[test]
    fn hopper_block_entities_are_chunk_scoped_runtime_state() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        let mut hopper = world.hopper_block_entity(pos).unwrap().unwrap();
        assert!(hopper.slots[0].is_empty());
        hopper.slots[4] = crate::chunk::FurnaceSlot {
            count: 2,
            item_id: 42,
            damage: Some(7),
            enchantments: Vec::new(),
        };

        assert!(world.set_hopper_block_entity(pos, hopper.clone()).unwrap());
        assert_eq!(world.dirty_count(), 1);
        assert_eq!(world.hopper_block_entity(pos).unwrap(), Some(hopper));
    }

    #[test]
    fn scheduled_block_ticks_are_chunk_scoped_runtime_state() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        assert!(
            world
                .schedule_block_tick(ScheduledBlockTick::new(pos, block.clone(), 20, 0))
                .unwrap()
        );
        assert_eq!(world.dirty_count(), 1);
        assert_eq!(
            world.scheduled_block_ticks(cpos).unwrap().unwrap()[0].block,
            block
        );

        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
        assert!(
            world
                .drain_due_block_ticks(cpos, 19, usize::MAX)
                .unwrap()
                .is_empty()
        );
        assert_eq!(world.dirty_count(), 0);

        let due = world.drain_due_block_ticks(cpos, 20, usize::MAX).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].pos, pos);
        assert_eq!(world.dirty_count(), 1);

        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
        assert!(
            world
                .schedule_block_tick(ScheduledBlockTick::new(pos, block, 30, 0))
                .unwrap()
        );
        let removed = world.remove_scheduled_block_ticks_at(pos).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(
            world
                .scheduled_block_ticks(cpos)
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn scheduled_tick_view_tracks_block_queue_changes() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let scheduled_ticks = world.scheduled_tick_view();

        assert!(!scheduled_ticks.block_due(cpos, 20));
        assert!(
            world
                .schedule_block_tick(ScheduledBlockTick::new(pos, block, 20, 0))
                .unwrap()
        );
        assert!(!scheduled_ticks.block_due(cpos, 19));
        assert!(scheduled_ticks.block_due(cpos, 20));

        assert_eq!(
            world
                .drain_due_block_ticks(cpos, 20, usize::MAX)
                .unwrap()
                .len(),
            1
        );
        assert!(!scheduled_ticks.block_due(cpos, u64::MAX));
    }

    #[test]
    fn scheduled_tick_view_flags_hopper_without_tick_for_backfill() {
        let registry = air_stone_hopper_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let scheduled_ticks = world.scheduled_tick_view();

        world.set_block_at(pos, BlockStateId(2)).unwrap();
        world
            .set_hopper_block_entity(pos, HopperBlockEntity::default())
            .unwrap();

        assert!(scheduled_ticks.block_due(cpos, 0));
    }

    #[test]
    fn scheduled_tick_view_tracks_fluid_queue_changes() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        let scheduled_ticks = world.scheduled_tick_view();

        assert!(
            world
                .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 30, 0))
                .unwrap()
        );
        assert!(!scheduled_ticks.fluid_due(cpos, 29));
        assert!(scheduled_ticks.fluid_due(cpos, 30));

        assert_eq!(world.remove_scheduled_fluid_ticks_at(pos).unwrap().len(), 1);
        assert!(!scheduled_ticks.fluid_due(cpos, u64::MAX));
    }

    #[test]
    fn scheduled_fluid_ticks_are_chunk_scoped_runtime_state() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        assert!(
            world
                .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid.clone(), 20, 0))
                .unwrap()
        );
        assert_eq!(world.dirty_count(), 1);
        assert_eq!(
            world.scheduled_fluid_ticks(cpos).unwrap().unwrap()[0].fluid,
            fluid
        );

        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
        assert!(
            world
                .drain_due_fluid_ticks(cpos, 19, usize::MAX)
                .unwrap()
                .is_empty()
        );
        assert_eq!(world.dirty_count(), 0);

        let due = world.drain_due_fluid_ticks(cpos, 20, usize::MAX).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].pos, pos);
        assert_eq!(world.dirty_count(), 1);

        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;
        assert!(
            world
                .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 30, 0))
                .unwrap()
        );
        let removed = world.remove_scheduled_fluid_ticks_at(pos).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(
            world
                .scheduled_fluid_ticks(cpos)
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cached_due_tick_drain_does_not_invalidate_dirty_shared_chunk_without_due_ticks() {
        let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
        let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        assert!(
            world
                .schedule_block_tick(ScheduledBlockTick::new(pos, block, 20, 0))
                .unwrap()
        );
        assert!(
            world
                .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 20, 0))
                .unwrap()
        );
        let _shared = world.cached_chunk_snapshot(cpos).unwrap();
        let before = world.resident.snapshot(cpos).unwrap().dirty_generation;

        assert!(
            world
                .drain_due_cached_block_ticks(cpos, 19, usize::MAX)
                .is_empty()
        );
        assert!(
            world
                .drain_due_cached_fluid_ticks(cpos, 19, usize::MAX)
                .is_empty()
        );

        assert_eq!(
            world.resident.snapshot(cpos).unwrap().dirty_generation,
            before
        );
    }

    #[test]
    fn furnace_block_entity_survives_flush_and_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let air = mc_data::blocks::BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        };
        let registry = Arc::new(BlockRegistry::from_report(&[air]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:raw_iron").unwrap(),
                protocol_id: 10,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:coal").unwrap(),
                protocol_id: 11,
            },
        ]));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        let mut furnace = FurnaceBlockEntity {
            burn_remaining: 1200,
            burn_total: 1600,
            cook_progress: 37,
            cook_total: 200,
            ..FurnaceBlockEntity::default()
        };
        furnace.slots[0] = crate::chunk::FurnaceSlot {
            count: 1,
            item_id: 10,
            damage: None,
            enchantments: Vec::new(),
        };
        furnace.slots[1] = crate::chunk::FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_furnace_block_entity(pos, furnace.clone())
            .unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);

        let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
            .unwrap()
            .with_item_registry(items);
        assert_eq!(fresh.furnace_block_entity(pos).unwrap(), Some(furnace));
    }

    #[test]
    fn chest_block_entity_survives_flush_and_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let air = mc_data::blocks::BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        };
        let registry = Arc::new(BlockRegistry::from_report(&[air]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
                protocol_id: 10,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
                protocol_id: 11,
            },
        ]));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = crate::chunk::FurnaceSlot {
            count: 64,
            item_id: 10,
            damage: None,
            enchantments: Vec::new(),
        };
        chest.slots[26] = crate::chunk::FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
            enchantments: Vec::new(),
        };
        world.set_chest_block_entity(pos, chest.clone()).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);

        let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
            .unwrap()
            .with_item_registry(items);
        assert_eq!(fresh.chest_block_entity(pos).unwrap(), Some(chest));
    }

    #[test]
    fn hopper_block_entity_survives_flush_and_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = air_stone_hopper_registry();
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
                protocol_id: 10,
            },
            mc_data::items::ItemReport {
                id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
                protocol_id: 11,
            },
        ]));
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
            .unwrap()
            .with_item_registry(Arc::clone(&items));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(pos, BlockStateId(2)).unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().dirty = false;

        let mut hopper = crate::chunk::HopperBlockEntity {
            transfer_cooldown: 6,
            ..Default::default()
        };
        hopper.slots[0] = crate::chunk::FurnaceSlot {
            count: 64,
            item_id: 10,
            damage: None,
            enchantments: Vec::new(),
        };
        hopper.slots[4] = crate::chunk::FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
            enchantments: Vec::new(),
        };
        world.set_hopper_block_entity(pos, hopper.clone()).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);

        let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
            .unwrap()
            .with_item_registry(items);
        assert_eq!(fresh.hopper_block_entity(pos).unwrap(), Some(hopper));
    }

    #[test]
    fn replacing_chest_block_prunes_stale_block_entity() {
        let registry = air_stone_chest_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(pos, BlockStateId(2)).unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = crate::chunk::FurnaceSlot {
            count: 1,
            item_id: 10,
            damage: None,
            enchantments: Vec::new(),
        };
        world.set_chest_block_entity(pos, chest).unwrap();

        world.set_block_at(pos, BlockStateId(1)).unwrap();

        let chunk = world.resident.snapshot(cpos).unwrap();
        assert!(!chunk.chests.contains_key(&pos));
    }

    #[test]
    fn replacing_hopper_block_prunes_stale_block_entity() {
        let registry = air_stone_hopper_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 2, z: 3 };
        let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(pos, BlockStateId(2)).unwrap();
        let mut hopper = crate::chunk::HopperBlockEntity::default();
        hopper.slots[0] = crate::chunk::FurnaceSlot {
            count: 1,
            item_id: 10,
            damage: None,
            enchantments: Vec::new(),
        };
        world.set_hopper_block_entity(pos, hopper).unwrap();

        world.set_block_at(pos, BlockStateId(1)).unwrap();

        let chunk = world.resident.snapshot(cpos).unwrap();
        assert!(!chunk.hoppers.contains_key(&pos));
    }

    /// End-to-end: open the generated flat test world, query known
    /// coordinates of the local test world, assert out-of-range /
    /// missing chunks return None instead of erroring, and confirm the
    /// LRU stays bounded. The oracle may be the old vanilla flat world
    /// or a Solaris-generated terrain world.
    #[test]
    fn opens_real_test_world_and_queries_blocks() {
        let world_dir = workspace_path(".analysis/test-world");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !world_dir.is_dir() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = match WorldStorage::open_with_capacity(&world_dir, Arc::clone(&registry), 4)
        {
            Ok(world) => world,
            Err(err) => {
                eprintln!("skipping: {} ({err})", world_dir.display());
                return;
            }
        };

        let resolve = |w: &WorldStorage, id: BlockStateId| {
            w.registry()
                .by_id(id)
                .unwrap()
                .block
                .id
                .as_str()
                .to_string()
        };

        let air_id = air_state_id(&registry);
        let top_y = top_non_air_y(&mut world, 0, 0, air_id).expect("origin column has terrain");
        let top = world
            .get_block(BlockPos {
                x: 0,
                y: top_y,
                z: 0,
            })
            .unwrap()
            .unwrap();
        let air_above = world
            .get_block(BlockPos {
                x: 0,
                y: top_y + 1,
                z: 0,
            })
            .unwrap()
            .unwrap();
        assert_ne!(top, air_id, "top terrain block must not be air");
        assert_eq!(resolve(&world, air_above), "minecraft:air");

        // Out-of-range Y returns None gracefully.
        assert_eq!(
            world
                .get_block(BlockPos {
                    x: 0,
                    y: 1000,
                    z: 0
                })
                .unwrap(),
            None
        );
        // A chunk in a region that doesn't exist on disk returns
        // None, not an error.
        assert_eq!(
            world
                .get_block(BlockPos {
                    x: 100_000,
                    y: 0,
                    z: 0,
                })
                .unwrap(),
            None
        );

        // LRU stays bounded across many lookups.
        for x in 0..50 {
            let _ = world.get_block(BlockPos { x, y: -64, z: 0 }).unwrap();
        }
        assert!(world.cache_len() <= 4);
    }

    /// Walking 121 chunks of one region must leave exactly one entry
    /// in the region cache regardless of chunk-LRU thrash. This is
    /// the structural M3.f assertion: without the region cache the
    /// equivalent path re-opened `r.0.0.mca` 121 times.
    #[test]
    fn region_cache_holds_one_region_across_quadrant_walk() {
        let world_dir = workspace_path(".analysis/test-world");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !world_dir.is_dir() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = match WorldStorage::open_with_capacity(&world_dir, registry, 4) {
            Ok(world) => world,
            Err(err) => {
                eprintln!("skipping: {} ({err})", world_dir.display());
                return;
            }
        };

        for cz in 0..=10 {
            for cx in 0..=10 {
                let _ = world.get_chunk(ChunkPos { x: cx, z: cz }).unwrap();
            }
        }
        // Chunk LRU still capped at 4. Region LRU now holds exactly
        // the one region those chunks live in.
        assert!(world.cache_len() <= 4);
        assert_eq!(world.region_cache_len(), 1);
    }

    /// M6.b: a dirty chunk in the cache is flushed to its `.mca`
    /// when `flush_dirty` is called, and the flush survives a fresh
    /// `WorldStorage::open` (i.e. the next read picks it up from
    /// disk, not from the in-memory cache).
    #[test]
    fn flush_dirty_writes_modified_chunks_to_disk() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen {
            stone: BlockStateId,
        }

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let air = BlockStateId(0);
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, air, biome);
                chunk.set_block(3, 0, 5, self.stone);
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:dirt").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen {
                    stone: BlockStateId(1),
                }));

        let stone_id = registry
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .map(|b| b.default)
            .unwrap();
        let dirt_id = registry
            .block(&Identifier::parse("minecraft:dirt").unwrap())
            .map(|b| b.default)
            .unwrap();
        let edit_pos = BlockPos { x: 3, y: 0, z: 5 };
        let current = world.get_block(edit_pos).unwrap().unwrap();
        let new_state = if current == stone_id {
            dirt_id
        } else {
            stone_id
        };
        let prev = world.set_block_at(edit_pos, new_state).unwrap().unwrap();
        assert_ne!(prev, new_state, "test world cell must change state");
        assert_eq!(world.dirty_count(), 1);

        let n_flushed = world.flush_dirty().unwrap();
        assert_eq!(n_flushed, 1);
        assert_eq!(world.dirty_count(), 0);

        // Drop the in-memory world and re-open fresh — proves the
        // edit landed on disk, not just in the LRU.
        drop(world);
        let mut world2 =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
        let after = world2.get_block(edit_pos).unwrap().unwrap();
        assert_eq!(after, new_state);
    }

    #[test]
    fn section_light_arrays_survive_flush_and_reopen() {
        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let block_light = (0..crate::chunk::LIGHT_LAYER_BYTES)
            .map(|index| (index & 0xFF) as u8)
            .collect::<Vec<_>>();
        let sky_light = (0..crate::chunk::LIGHT_LAYER_BYTES)
            .map(|index| 255u8.wrapping_sub((index & 0xFF) as u8))
            .collect::<Vec<_>>();

        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.section_lights[0].block = Some(block_light.clone());
        chunk.section_lights[0].sky = Some(sky_light.clone());
        chunk.section_lights[4].sky = Some(block_light.clone());
        chunk.mark_dirty();

        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();
        assert_eq!(world.dirty_count(), 1);

        assert_eq!(world.flush_dirty().unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
        drop(world);

        let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
        let chunk = reopened.get_chunk(cpos).unwrap().unwrap();

        assert_eq!(
            chunk.section_lights[0].block.as_deref(),
            Some(&block_light[..])
        );
        assert_eq!(chunk.section_lights[0].sky.as_deref(), Some(&sky_light[..]));
        assert_eq!(chunk.section_lights[1].block, None);
        assert_eq!(chunk.section_lights[1].sky, None);
        assert_eq!(chunk.section_lights[4].block, None);
        assert_eq!(
            chunk.section_lights[4].sky.as_deref(),
            Some(&block_light[..])
        );
    }

    #[test]
    fn unknown_root_extras_survive_world_storage_edit_flush_reopen() {
        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let extras = vec![
            ("DataVersion".into(), Tag::Int(4444)),
            ("InhabitedTime".into(), Tag::Long(123_456)),
            ("structures".into(), Tag::Compound(Vec::new())),
        ];

        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.extras = extras.clone();
        chunk.mark_dirty();

        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);
        drop(world);

        let mut edited =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        edited
            .set_block_at(BlockPos { x: 1, y: 0, z: 1 }, BlockStateId(1))
            .unwrap();
        assert_eq!(edited.flush_dirty().unwrap(), 1);
        drop(edited);

        let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
        let chunk = reopened.get_chunk(cpos).unwrap().unwrap();
        assert_eq!(chunk.extras, extras);
        assert_eq!(chunk.get_block(1, 0, 1).unwrap(), BlockStateId(1));
    }

    #[test]
    fn world_journal_lsn_survives_anvil_flush_and_reopen() {
        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = single_air_registry();
        let position = ChunkPos { x: -1, z: 2 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(position, BlockStateId(0), biome);
        chunk.set_world_journal_lsn(73);

        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(position, chunk).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);
        drop(world);

        let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
        assert_eq!(
            reopened
                .get_chunk(position)
                .unwrap()
                .unwrap()
                .world_journal_lsn(),
            73
        );
    }

    #[test]
    fn journaled_resident_batch_stamps_complete_sorted_touched_footprint() {
        let registry = Arc::new(
            BlockRegistry::from_report(&[
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 0,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
                mc_data::blocks::BlockReport {
                    id: Identifier::parse("minecraft:oak_leaves").unwrap(),
                    properties: std::collections::BTreeMap::new(),
                    states: vec![mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: true,
                        properties: std::collections::BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        );
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let biome = Identifier::parse("minecraft:plains").unwrap();
        for x in 0..=2 {
            let position = ChunkPos { x, z: 0 };
            world
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        let direct = BlockPos { x: 1, y: 1, z: 1 };
        let requested = BlockPos { x: 31, y: 1, z: 1 };
        let leaf = BlockPos { x: 32, y: 1, z: 1 };
        world.set_block_at(direct, BlockStateId(1)).unwrap();
        world.set_block_at(requested, BlockStateId(1)).unwrap();
        world.set_block_at(leaf, BlockStateId(2)).unwrap();
        let direct_token = world.block_mutation_token(direct).unwrap();
        let requested_token = world.block_mutation_token(requested).unwrap();

        let (result, touched) = world
            .mutation_view()
            .apply_block_edits_conditionally_journaled(
                91,
                &[
                    crate::ResidentBlockEdit {
                        pos: direct,
                        new_state: BlockStateId(0),
                        preserve_light: false,
                    },
                    crate::ResidentBlockEdit {
                        pos: requested,
                        new_state: BlockStateId(0),
                        preserve_light: false,
                    },
                ],
                &[
                    crate::ResidentBlockPrecondition {
                        pos: direct,
                        expected_state: BlockStateId(1),
                        expected_token: direct_token,
                    },
                    crate::ResidentBlockPrecondition {
                        pos: requested,
                        expected_state: BlockStateId(1),
                        expected_token: requested_token,
                    },
                ],
                &[ScheduledBlockTick::new(
                    requested,
                    Identifier::parse("minecraft:air").unwrap(),
                    20,
                    0,
                )],
                None,
                Some(12),
            );

        assert!(matches!(
            result,
            crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 2
        ));
        assert_eq!(
            touched,
            vec![
                ChunkPos { x: 0, z: 0 },
                ChunkPos { x: 1, z: 0 },
                ChunkPos { x: 2, z: 0 },
            ]
        );
        for position in touched {
            assert_eq!(
                world
                    .cached_chunk_snapshot(position)
                    .unwrap()
                    .world_journal_lsn(),
                91
            );
        }
        assert_eq!(
            world
                .scheduled_block_ticks(ChunkPos { x: 1, z: 0 })
                .unwrap()
                .unwrap(),
            &[ScheduledBlockTick::new(
                requested,
                Identifier::parse("minecraft:air").unwrap(),
                20,
                0,
            )]
        );
        assert_eq!(
            world
                .scheduled_block_ticks(ChunkPos { x: 2, z: 0 })
                .unwrap()
                .unwrap(),
            &[ScheduledBlockTick::new(
                leaf,
                Identifier::parse("minecraft:oak_leaves").unwrap(),
                12,
                0,
            )]
        );
    }

    #[test]
    fn dirty_flush_plan_excludes_journal_pending_chunk() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();

        let (result, touched) = world
            .mutation_view()
            .apply_block_edits_conditionally_journaled(
                41,
                &[crate::ResidentBlockEdit {
                    pos: BlockPos { x: 1, y: 1, z: 1 },
                    new_state: BlockStateId(1),
                    preserve_light: false,
                }],
                &[],
                &[],
                None,
                None,
            );

        assert!(matches!(
            result,
            crate::ResidentBlockEditBatchResult::Applied(ref applied) if applied.len() == 1
        ));
        assert_eq!(touched, vec![position]);
        assert!(world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(world.dirty_count(), 1);
    }

    #[test]
    fn clearing_journal_pending_chunk_makes_it_flushable() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mutation_view = world.mutation_view();
        let (_, touched) = mutation_view.apply_block_edits_conditionally_journaled(
            42,
            &[crate::ResidentBlockEdit {
                pos: BlockPos { x: 1, y: 1, z: 1 },
                new_state: BlockStateId(1),
                preserve_light: false,
            }],
            &[],
            &[],
            None,
            None,
        );
        let notifications = Arc::new(AtomicUsize::new(0));
        world.set_dirty_high_water_notifier({
            let notifications = Arc::clone(&notifications);
            Arc::new(move || {
                notifications.fetch_add(1, Ordering::SeqCst);
            })
        });

        assert!(world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(
            mutation_view.clear_journal_pending_conditionally(42, &touched),
            1
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert_eq!(
            mutation_view.clear_journal_pending_conditionally(41, &touched),
            0
        );
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        assert_eq!(world.plan_dirty_flush().unwrap().chunks, 1);
        assert_eq!(world.dirty_count(), 1);
    }

    #[test]
    fn stale_journal_completion_cannot_clear_newer_pending_lsn() {
        let registry = air_stone_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 0, z: 0 };
        let block = BlockPos { x: 1, y: 1, z: 1 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mutation_view = world.mutation_view();
        let (_, first_touched) = mutation_view.apply_block_edits_conditionally_journaled(
            50,
            &[crate::ResidentBlockEdit {
                pos: block,
                new_state: BlockStateId(1),
                preserve_light: false,
            }],
            &[],
            &[],
            None,
            None,
        );
        let (_, second_touched) = mutation_view.apply_block_edits_conditionally_journaled(
            51,
            &[crate::ResidentBlockEdit {
                pos: block,
                new_state: BlockStateId(0),
                preserve_light: false,
            }],
            &[],
            &[],
            None,
            None,
        );

        assert_eq!(
            mutation_view.clear_journal_pending_conditionally(50, &first_touched),
            0
        );
        assert!(world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(
            mutation_view.clear_journal_pending_conditionally(51, &second_touched),
            1
        );
        assert_eq!(world.plan_dirty_flush().unwrap().chunks, 1);
    }

    #[test]
    fn coordinator_journal_stamp_fences_flush_until_exact_clear() {
        let registry = single_air_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mutation = world.mutation_view();

        let crate::JournalStampResult::Stamped(snapshots) =
            world.stamp_cached_chunks_for_world_journal(7, &[position])
        else {
            panic!("cached chunk accepts coordinator journal stamp");
        };

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].world_journal_lsn(), 7);
        assert!(world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(
            mutation.clear_journal_pending_conditionally(7, &[position]),
            1
        );
        assert_eq!(world.plan_dirty_flush().unwrap().chunks, 1);
    }

    #[test]
    fn coordinator_journal_stamp_never_decreases_lsn() {
        let registry = single_air_registry();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        let position = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                position,
                Chunk::empty(
                    position,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        assert!(matches!(
            world.stamp_cached_chunks_for_world_journal(12, &[position]),
            crate::JournalStampResult::Stamped(_)
        ));

        assert!(matches!(
            world.stamp_cached_chunks_for_world_journal(11, &[position]),
            crate::JournalStampResult::NewerDecision(12)
        ));
        assert_eq!(
            world
                .cached_chunk_snapshot(position)
                .unwrap()
                .world_journal_lsn(),
            12
        );
    }

    #[test]
    fn journal_replay_applies_only_images_newer_than_disk_or_resident_chunk() {
        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let position = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut durable = Chunk::empty(position, BlockStateId(0), biome.clone());
        durable.set_block(1, 0, 1, BlockStateId(1));
        durable.set_world_journal_lsn(10);

        let mut initial =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        initial.insert_chunk(position, durable).unwrap();
        assert_eq!(initial.flush_dirty().unwrap(), 1);
        drop(initial);

        let mut reopened =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        let mut equal = Chunk::empty(position, BlockStateId(0), biome.clone());
        equal.set_world_journal_lsn(10);
        assert!(!reopened.replay_journal_chunk(equal).unwrap());
        assert_eq!(
            reopened.get_block(BlockPos { x: 1, y: 0, z: 1 }).unwrap(),
            Some(BlockStateId(1))
        );

        let mut older = Chunk::empty(position, BlockStateId(0), biome.clone());
        older.set_world_journal_lsn(9);
        assert!(!reopened.replay_journal_chunk(older).unwrap());

        let mut newer = Chunk::empty(position, BlockStateId(0), biome);
        newer.set_world_journal_lsn(11);
        assert!(reopened.replay_journal_chunk(newer).unwrap());
        let replayed = reopened.cached_chunk_snapshot(position).unwrap();
        assert_eq!(replayed.world_journal_lsn(), 11);
        assert_eq!(replayed.get_block(1, 0, 1), Some(BlockStateId(0)));
        assert!(replayed.dirty);
    }

    #[test]
    fn visit_existing_chunks_without_generation_scans_disk_without_cache_mutation() {
        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let cpos = ChunkPos { x: -1, z: 32 };
        let biome = Identifier::parse("minecraft:plains").unwrap();

        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.set_block(15, 0, 0, BlockStateId(1));
        chunk.mark_dirty();

        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);
        drop(world);

        let reopened =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        assert_eq!(reopened.stats().chunk_cache_len, 0);
        assert_eq!(reopened.stats().region_cache_len, 0);

        let mut visited = Vec::new();
        let count = reopened
            .visit_existing_chunks_without_generation(|pos, chunk| {
                visited.push((pos, chunk.get_block(15, 0, 0).unwrap()));
            })
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(visited, vec![(cpos, BlockStateId(1))]);
        assert_eq!(reopened.stats().chunk_cache_len, 0);
        assert_eq!(reopened.stats().region_cache_len, 0);
    }

    #[test]
    fn dirty_flush_uses_unique_region_tmp_without_clobbering_stale_fixed_tmp() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen;

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
                chunk.status = "minecraft:full".into();
                chunk.dirty = true;
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        let region_dir = tmp_world.path().join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let region_path = region_dir.join("r.0.0.mca");
        let tmp_path = region_path.with_extension("mca.tmp");
        let stale_tmp = b"interrupted previous flush";
        std::fs::write(&tmp_path, stale_tmp).unwrap();

        let report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen));

        assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
        assert_eq!(world.flush_dirty().unwrap(), 1);

        assert!(region_path.is_file());
        assert_eq!(std::fs::read(&tmp_path).unwrap(), stale_tmp);
        assert_eq!(read_region(&region_path).unwrap().len(), 1);
    }

    #[test]
    fn region_replace_rejects_stale_expected_version() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen;

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
                chunk.status = "minecraft:full".into();
                chunk.dirty = true;
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        let region_dir = tmp_world.path().join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let region_path = region_dir.join("r.0.0.mca");
        let report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16)
            .unwrap()
            .with_generator(Arc::new(StubGen));
        assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
        assert_eq!(world.flush_dirty().unwrap(), 1);

        let expected = region_file_version(&region_path).unwrap();
        let payloads = read_region(&region_path).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&region_path)
            .unwrap();
        use std::io::Write as _;
        file.write_all(&[0]).unwrap();

        let Err(WorldError::StaleRegion(path)) =
            replace_region_file(&region_path, &payloads, expected.as_ref())
        else {
            panic!("stale region version must reject replacement");
        };
        assert_eq!(path, region_path);
    }

    #[test]
    fn existing_region_install_rechecks_stale_target_before_rename() {
        use std::io::Write as _;

        let tmp_world = tempfile::tempdir().unwrap();
        let region_path = tmp_world.path().join("r.0.0.mca");
        let tmp_path = tmp_world.path().join(".r.0.0.mca.tmp");
        std::fs::write(&region_path, b"old region").unwrap();
        std::fs::write(&tmp_path, b"planned replacement").unwrap();
        let expected = region_file_version(&region_path).unwrap();

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&region_path)
            .unwrap();
        file.write_all(b" changed").unwrap();

        let Err(WorldError::StaleRegion(path)) =
            install_existing_region_file(&region_path, &tmp_path, expected.as_ref())
        else {
            panic!("existing-region install must reject a stale target before rename");
        };

        assert_eq!(path, region_path);
        assert_eq!(std::fs::read(&region_path).unwrap(), b"old region changed");
        assert!(!tmp_path.exists());
    }

    #[test]
    fn dirty_flush_write_rejects_region_changed_after_planning() {
        use crate::chunk::ChunkGenerator;
        use std::io::Write as _;

        struct StubGen;

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
                chunk.set_block(0, 0, 0, BlockStateId(1));
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        let region_dir = tmp_world.path().join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let region_path = region_dir.join("r.0.0.mca");
        let registry = air_stone_registry();

        let mut initial =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen));
        assert!(
            initial
                .get_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .is_some()
        );
        assert_eq!(initial.flush_dirty().unwrap(), 1);

        let mut stale = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
        stale
            .set_block_at(BlockPos { x: 0, y: 0, z: 0 }, BlockStateId(0))
            .unwrap()
            .unwrap();
        let plan = stale.plan_dirty_flush().unwrap();
        assert_eq!(plan.chunk_count(), 1);

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&region_path)
            .unwrap();
        file.write_all(&[0]).unwrap();

        let Err(WorldError::StaleRegion(path)) = plan.write() else {
            panic!("flush write must reject a region changed after planning");
        };
        assert_eq!(path, region_path);
        assert_eq!(stale.dirty_count(), 1);
    }

    #[test]
    fn sync_dirty_flush_replans_when_competing_writer_creates_region() {
        use crate::chunk::ChunkGenerator;

        struct StubGen;

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, BlockStateId(0), biome);
                chunk.set_block(0, 0, 0, BlockStateId(1));
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen));

        assert!(world.get_chunk(ChunkPos { x: 0, z: 0 }).unwrap().is_some());
        assert_eq!(world.dirty_count(), 1);
        let mut competing_plan = Some(world.plan_dirty_flush().unwrap());

        let flushed = world
            .flush_dirty_at_tick_with_pre_write_hook(0, |_| {
                if let Some(plan) = competing_plan.take() {
                    let commit = plan.write().unwrap();
                    assert_eq!(commit.regions.len(), 1);
                }
            })
            .unwrap();

        assert_eq!(flushed, 1);
        assert_eq!(world.dirty_count(), 0);

        let mut reopened =
            WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
        assert_eq!(
            reopened.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
            Some(BlockStateId(1))
        );
    }

    #[test]
    fn new_region_install_rejects_concurrent_create() {
        let tmp_world = tempfile::tempdir().unwrap();
        let region_path = tmp_world.path().join("r.0.0.mca");
        let tmp_path = tmp_world.path().join(".r.0.0.mca.tmp");
        std::fs::write(&tmp_path, b"planned replacement").unwrap();
        std::fs::write(&region_path, b"concurrent region").unwrap();

        let Err(WorldError::StaleRegion(path)) = install_new_region_file(&region_path, &tmp_path)
        else {
            panic!("new-region install must reject a concurrently created target");
        };

        assert_eq!(path, region_path);
        assert_eq!(std::fs::read(&region_path).unwrap(), b"concurrent region");
        assert!(!tmp_path.exists());
    }

    #[test]
    fn dirty_flush_commit_preserves_chunks_changed_after_planning() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen {
            stone: BlockStateId,
        }

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let air = BlockStateId(0);
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, air, biome);
                chunk.set_block(3, 0, 5, self.stone);
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:dirt").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen {
                    stone: BlockStateId(1),
                }));

        let edit_pos = BlockPos { x: 3, y: 0, z: 5 };
        world.get_block(edit_pos).unwrap().unwrap();
        world
            .set_block_at(edit_pos, BlockStateId(2))
            .unwrap()
            .unwrap();
        let plan = world.plan_dirty_flush().unwrap();
        assert_eq!(plan.chunk_count(), 1);

        world
            .set_block_at(edit_pos, BlockStateId(1))
            .unwrap()
            .unwrap();
        let commit = plan.write().unwrap();
        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
        assert_eq!(world.dirty_count(), 1);

        let mut fresh =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
        assert_eq!(fresh.get_block(edit_pos).unwrap(), Some(BlockStateId(2)));

        assert_eq!(world.flush_dirty().unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
        let mut fresh = WorldStorage::open_with_capacity(tmp_world.path(), registry, 16).unwrap();
        assert_eq!(fresh.get_block(edit_pos).unwrap(), Some(BlockStateId(1)));
    }

    #[test]
    fn dirty_flush_does_not_overwrite_newer_region_with_stale_cached_snapshot() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen {
            state: BlockStateId,
        }

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let air = BlockStateId(0);
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, air, biome);
                chunk.set_block(0, 0, 0, self.state);
                chunk.status = "minecraft:full".into();
                chunk.dirty = true;
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:dirt").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 2,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());

        let mut initial =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen {
                    state: BlockStateId(1),
                }));
        assert!(
            initial
                .get_chunk(ChunkPos { x: 0, z: 0 })
                .unwrap()
                .is_some()
        );
        assert_eq!(initial.flush_dirty().unwrap(), 1);

        let mut stale_cached =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
        assert_eq!(
            stale_cached
                .get_block(BlockPos { x: 0, y: 0, z: 0 })
                .unwrap(),
            Some(BlockStateId(1))
        );

        let mut newer =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16)
                .unwrap()
                .with_generator(Arc::new(StubGen {
                    state: BlockStateId(1),
                }));
        assert!(newer.get_chunk(ChunkPos { x: 1, z: 0 }).unwrap().is_some());
        assert_eq!(newer.flush_dirty().unwrap(), 1);

        stale_cached
            .set_block_at(BlockPos { x: 0, y: 0, z: 0 }, BlockStateId(2))
            .unwrap()
            .unwrap();
        assert_eq!(stale_cached.flush_dirty().unwrap(), 1);

        let mut fresh =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 16).unwrap();
        assert_eq!(
            fresh.get_block(BlockPos { x: 16, y: 0, z: 0 }).unwrap(),
            Some(BlockStateId(1))
        );
        assert_eq!(
            fresh.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
            Some(BlockStateId(2))
        );
    }

    #[test]
    fn get_chunk_mut_does_not_mark_read_like_access_dirty() {
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut world = WorldStorage::in_memory(Arc::clone(&registry));
        world
            .insert_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();

        let chunk = world.get_chunk_mut(cpos).unwrap().unwrap();

        assert_eq!(chunk.dirty_generation, 0);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_plan_tracks_retained_snapshot_token_without_payload_encoding() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let snapshot = world.resident.snapshot(cpos).unwrap();

        assert!(Arc::ptr_eq(&planned.snapshot, &snapshot));
        assert_eq!(planned.snapshot_token, chunk_snapshot_token(&snapshot));
    }

    #[test]
    fn bounded_dirty_flush_plan_commits_one_batch_and_leaves_remainder() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let mut world = WorldStorage::open_with_capacity(temp.path(), registry, 3).unwrap();
        let biome = Identifier::parse("minecraft:plains").unwrap();
        for x in 0..3 {
            let position = ChunkPos { x, z: 0 };
            world
                .insert_generated_chunk(
                    position,
                    Chunk::empty(position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }

        let plan = world.plan_dirty_flush_at_tick_bounded(17, 2).unwrap();
        assert_eq!(plan.chunk_count(), 2);
        assert_eq!(world.commit_dirty_flush(plan.write().unwrap()).unwrap(), 2);
        assert_eq!(world.dirty_count(), 1);

        let remainder = world.plan_dirty_flush_at_tick_bounded(18, 2).unwrap();
        assert_eq!(remainder.chunk_count(), 1);
        assert_eq!(
            world
                .commit_dirty_flush(remainder.write().unwrap())
                .unwrap(),
            1
        );
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_plan_clones_snapshots_without_encoding_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let payload_encode_count = plan.payload_encode_counter();

        assert_eq!(
            payload_encode_count.load(Ordering::Relaxed),
            0,
            "dirty flush planning should only clone snapshots while the world lock is held"
        );

        let commit = plan.write().unwrap();

        assert_eq!(payload_encode_count.load(Ordering::Relaxed), 1);
        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_write_carries_retained_snapshot_fast_path_metadata_into_commit() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let expected_snapshot = Arc::clone(&planned.snapshot);
        let expected_snapshot_token = planned.snapshot_token;
        let commit = plan.write().unwrap();
        let committed = &commit.regions[0].chunks[0];

        assert!(Arc::ptr_eq(&committed.snapshot, &expected_snapshot));
        assert_eq!(committed.snapshot_token, expected_snapshot_token);
        assert_eq!(
            committed.payload_digest,
            payload_digest(&committed.uncompressed_nbt)
        );
    }

    #[test]
    fn dirty_flush_fast_path_requires_matching_snapshot_generation_and_identity() {
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let snapshot = Arc::new(chunk);

        assert!(can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation,
            &snapshot,
        ));
        assert!(!can_fast_clean_chunk(&snapshot, 0, &snapshot,));

        let other_snapshot = Arc::new((*snapshot).clone());
        assert!(!can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation,
            &other_snapshot,
        ));
        assert!(!can_fast_clean_chunk(
            &snapshot,
            snapshot.dirty_generation + 1,
            &snapshot,
        ));
    }

    #[test]
    fn dirty_flush_mutable_fork_after_plan_bumps_generation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let planned_generation = planned.dirty_generation;
        let planned_snapshot = Arc::clone(&planned.snapshot);

        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .set_block(1, 0, 1, BlockStateId(1));

        let live_snapshot = world.resident.snapshot(cpos).unwrap();
        assert!(live_snapshot.dirty_generation > planned_generation);
        assert!(!Arc::ptr_eq(&live_snapshot, &planned_snapshot));
        assert_eq!(
            planned_snapshot.get_block(1, 0, 1).unwrap(),
            BlockStateId(0)
        );
        assert_eq!(live_snapshot.get_block(1, 0, 1).unwrap(), BlockStateId(1));
        assert!(!can_fast_clean_chunk(
            &live_snapshot,
            planned_generation,
            &planned_snapshot,
        ));
    }

    #[test]
    fn dirty_flush_mutable_alias_after_plan_invalidates_planned_generation() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned_generation = plan.regions[0].dirty_payloads[0].dirty_generation;

        let _chunk = world.get_chunk_mut(cpos).unwrap().unwrap();
        assert!(
            world.resident.snapshot(cpos).unwrap().dirty_generation > planned_generation,
            "mutable access after dirty flush planning must invalidate the planned generation"
        );

        let commit = plan.write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
        assert_eq!(world.dirty_count(), 1);
    }

    #[test]
    fn dirty_flush_commit_cleans_unchanged_nonzero_generation_snapshot_fast_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let planned_generation = planned.dirty_generation;
        let planned_snapshot = Arc::clone(&planned.snapshot);
        let commit = plan.write().unwrap();

        assert_ne!(planned_generation, 0);
        assert!(can_fast_clean_chunk(
            &world.resident.snapshot(cpos).unwrap(),
            planned_generation,
            &planned_snapshot,
        ));
        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_commit_fast_path_clears_without_copying_unchanged_chunk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let before_token = chunk_snapshot_token(&world.resident.snapshot(cpos).unwrap());
        let commit = plan.write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);

        let live = world.resident.snapshot(cpos).unwrap();
        assert!(!live.dirty);
        assert_eq!(chunk_snapshot_token(&live), before_token);
    }

    #[test]
    fn dirty_flush_commit_falls_back_to_payload_compare_for_defensive_snapshot_change() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let table =
            BlockLightTable::from_arrays("test", vec![0, 0], vec![0, 15], vec![true, false]);
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.set_block(1, 0, 1, BlockStateId(1));
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let planned_generation = planned.dirty_generation;
        let planned_snapshot = Arc::clone(&planned.snapshot);

        let mut fork = (*world.resident.snapshot(cpos).unwrap()).clone();
        fork.update_highest_opaque_column(1, 1, &table);
        world.resident.replace_for_test(cpos, Arc::new(fork));

        let live_snapshot = world.resident.snapshot(cpos).unwrap();
        let commit = plan.write().unwrap();

        assert_eq!(live_snapshot.dirty_generation, planned_generation);
        assert!(!Arc::ptr_eq(&live_snapshot, &planned_snapshot));
        assert_eq!(planned_snapshot.highest_opaque_y(1, 1), None);
        assert_eq!(live_snapshot.highest_opaque_y(1, 1), Some(0));
        assert!(!can_fast_clean_chunk(
            &live_snapshot,
            planned_generation,
            &planned_snapshot,
        ));
        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_flush_commit_keeps_matching_nonzero_generation_dirty_on_payload_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let table =
            BlockLightTable::from_arrays("test", vec![0, 0], vec![0, 15], vec![true, false]);
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.set_block(1, 0, 1, BlockStateId(1));
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned = &plan.regions[0].dirty_payloads[0];
        let planned_generation = planned.dirty_generation;
        let planned_snapshot = Arc::clone(&planned.snapshot);
        let mut fork = (*world.resident.snapshot(cpos).unwrap()).clone();
        fork.update_highest_opaque_column(1, 1, &table);
        world.resident.replace_for_test(cpos, Arc::new(fork));
        let live_snapshot = world.resident.snapshot(cpos).unwrap();
        let mut commit = plan.write().unwrap();
        commit.regions[0].chunks[0].uncompressed_nbt.clear();

        assert_eq!(live_snapshot.dirty_generation, planned_generation);
        assert!(!can_fast_clean_chunk(
            &live_snapshot,
            planned_generation,
            &planned_snapshot,
        ));
        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
        assert_eq!(world.dirty_count(), 1);
    }

    #[test]
    fn dirty_flush_commit_keeps_post_plan_unmarked_chunk_mutation_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = air_stone_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let planned_generation = world.resident.snapshot(cpos).unwrap().dirty_generation;
        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .set_block(1, 0, 1, BlockStateId(1));
        assert!(world.resident.snapshot(cpos).unwrap().dirty_generation > planned_generation);

        let commit = plan.write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
        assert_eq!(world.dirty_count(), 1);
        assert_eq!(
            world.get_block(BlockPos { x: 1, y: 0, z: 1 }).unwrap(),
            Some(BlockStateId(1))
        );
    }

    #[test]
    fn dirty_flush_commit_keeps_nonzero_generation_mismatch_dirty_even_if_payload_matches() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.mark_dirty();
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        world.get_chunk_mut(cpos).unwrap().unwrap().mark_dirty();
        let matching_payload = crate::anvil::chunk_to_payload_with_items(
            &world.resident.snapshot(cpos).unwrap(),
            &registry,
            world.item_registry.as_deref(),
            0,
        )
        .unwrap()
        .uncompressed_nbt;
        let mut commit = plan.write().unwrap();
        commit.regions[0].chunks[0].uncompressed_nbt = matching_payload;

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 0);
        assert_eq!(world.dirty_count(), 1);
    }

    #[test]
    fn dirty_flush_commit_uses_payload_fallback_for_legacy_zero_generation_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = single_air_registry();
        let cpos = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(cpos, BlockStateId(0), biome);
        chunk.dirty = true;
        let mut world =
            WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4).unwrap();
        world.insert_chunk(cpos, chunk).unwrap();

        let plan = world.plan_dirty_flush().unwrap();
        let matching_payload = crate::anvil::chunk_to_payload_with_items(
            &world.resident.snapshot(cpos).unwrap(),
            &registry,
            world.item_registry.as_deref(),
            0,
        )
        .unwrap()
        .uncompressed_nbt;
        let mut commit = plan.write().unwrap();
        commit.regions[0].chunks[0].uncompressed_nbt = matching_payload;

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);
        assert_eq!(world.dirty_count(), 0);
    }

    #[test]
    fn dirty_lru_eviction_does_not_flush_under_insert() {
        use crate::chunk::ChunkGenerator;
        use mc_data::Identifier;

        struct StubGen;

        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let air = BlockStateId(0);
                let biome = Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = Chunk::empty(pos, air, biome);
                chunk.set_block(0, 0, 0, BlockStateId(1));
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
                chunk
            }
        }

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 1,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            },
        ];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = WorldStorage::open_with_capacity(tmp_world.path(), registry, 1)
            .unwrap()
            .with_generator(Arc::new(StubGen));

        assert_eq!(
            world.get_block(BlockPos { x: 0, y: 0, z: 0 }).unwrap(),
            Some(BlockStateId(1))
        );
        assert_eq!(world.dirty_count(), 1);

        assert_eq!(
            world.get_block(BlockPos { x: 16, y: 0, z: 0 }).unwrap(),
            Some(BlockStateId(1))
        );

        assert_eq!(world.cache_len(), 2);
        assert_eq!(world.dirty_count(), 2);
        assert!(!tmp_world.path().join("region/r.0.0.mca").exists());
    }

    #[test]
    fn fluid_state_and_scheduled_tick_survive_flush_and_reopen() {
        use mc_data::Identifier;
        use std::collections::BTreeMap;

        let tmp_world = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();
        let mut water_properties = BTreeMap::new();
        water_properties.insert("level".to_string(), vec!["0".to_string(), "1".to_string()]);
        let report = vec![
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:water").unwrap(),
                properties: water_properties,
                states: vec![
                    mc_data::blocks::BlockStateReport {
                        id: 1,
                        default: true,
                        properties: BTreeMap::from([("level".to_string(), "0".to_string())]),
                    },
                    mc_data::blocks::BlockStateReport {
                        id: 2,
                        default: false,
                        properties: BTreeMap::from([("level".to_string(), "1".to_string())]),
                    },
                ],
            },
        ];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let cpos = ChunkPos { x: 0, z: 0 };
        let pos = BlockPos { x: 1, y: 64, z: 1 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let water = Identifier::parse("minecraft:water").unwrap();
        let mut world =
            WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&registry), 4).unwrap();
        world
            .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
            .unwrap();
        world.set_block_at(pos, BlockStateId(1)).unwrap();
        assert!(
            world
                .schedule_fluid_tick(ScheduledFluidTick::new(pos, water.clone(), 12, 0))
                .unwrap()
        );

        assert_eq!(world.flush_dirty().unwrap(), 1);
        drop(world);

        let mut reopened = WorldStorage::open_with_capacity(tmp_world.path(), registry, 4).unwrap();
        assert_eq!(reopened.get_block(pos).unwrap(), Some(BlockStateId(1)));
        let ticks = reopened.scheduled_fluid_ticks(cpos).unwrap().unwrap();
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].pos, pos);
        assert_eq!(ticks[0].fluid, water);
        assert_eq!(ticks[0].trigger_tick, 12);
    }

    /// M6.b: the spawn-burst load path (read 121 chunks) must not
    /// produce any dirty chunks (chunks decoded from disk start
    /// clean). This guards against an accidental `dirty = true`
    /// default that would turn the burst into an I/O storm.
    #[test]
    fn spawn_burst_load_does_not_dirty_chunks() {
        let world_dir = workspace_path(".analysis/test-world");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !world_dir.is_dir() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = match WorldStorage::open_with_capacity(&world_dir, registry, 4) {
            Ok(world) => world,
            Err(err) => {
                eprintln!("skipping: {} ({err})", world_dir.display());
                return;
            }
        };
        for cz in 0..=10 {
            for cx in 0..=10 {
                let _ = world.get_chunk(ChunkPos { x: cx, z: cz }).unwrap();
            }
        }
        assert_eq!(world.dirty_count(), 0);
    }

    /// M7.c: a `WorldStorage` opened on a path *without* region
    /// files but with a generator attached resolves every chunk
    /// position to a non-empty `Chunk`.
    #[test]
    fn worldgen_fallback_fills_missing_chunks() {
        use crate::chunk::ChunkGenerator;

        let tmp = tempfile::tempdir().unwrap();
        // Create the expected directory layout without populating it.
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();

        // Stub generator: every chunk is a single grass block at the
        // origin column. Enough to assert "we hit the generator".
        struct StubGen;
        impl ChunkGenerator for StubGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                let mut c = Chunk::empty(
                    pos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                );
                c.set_block(0, 0, 0, BlockStateId(42));
                c.dirty = true;
                c
            }
        }

        // The stub registry has only "air" but generator emits
        // BlockStateId(42) directly; the registry isn't consulted on
        // the read path for raw state ids.
        let report = vec![mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        let mut world = WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 4)
            .unwrap()
            .with_generator(Arc::new(StubGen));

        let cpos = ChunkPos { x: 999, z: -999 };
        let chunk = world.get_chunk(cpos).unwrap().expect("generator ran");
        assert_eq!(chunk.get_block(0, 0, 0), Some(BlockStateId(42)));
        assert!(chunk.dirty);
    }

    #[test]
    fn chunk_lookup_without_generation_does_not_run_generator() {
        use crate::chunk::ChunkGenerator;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingGen {
            calls: Arc<AtomicUsize>,
        }

        impl ChunkGenerator for CountingGen {
            fn generate(&self, pos: ChunkPos) -> Chunk {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Chunk::empty(
                    pos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                )
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let registry = Arc::new(
            BlockRegistry::from_report(&[mc_data::blocks::BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: std::collections::BTreeMap::new(),
                states: vec![mc_data::blocks::BlockStateReport {
                    id: 0,
                    default: true,
                    properties: std::collections::BTreeMap::new(),
                }],
            }])
            .unwrap(),
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let mut world = WorldStorage::open_with_capacity(tmp.path(), registry, 4)
            .unwrap()
            .with_generator(Arc::new(CountingGen {
                calls: Arc::clone(&calls),
            }));

        assert!(
            world
                .get_chunk_without_generation(ChunkPos { x: 4, z: 4 })
                .unwrap()
                .is_none()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        assert!(world.get_chunk(ChunkPos { x: 4, z: 4 }).unwrap().is_some());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Bench-style coverage of the M3.e load pattern: stream the
    /// bottom-right quadrant of the view-distance ring (chunks 0..=10
    /// in both axes — the slice of vd=10 around spawn that exists in
    /// the test world's only region file). The point is to measure
    /// the time-to-stream so the M3.f region-cache lands with a
    /// before/after number rather than a guess.
    #[test]
    fn streams_view_distance_quadrant_within_budget() {
        let world_dir = workspace_path(".analysis/test-world");
        let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
        if !world_dir.is_dir() || !blocks_path.is_file() {
            eprintln!("skipping: prerequisites missing");
            return;
        }
        let report = mc_data::blocks::load_blocks_report(&blocks_path).unwrap();
        let registry = Arc::new(BlockRegistry::from_report(&report).unwrap());
        // Match the production chunk-LRU default. M3.e thrashes this
        // at vd=10 because the LRU only holds 16 of the 121 hot
        // chunks; the region cache is what de-amortises it.
        let mut world = match WorldStorage::open(&world_dir, registry) {
            Ok(world) => world,
            Err(err) => {
                eprintln!("skipping: {} ({err})", world_dir.display());
                return;
            }
        };

        for cz in 0..=10 {
            for cx in 0..=10 {
                if world
                    .get_chunk_without_generation(ChunkPos { x: cx, z: cz })
                    .unwrap()
                    .is_none()
                {
                    eprintln!(
                        "skipping: {} does not contain required vd=10 chunk ({cx}, {cz})",
                        world_dir.display()
                    );
                    return;
                }
            }
        }

        let started = std::time::Instant::now();
        let mut hit = 0usize;
        for cz in 0..=10 {
            for cx in 0..=10 {
                let chunk = world.get_chunk(ChunkPos { x: cx, z: cz }).unwrap();
                assert!(chunk.is_some(), "chunk ({cx}, {cz}) missing from r.0.0.mca");
                hit += 1;
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "vd-quadrant stream: {hit} chunks in {ms} ms ({per_chunk_us} us/chunk)",
            ms = elapsed.as_millis(),
            per_chunk_us = elapsed.as_micros() as f64 / hit as f64,
        );
        // Generous ceiling. With no region cache and chunk-LRU=16
        // this typically sits around 1–2 s; once the M3.f region
        // cache lands it should drop to tens of ms. Set the cap at
        // 10 s so the test still fails loudly on a 10× regression
        // but doesn't flake on a contended CI runner.
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "vd-quadrant stream took {elapsed:?} — suspicious regression",
        );
    }
}
