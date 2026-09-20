//! Lazy world storage on top of the Anvil codec.
//!
//! Opens a vanilla world directory (the one containing
//! `dimensions/minecraft/overworld/region/` or, on older saves,
//! `region/` directly), and serves block queries by loading the
//! covering region file on demand. Chunk and indexed-region LRUs keep
//! recent data resident; dirty chunks are flushed back through region
//! planning/write/commit paths.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};

use thiserror::Error;

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;

use crate::anvil::region::RegionReader;
use crate::anvil::{ChunkNbtError, RegionError, chunk_from_payload_with_items_at_position};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockPos, ChestBlockEntity, Chunk, ChunkGenerator, ChunkPos, FurnaceBlockEntity,
    HopperBlockEntity, ScheduledBlockTick, ScheduledFluidTick,
};
use crate::light::ChunkLight;
use crate::resident::{ResidentChunkStore, WorldMutationView};
use crate::section::SECTION_DIM;

#[cfg(test)]
mod admission_tests;
mod block_edits;
mod budget;
mod dirty_flush;
mod read_view;
#[cfg(test)]
mod test_support;
mod world_lease;

use world_lease::{WorldRootLease, acquire_world_root_lease};

pub use dirty_flush::{
    DirtyFlushCommit, DirtyFlushFinalize, DirtyFlushInstall, DirtyFlushPlan, DirtyFlushSynced,
    JournalBarrier,
};
pub(crate) use read_view::ResidentPublicationState;
pub use read_view::{
    ChunkDiskLoadPlan, ChunkPrepareSource, ChunkSnapshot, ChunkSnapshotPlan, ChunkSourceView,
    DirtyHighWaterNotifier, ScheduledTickView, WorldReadSnapshot, WorldReadView, WorldSpawn,
};

const REGION_AXIS_CHUNKS: i32 = 32;
const DEFAULT_LRU_CAPACITY: usize = 16;
/// How many region indexes and open `.mca` handles we retain at once.
/// Compressed payloads are read by slot; decoded NBT is never cached.
const DEFAULT_REGION_LRU_CAPACITY: usize = 4;
const DEFAULT_RESIDENT_BYTES_PER_CHUNK: usize = 8 * 1024 * 1024;
const DEFAULT_FLUSH_RESERVE_BYTES: usize = 16 * 1024 * 1024;

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
    #[error("writable world root is already leased: {root}; holder metadata: {metadata}")]
    WorldLocked { root: PathBuf, metadata: String },
    #[error("world lease {operation} failed at {path}: {source}")]
    WorldLeaseIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("world was opened read-only and cannot be flushed: {0}")]
    ReadOnlyWorld(PathBuf),
    #[error("journal durability barrier failed: {0}")]
    JournalBarrier(#[source] std::io::Error),
    #[error(
        "chunk cache pressure: request {requested_bytes} bytes, resident {resident_bytes}/{resident_budget}, dirty {dirty_bytes}/{dirty_budget}, save_healthy={save_healthy}"
    )]
    ChunkCachePressure {
        requested_bytes: usize,
        resident_bytes: usize,
        resident_budget: usize,
        dirty_bytes: usize,
        dirty_budget: usize,
        save_healthy: bool,
    },
    #[error("invalid chunk-cache byte budgets: resident={resident_budget}, dirty={dirty_budget}")]
    InvalidChunkCacheBudgets {
        resident_budget: usize,
        dirty_budget: usize,
    },
    #[error(
        "chunk position mismatch: expected ({expected_x},{expected_z}), held ({actual_x},{actual_z})"
    )]
    ChunkPositionMismatch {
        expected_x: i32,
        expected_z: i32,
        actual_x: i32,
        actual_z: i32,
    },
    #[error(
        "resident chunks kept changing during dirty flush after {attempts} attempts; {remaining_dirty} chunks remain dirty"
    )]
    ResidentChangedDuringFlush {
        attempts: usize,
        remaining_dirty: usize,
    },
    #[error(
        "dirty flush captured {dirty_chunks} dirty chunks but only {flushable_chunks} were journal-ready"
    )]
    JournalPendingDirtyChunks {
        dirty_chunks: usize,
        flushable_chunks: usize,
    },
}

fn ensure_chunk_position(expected: ChunkPos, actual: ChunkPos) -> Result<(), WorldError> {
    if expected != actual {
        return Err(WorldError::ChunkPositionMismatch {
            expected_x: expected.x,
            expected_z: expected.z,
            actual_x: actual.x,
            actual_z: actual.z,
        });
    }
    Ok(())
}

fn default_chunk_byte_budgets(capacity: usize) -> (usize, usize) {
    let resident = capacity
        .max(1)
        .saturating_mul(DEFAULT_RESIDENT_BYTES_PER_CHUNK);
    let reserve = DEFAULT_FLUSH_RESERVE_BYTES.min(resident / 4);
    (resident, resident.saturating_sub(reserve).max(1))
}

/// Handle to a world's chunk data, generated chunks, and dirty flush state.
pub struct WorldStorage {
    world_root: Option<PathBuf>,
    _world_lease: Option<Arc<WorldRootLease>>,
    read_only: bool,
    dirty_flush_cursor: Option<ChunkPos>,
    journal_barrier: Option<JournalBarrier>,
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
    resident_byte_budget: usize,
    dirty_byte_budget: usize,
    save_healthy: bool,
    /// LRU of validated region indexes and open files. Each chunk miss reads
    /// and decompresses only its own slot, without retaining raw NBT alongside
    /// the resident decoded chunks.
    regions: HashMap<(i32, i32), Arc<RegionReader>>,
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
    pub resident_bytes: usize,
    pub resident_byte_budget: usize,
    pub region_cache_len: usize,
    pub region_cache_capacity: usize,
    pub dirty_chunks: usize,
    pub dirty_bytes: usize,
    pub dirty_byte_budget: usize,
    pub save_healthy: bool,
    pub dirty_chunk_cache_saturated: bool,
}

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

pub(crate) fn make_cached_chunk_mut(chunk: &mut ChunkSnapshot) -> &mut Chunk {
    let invalidate_planned_flush = chunk.dirty && Arc::strong_count(chunk) > 1;
    let chunk = Arc::make_mut(chunk);
    if invalidate_planned_flush {
        chunk.mark_dirty();
    }
    chunk
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

    /// Open a world for tooling that performs no persistent mutation.
    /// Read-only opens do not acquire the process-wide writable lease.
    pub fn open_read_only(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacities_mode(
            world_dir,
            registry,
            DEFAULT_LRU_CAPACITY,
            DEFAULT_REGION_LRU_CAPACITY,
            true,
        )
    }

    pub fn open_with_capacity(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        capacity: usize,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacities(world_dir, registry, capacity, DEFAULT_REGION_LRU_CAPACITY)
    }

    pub fn open_with_capacities(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        capacity: usize,
        region_capacity: usize,
    ) -> Result<Self, WorldError> {
        Self::open_with_capacities_mode(world_dir, registry, capacity, region_capacity, false)
    }

    fn open_with_capacities_mode(
        world_dir: impl AsRef<Path>,
        registry: Arc<BlockRegistry>,
        capacity: usize,
        region_capacity: usize,
        read_only: bool,
    ) -> Result<Self, WorldError> {
        let dir = world_dir.as_ref();
        if !dir.is_dir() {
            return Err(WorldError::Missing(dir.to_path_buf()));
        }
        let world_lease = if read_only {
            None
        } else {
            Some(acquire_world_root_lease(dir)?)
        };
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
        let (resident_byte_budget, dirty_byte_budget) = default_chunk_byte_budgets(capacity);
        let read_view = WorldReadView::with_capacity(capacity);
        let scheduled_tick_view =
            ScheduledTickView::with_publication(read_view.publication_state());
        let resident = ResidentChunkStore::new(
            read_view.clone(),
            scheduled_tick_view.clone(),
            Arc::clone(&registry),
        );
        Ok(Self {
            world_root: Some(dir.to_path_buf()),
            _world_lease: world_lease,
            read_only,
            dirty_flush_cursor: None,
            journal_barrier: None,
            region_root,
            registry,
            resident,
            read_view,
            scheduled_tick_view,
            lru: VecDeque::new(),
            capacity,
            resident_byte_budget,
            dirty_byte_budget,
            save_healthy: true,
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
        let (resident_byte_budget, dirty_byte_budget) = default_chunk_byte_budgets(capacity);
        let read_view = WorldReadView::with_capacity(capacity);
        let scheduled_tick_view =
            ScheduledTickView::with_publication(read_view.publication_state());
        let resident = ResidentChunkStore::new(
            read_view.clone(),
            scheduled_tick_view.clone(),
            Arc::clone(&registry),
        );
        Self {
            world_root: None,
            _world_lease: None,
            read_only: false,
            dirty_flush_cursor: None,
            journal_barrier: None,
            region_root: PathBuf::new(),
            registry,
            resident,
            read_view,
            scheduled_tick_view,
            lru: VecDeque::new(),
            capacity,
            resident_byte_budget,
            dirty_byte_budget,
            save_healthy: true,
            regions: HashMap::new(),
            region_lru: VecDeque::new(),
            region_capacity: DEFAULT_REGION_LRU_CAPACITY,
            item_registry: None,
            generator: None,
            generator_available: Arc::new(AtomicBool::new(false)),
            borrowed_chunk: None,
        }
    }

    pub(crate) fn ensure_writable(&self) -> Result<(), WorldError> {
        if self.read_only {
            return Err(WorldError::ReadOnlyWorld(
                self.world_root.clone().unwrap_or_default(),
            ));
        }
        Ok(())
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

    #[must_use]
    pub fn with_spawn(self, spawn: WorldSpawn) -> Self {
        self.read_view.set_spawn(spawn);
        self
    }

    #[must_use]
    pub fn spawn(&self) -> WorldSpawn {
        self.read_view.spawn()
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
    pub fn mutation_view(&self) -> WorldMutationView {
        self.resident.mutation_view()
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

    /// Report whether a chunk is resident or already stored on disk, without
    /// loading its payload and without invoking the fallback generator.
    pub fn chunk_is_stored(&mut self, cpos: ChunkPos) -> Result<bool, WorldError> {
        if self.resident.contains(cpos) {
            return Ok(true);
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        match self.ensure_region(rx, rz)? {
            Some(region) => Ok(region.has_chunk(local_x, local_z)),
            None => Ok(false),
        }
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

    pub fn commit_chunk_snapshot(
        &mut self,
        cpos: ChunkPos,
        chunk: Chunk,
    ) -> Result<ChunkSnapshot, WorldError> {
        ensure_chunk_position(cpos, chunk.pos)?;
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
        self.prepare_new_chunk_admission(position, &chunk)?;
        self.resident.replace(position, Arc::new(chunk));
        self.lru.retain(|cached| *cached != position);
        self.lru.push_back(position);
        let region = (position.x.div_euclid(32), position.z.div_euclid(32));
        self.regions.remove(&region);
        self.region_lru.retain(|cached| *cached != region);
        Ok(true)
    }

    pub fn try_commit_chunk_snapshot(
        &mut self,
        cpos: ChunkPos,
        chunk: Chunk,
    ) -> Result<Option<ChunkSnapshot>, WorldError> {
        if !self.can_cache_new_chunk(cpos) {
            return Ok(None);
        }
        // The optimistic probe cannot account for retained clean entries or
        // the incoming chunk's exact size. Actual admission may still defer.
        match self.commit_chunk_snapshot(cpos, chunk) {
            Ok(chunk) => Ok(Some(chunk)),
            Err(WorldError::ChunkCachePressure { .. }) => Ok(None),
            Err(err) => Err(err),
        }
    }

    #[must_use]
    pub fn can_cache_new_chunk(&self, cpos: ChunkPos) -> bool {
        if self.resident.contains(cpos) {
            return true;
        }
        let (resident_bytes, dirty_bytes) = self.chunk_byte_usage();
        let clean_evictable = self.resident.dirty_count() < self.resident.len();
        (clean_evictable
            || self.resident.len() < self.capacity && resident_bytes < self.resident_byte_budget)
            && dirty_bytes < self.dirty_byte_budget
            && (self.save_healthy || dirty_bytes == 0)
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
                let same_block_type = chunk
                    .get_block(local_x, pos.y, local_z)
                    .is_some_and(|previous| registry.same_block_type(previous, state));
                let prev = if clear_baked_light {
                    chunk.set_block_and_update(local_x, pos.y, local_z, state, air, same_block_type)
                } else {
                    chunk.set_block_and_update_preserving_light(
                        local_x,
                        pos.y,
                        local_z,
                        state,
                        air,
                        same_block_type,
                    )
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
        match self.insert_chunk(cpos, chunk) {
            Ok(()) => Ok(true),
            Err(WorldError::ChunkCachePressure { .. }) => Ok(false),
            Err(err) => Err(err),
        }
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

        let payload = match self.ensure_region(rx, rz)? {
            Some(region) => region.read_chunk(local_x, local_z)?,
            None => None,
        };

        if let Some(payload) = payload {
            let chunk = chunk_from_payload_with_items_at_position(
                &payload.uncompressed_nbt,
                cpos,
                &self.registry,
                self.item_registry.as_deref(),
            )?;
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
    /// its validated index and open file. Returns `None` when the underlying
    /// `.mca` file doesn't exist on disk.
    fn ensure_region(&mut self, rx: i32, rz: i32) -> Result<Option<&RegionReader>, WorldError> {
        let key = (rx, rz);
        if self.regions.contains_key(&key) {
            self.touch_region(key);
        } else {
            let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
            if !region_path.is_file() {
                return Ok(None);
            }
            let reader = Arc::new(RegionReader::open(&region_path)?);
            self.insert_region(key, reader);
        }
        Ok(self.regions.get(&key).map(Arc::as_ref))
    }

    fn insert_region(&mut self, key: (i32, i32), region: Arc<RegionReader>) {
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
        ensure_chunk_position(cpos, chunk.pos)?;
        if self.resident.contains(cpos) {
            self.touch(cpos);
            return Ok(());
        }
        self.prepare_new_chunk_admission(cpos, &chunk)?;
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
            if self.read_view.chunk_is_retained(evict)
                || self
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

    /// How many validated region indexes and open files are currently cached.
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
        let (resident_bytes, dirty_bytes) = self.chunk_byte_usage();
        WorldStorageStats {
            chunk_cache_len: self.resident.len(),
            chunk_cache_capacity: self.capacity,
            resident_bytes,
            resident_byte_budget: self.resident_byte_budget,
            region_cache_len: self.regions.len(),
            region_cache_capacity: self.region_capacity,
            dirty_chunks: self.dirty_count(),
            dirty_bytes,
            dirty_byte_budget: self.dirty_byte_budget,
            save_healthy: self.save_healthy,
            dirty_chunk_cache_saturated: self.dirty_chunk_cache_saturated(),
        }
    }

    #[must_use]
    pub fn dirty_chunk_cache_saturated(&self) -> bool {
        let (resident_bytes, dirty_bytes) = self.chunk_byte_usage();
        (self.resident.len() >= self.capacity && self.resident.dirty_count() == self.resident.len())
            || resident_bytes >= self.resident_byte_budget
            || dirty_bytes >= self.dirty_byte_budget
            || (!self.save_healthy && dirty_bytes != 0)
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
    let added_chest = if keeps_chest && !chunk.chests.contains_key(&pos) {
        chunk.chests.insert(pos, ChestBlockEntity::default());
        true
    } else {
        false
    };
    let removed_chest = !keeps_chest && chunk.chests.remove(&pos).is_some();
    let removed_furnace = !keeps_furnace && chunk.furnaces.remove(&pos).is_some();
    let removed_hopper = !keeps_hopper && chunk.hoppers.remove(&pos).is_some();
    let removed_opaque = !keeps_opaque && chunk.block_entities.remove(&pos).is_some();
    let removed = added_chest | removed_chest | removed_furnace | removed_hopper | removed_opaque;
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

#[cfg(test)]
#[path = "storage/tests.rs"]
mod tests;
