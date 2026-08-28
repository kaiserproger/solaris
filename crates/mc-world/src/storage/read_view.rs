use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use mc_data::items::ItemRegistry;

use crate::anvil::chunk_from_payload_with_items_at_position;
use crate::anvil::region::read_chunk;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, Chunk, ChunkPos, FurnaceBlockEntity};
use crate::section::SECTION_DIM;

use super::{
    DecodedRegion, REGION_AXIS_CHUNKS, WorldError, WorldStorage, chunk_pos_of,
    make_cached_chunk_mut, region_of,
};

const READ_VIEW_REGION_AXIS_CHUNKS: i32 = 8;
const READ_VIEW_SHARD_COUNT: usize = 64;

pub type ChunkSnapshot = Arc<Chunk>;

type FurnaceSnapshotsByChunk = HashMap<ChunkPos, Arc<HashMap<BlockPos, FurnaceBlockEntity>>>;
type PublishedChunkShard = RwLock<HashMap<ChunkPos, ChunkSnapshot>>;
type FurnaceSnapshotShard = RwLock<FurnaceSnapshotsByChunk>;
pub type DirtyHighWaterNotifier = Arc<dyn Fn() + Send + Sync + 'static>;

pub(crate) struct ResidentPublicationState {
    mutation_gate: RwLock<()>,
    generation: AtomicU64,
    fail_stopped: AtomicBool,
}

impl ResidentPublicationState {
    fn new() -> Self {
        Self {
            mutation_gate: RwLock::new(()),
            generation: AtomicU64::new(0),
            fail_stopped: AtomicBool::new(false),
        }
    }

    pub(crate) fn mutation(&self) -> RwLockReadGuard<'_, ()> {
        let guard = self
            .mutation_gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            !self.fail_stopped.load(Ordering::Acquire),
            "resident publication is fail-stopped"
        );
        guard
    }

    pub(crate) fn transaction(&self) -> RwLockWriteGuard<'_, ()> {
        let guard = self
            .mutation_gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            !self.fail_stopped.load(Ordering::Acquire),
            "resident publication is fail-stopped"
        );
        guard
    }

    pub(crate) fn read_consistent<R>(&self, read: impl Fn() -> R) -> R {
        let _admission = self.mutation();
        let generation = self.generation.load(Ordering::Acquire);
        assert_eq!(
            generation & 1,
            0,
            "admitted resident reader saw odd generation"
        );
        let value = read();
        assert_eq!(
            self.generation.load(Ordering::Acquire),
            generation,
            "resident publication changed under reader admission"
        );
        value
    }

    pub(crate) fn begin_publish<'a>(
        &'a self,
        transaction: RwLockWriteGuard<'a, ()>,
    ) -> ResidentPublishGuard<'a> {
        let previous = self.generation.fetch_add(1, Ordering::AcqRel);
        assert_eq!(previous & 1, 0, "resident publication cannot nest");
        ResidentPublishGuard {
            state: self,
            _transaction: transaction,
            completed: false,
        }
    }
}

#[must_use = "a resident publication must be completed explicitly"]
/// Dropping without completion fail-stops the state before releasing reader
/// admission, so no caller can observe a partial publication.
pub(crate) struct ResidentPublishGuard<'a> {
    state: &'a ResidentPublicationState,
    _transaction: RwLockWriteGuard<'a, ()>,
    completed: bool,
}

impl ResidentPublishGuard<'_> {
    pub(crate) fn complete(mut self) {
        let previous = self.state.generation.load(Ordering::Acquire);
        assert_eq!(previous & 1, 1, "resident publication generation is odd");
        self.state
            .generation
            .store(previous.wrapping_add(1), Ordering::Release);
        self.completed = true;
    }
}

impl Drop for ResidentPublishGuard<'_> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.state.fail_stopped.store(true, Ordering::Release);
        let generation = self.state.generation.load(Ordering::Acquire);
        if generation & 1 == 1 {
            self.state
                .generation
                .store(generation.wrapping_add(1), Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorldSpawn {
    pub block_x: i32,
    pub block_z: i32,
}

impl WorldSpawn {
    #[must_use]
    pub const fn new(block_x: i32, block_z: i32) -> Self {
        Self { block_x, block_z }
    }

    #[must_use]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.block_x.div_euclid(16),
            z: self.block_z.div_euclid(16),
        }
    }
}

#[derive(Clone)]
pub struct WorldReadView {
    chunks: Arc<[PublishedChunkShard; READ_VIEW_SHARD_COUNT]>,
    furnaces: Arc<[FurnaceSnapshotShard; READ_VIEW_SHARD_COUNT]>,
    resident_chunks: Arc<AtomicUsize>,
    dirty_chunks: Arc<AtomicUsize>,
    capacity: usize,
    dirty_saturated: Arc<AtomicBool>,
    dirty_high_water_notifier: Arc<RwLock<Option<DirtyHighWaterNotifier>>>,
    publication: Arc<ResidentPublicationState>,
    spawn: Arc<RwLock<WorldSpawn>>,
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

#[derive(Clone)]
pub struct ScheduledTickView {
    chunks: Arc<RwLock<HashMap<ChunkPos, ScheduledTickHint>>>,
    publication: Arc<ResidentPublicationState>,
}

#[derive(Clone, Copy, Default)]
struct ScheduledTickHint {
    next_block_tick: Option<u64>,
    next_fluid_tick: Option<u64>,
    hopper_backfill_required: bool,
}

impl WorldReadView {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            chunks: Arc::new(std::array::from_fn(|_| RwLock::new(HashMap::new()))),
            furnaces: Arc::new(std::array::from_fn(|_| RwLock::new(HashMap::new()))),
            resident_chunks: Arc::new(AtomicUsize::new(0)),
            dirty_chunks: Arc::new(AtomicUsize::new(0)),
            capacity: capacity.max(1),
            dirty_saturated: Arc::new(AtomicBool::new(false)),
            dirty_high_water_notifier: Arc::new(RwLock::new(None)),
            publication: Arc::new(ResidentPublicationState::new()),
            spawn: Arc::new(RwLock::new(WorldSpawn::default())),
        }
    }

    pub(crate) fn publication_state(&self) -> Arc<ResidentPublicationState> {
        Arc::clone(&self.publication)
    }

    #[must_use]
    pub fn spawn(&self) -> WorldSpawn {
        *self
            .spawn
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn set_spawn(&self, spawn: WorldSpawn) {
        *self
            .spawn
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = spawn;
    }

    #[must_use]
    pub fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.publication.read_consistent(|| {
            let cpos = chunk_pos_of(pos);
            let chunks = self.chunks[read_view_shard(cpos)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let chunk = chunks.get(&cpos)?;
            let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            chunk.get_block(local_x, pos.y, local_z)
        })
    }

    #[must_use]
    pub fn block_mutation_token(&self, pos: BlockPos) -> Option<crate::BlockMutationToken> {
        self.publication.read_consistent(|| {
            let cpos = chunk_pos_of(pos);
            let chunks = self.chunks[read_view_shard(cpos)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let chunk = chunks.get(&cpos)?;
            let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            chunk.block_mutation_token(local_x, pos.y, local_z)
        })
    }

    #[must_use]
    pub fn block_mutation_snapshot(
        &self,
        pos: BlockPos,
    ) -> Option<(BlockStateId, crate::BlockMutationToken)> {
        self.publication.read_consistent(|| {
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
        })
    }

    #[must_use]
    pub fn snapshot_chunks(&self, positions: &[ChunkPos]) -> WorldReadSnapshot {
        self.publication.read_consistent(|| {
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
        })
    }

    #[must_use]
    pub fn furnace_snapshots(&self, positions: &[ChunkPos]) -> Vec<(BlockPos, FurnaceBlockEntity)> {
        self.publication.read_consistent(|| {
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
        })
    }

    /// Report whether a new chunk can enter the cache without waiting for the
    /// mutable storage owner. The final insert rechecks the same condition.
    #[must_use]
    pub fn can_cache_new_chunk(&self, position: ChunkPos) -> bool {
        self.publication.read_consistent(|| {
            if !self.dirty_saturated.load(Ordering::Acquire) {
                return true;
            }
            let chunks = self.chunks[read_view_shard(position)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            chunks.contains_key(&position)
        })
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

    pub(super) fn remove_furnaces(&self, position: ChunkPos) {
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
        self.publication
            .read_consistent(|| self.resident_chunks.load(Ordering::Acquire))
    }

    pub(crate) fn dirty_len(&self) -> usize {
        self.publication
            .read_consistent(|| self.dirty_chunks.load(Ordering::Acquire))
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
    pub(super) fn lock_chunk_shard_for_test(
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

impl ScheduledTickView {
    pub(super) fn with_publication(publication: Arc<ResidentPublicationState>) -> Self {
        Self {
            chunks: Arc::new(RwLock::new(HashMap::new())),
            publication,
        }
    }
}

impl Default for ScheduledTickView {
    fn default() -> Self {
        Self::with_publication(Arc::new(ResidentPublicationState::new()))
    }
}

impl ChunkSourceView {
    #[must_use]
    pub fn source_for(&self, position: ChunkPos) -> ChunkPrepareSource {
        self.resident.publication.read_consistent(|| {
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
        })
    }
}

impl ScheduledTickView {
    #[must_use]
    pub fn block_due(&self, position: ChunkPos, world_tick: u64) -> bool {
        self.publication.read_consistent(|| {
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
        })
    }

    #[must_use]
    pub fn fluid_due(&self, position: ChunkPos, world_tick: u64) -> bool {
        self.publication.read_consistent(|| {
            self.chunks
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&position)
                .and_then(|hint| hint.next_fluid_tick)
                .is_some_and(|trigger_tick| trigger_tick <= world_tick)
        })
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
    pub fn contains_chunk(&self, position: ChunkPos) -> bool {
        self.chunks.contains_key(&position)
    }

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

pub enum ChunkSnapshotPlan {
    Cached(ChunkSnapshot),
    Load(ChunkDiskLoadPlan),
}

pub struct ChunkDiskLoadPlan {
    expected_pos: ChunkPos,
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
            read_chunk(&self.region_path, self.local.0, self.local.1)?
        } else {
            None
        };

        let Some(payload) = payload else {
            return Ok(None);
        };
        chunk_from_payload_with_items_at_position(
            &payload.uncompressed_nbt,
            self.expected_pos,
            &self.registry,
            self.item_registry.as_deref(),
        )
        .map(Some)
        .map_err(WorldError::from)
    }
}

impl WorldStorage {
    #[must_use]
    pub fn read_view(&self) -> WorldReadView {
        self.read_view.clone()
    }

    /// Install the server-owned push boundary for dirty-cache high water.
    pub fn set_dirty_high_water_notifier(&self, notifier: DirtyHighWaterNotifier) {
        self.read_view.set_dirty_high_water_notifier(notifier);
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

    pub fn plan_chunk_snapshot_without_generation(&self, cpos: ChunkPos) -> ChunkSnapshotPlan {
        if let Some(chunk) = self.resident.snapshot(cpos) {
            return ChunkSnapshotPlan::Cached(chunk);
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        ChunkSnapshotPlan::Load(ChunkDiskLoadPlan {
            expected_pos: cpos,
            local: (local_x, local_z),
            region_path: self.region_root.join(format!("r.{rx}.{rz}.mca")),
            disk_backed: self.world_root.is_some(),
            cached_region: self.regions.get(&(rx, rz)).cloned(),
            registry: Arc::clone(&self.registry),
            item_registry: self.item_registry.clone(),
        })
    }
}

#[cfg(test)]
#[path = "read_view_tests.rs"]
mod tests;
