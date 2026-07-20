//! Lazy world storage on top of the Anvil codec.
//!
//! Opens a vanilla world directory (the one containing
//! `dimensions/minecraft/overworld/region/` or, on older saves,
//! `region/` directly), and serves block queries by loading the
//! covering region file on demand. Chunk and decoded-region LRUs keep
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

use crate::anvil::{
    ChunkNbtError, ChunkPayload, RegionError, chunk_from_nbt_with_items, read_region,
};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockPos, ChestBlockEntity, Chunk, ChunkGenerator, ChunkPos, FurnaceBlockEntity,
    HopperBlockEntity, ScheduledBlockTick, ScheduledFluidTick,
};
use crate::light::ChunkLight;
use crate::resident::{ResidentChunkStore, WorldMutationView};
use crate::section::SECTION_DIM;

mod dirty_flush;
mod read_view;
#[cfg(test)]
mod test_support;

pub use dirty_flush::{
    DirtyFlushCommit, DirtyFlushFinalize, DirtyFlushInstall, DirtyFlushPlan, DirtyFlushSynced,
};
pub(crate) use read_view::ResidentPublicationState;
pub use read_view::{
    ChunkDiskLoadPlan, ChunkPrepareSource, ChunkSnapshot, ChunkSnapshotPlan, ChunkSourceView,
    DirtyHighWaterNotifier, ScheduledTickView, WorldReadSnapshot, WorldReadView,
};

const REGION_AXIS_CHUNKS: i32 = 32;
const DEFAULT_LRU_CAPACITY: usize = 16;
/// How many decoded regions (`.mca` files with per-chunk payloads
/// already decompressed) we hold resident at once. Each entry is on
/// the order of tens of MB for a dense overworld region; four is a
/// pragmatic default that covers the M3.e view-distance ring around
/// a single player without growing unboundedly.
const DEFAULT_REGION_LRU_CAPACITY: usize = 4;
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
    #[error(
        "dirty flush captured {dirty_chunks} dirty chunks but only {flushable_chunks} were journal-ready"
    )]
    JournalPendingDirtyChunks {
        dirty_chunks: usize,
        flushable_chunks: usize,
    },
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
        let scheduled_tick_view =
            ScheduledTickView::with_publication(read_view.publication_state());
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
        let scheduled_tick_view =
            ScheduledTickView::with_publication(read_view.publication_state());
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

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use mc_nbt::Tag;

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
        assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
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
        let flushable = ChunkPos { x: 1, z: 0 };
        for chunk_position in [position, flushable] {
            world
                .insert_generated_chunk(
                    chunk_position,
                    Chunk::empty(
                        chunk_position,
                        BlockStateId(0),
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
        }

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
        let plan = world.plan_dirty_flush().unwrap();
        assert_eq!(plan.chunk_count(), 1);
        assert_eq!(plan.dirty_chunks_at_capture(), 2);
        assert!(!plan.captures_all_dirty_chunks());
        assert!(matches!(
            world.flush_dirty(),
            Err(WorldError::JournalPendingDirtyChunks {
                dirty_chunks: 2,
                flushable_chunks: 1,
            })
        ));
        assert_eq!(world.dirty_count(), 2);
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
        assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
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
        assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
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
        assert_eq!(world.plan_dirty_flush().unwrap().chunk_count(), 1);
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
