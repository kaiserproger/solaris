//! Lazy, read-only world storage on top of the Anvil codec.
//!
//! Opens a vanilla world directory (the one containing
//! `dimensions/minecraft/overworld/region/` or, on older saves,
//! `region/` directly), and serves block queries by loading the
//! covering region file on demand. A small LRU keeps the recently
//! used regions resident; everything else is reloaded as needed.
//!
//! M2 is read-only: no writes, no save-back. Modifications will
//! land in M3 along with chunk streaming.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_data::items::ItemRegistry;

use crate::anvil::{
    ChunkNbtError, ChunkPayload, RegionError, chunk_from_nbt_with_items,
    chunk_to_payload_with_items, read_region, write_region,
};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockPos, Chunk, ChunkGenerator, ChunkPos, FurnaceBlockEntity, ScheduledBlockTick,
    ScheduledFluidTick,
};
use crate::section::SECTION_DIM;

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
}

/// Read-only handle to a world's chunk data.
pub struct WorldStorage {
    region_root: PathBuf,
    registry: Arc<BlockRegistry>,
    /// LRU of fully decoded chunks, keyed by chunk position.
    cache: HashMap<ChunkPos, Chunk>,
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

        Ok(Self {
            region_root,
            registry,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
            regions: HashMap::new(),
            region_lru: VecDeque::new(),
            region_capacity: region_capacity.max(1),
            item_registry: None,
            generator: None,
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
        Self {
            region_root: PathBuf::new(),
            registry,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
            regions: HashMap::new(),
            region_lru: VecDeque::new(),
            region_capacity: DEFAULT_REGION_LRU_CAPACITY,
            item_registry: None,
            generator: None,
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
        self.generator = Some(generator);
        self
    }

    /// Convenience for the `mc-server` startup path: swap a generator
    /// in after the fact. Returns the previous generator (if any).
    pub fn set_generator(
        &mut self,
        generator: Option<Arc<dyn ChunkGenerator>>,
    ) -> Option<Arc<dyn ChunkGenerator>> {
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

    /// Borrow a cached chunk; loads its region on demand.
    pub fn get_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        self.ensure_chunk(cpos)?;
        Ok(self.cache.get(&cpos))
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
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        let air = self
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|b| b.default)
            .unwrap_or(BlockStateId(0));
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.set_block_and_update(local_x, pos.y, local_z, state, air))
    }

    pub fn update_highest_opaque_at(
        &mut self,
        pos: BlockPos,
        table: &BlockLightTable,
    ) -> Result<(), WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(());
        }
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        chunk.update_highest_opaque_column(local_x, local_z, table);
        Ok(())
    }

    pub fn get_chunk_mut(&mut self, cpos: ChunkPos) -> Result<Option<&mut Chunk>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        self.touch(cpos);
        Ok(self.cache.get_mut(&cpos))
    }

    pub fn furnace_block_entity(
        &mut self,
        pos: BlockPos,
    ) -> Result<Option<FurnaceBlockEntity>, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        let chunk = self
            .cache
            .get(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
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
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        if chunk.furnaces.get(&pos) == Some(&furnace) {
            return Ok(true);
        }
        chunk.furnaces.insert(pos, furnace);
        chunk.dirty = true;
        Ok(true)
    }

    pub fn scheduled_block_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledBlockTick]>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        let chunk = self
            .cache
            .get(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(Some(chunk.scheduled_block_ticks()))
    }

    pub fn schedule_block_tick(&mut self, tick: ScheduledBlockTick) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(tick.pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.schedule_block_tick(tick))
    }

    pub fn remove_scheduled_block_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledBlockTick>, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.remove_scheduled_block_ticks_at(pos))
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
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.drain_due_block_ticks(world_tick, max_ticks))
    }

    pub fn scheduled_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledFluidTick]>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        let chunk = self
            .cache
            .get(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(Some(chunk.scheduled_fluid_ticks()))
    }

    pub fn schedule_fluid_tick(&mut self, tick: ScheduledFluidTick) -> Result<bool, WorldError> {
        let cpos = chunk_pos_of(tick.pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(false);
        }
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.schedule_fluid_tick(tick))
    }

    pub fn remove_scheduled_fluid_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledFluidTick>, WorldError> {
        let cpos = chunk_pos_of(pos);
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(Vec::new());
        }
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.remove_scheduled_fluid_ticks_at(pos))
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
        let chunk = self
            .cache
            .get_mut(&cpos)
            .expect("ensure_chunk placed the chunk in cache");
        Ok(chunk.drain_due_fluid_ticks(world_tick, max_ticks))
    }

    /// Insert a freshly generated chunk through the same cache/LRU path
    /// as the lazy generator fallback. Existing cached chunks win.
    pub fn insert_generated_chunk(
        &mut self,
        cpos: ChunkPos,
        mut chunk: Chunk,
    ) -> Result<(), WorldError> {
        chunk.dirty = true;
        self.insert_chunk(cpos, chunk)
    }

    fn ensure_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return Ok(self.cache.get(&cpos));
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
            return Ok(self.cache.get(&cpos));
        }

        // M7: no on-disk chunk → ask the generator (if any).
        if let Some(generator) = self.generator.as_ref().map(Arc::clone) {
            let mut chunk = generator.generate(cpos);
            chunk.dirty = true; // belt-and-braces; generator already sets this
            self.insert_chunk(cpos, chunk)?;
            return Ok(self.cache.get(&cpos));
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
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return Ok(());
        }
        // M6.b: if eviction would drop a dirty chunk, flush every
        // dirty chunk to disk first. Spawn-burst loads aren't dirty,
        // so this only fires after player edits.
        if self.cache.len() >= self.capacity
            && let Some(front) = self.lru.front()
            && self.cache.get(front).is_some_and(|c| c.dirty)
        {
            self.flush_dirty()?;
        }
        while self.cache.len() >= self.capacity {
            if let Some(evict) = self.lru.pop_front() {
                self.cache.remove(&evict);
            } else {
                break;
            }
        }
        self.cache.insert(cpos, chunk);
        self.lru.push_back(cpos);
        Ok(())
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
        self.cache.len()
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

    /// M6.b: write every dirty chunk in the cache back to its
    /// `.mca` region file. Returns the number of chunks flushed.
    /// Groups dirty chunks by region so each `r.X.Z.mca` is rewritten
    /// at most once per call.
    pub fn flush_dirty(&mut self) -> Result<usize, WorldError> {
        let dirty_positions: Vec<ChunkPos> = self
            .cache
            .iter()
            .filter_map(|(pos, chunk)| chunk.dirty.then_some(*pos))
            .collect();
        if dirty_positions.is_empty() {
            return Ok(0);
        }
        let mut by_region: HashMap<(i32, i32), Vec<ChunkPos>> = HashMap::new();
        for pos in dirty_positions {
            by_region.entry(region_of(pos)).or_default().push(pos);
        }
        let mut flushed = 0usize;
        for ((rx, rz), positions) in by_region {
            flushed += self.flush_region(rx, rz, &positions)?;
        }
        Ok(flushed)
    }

    /// Flush one region: merge the listed dirty chunks (which must
    /// all live in `(rx, rz)`) with whatever already sits on disk,
    /// then rewrite the region file atomically via a sibling temp +
    /// rename. The region cache for this region is dropped on
    /// success so the next read picks up the fresh bytes.
    fn flush_region(
        &mut self,
        rx: i32,
        rz: i32,
        positions: &[ChunkPos],
    ) -> Result<usize, WorldError> {
        let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
        // Load existing payloads (if any) so unmodified slots survive.
        let mut by_slot: HashMap<(u8, u8), ChunkPayload> = if region_path.is_file() {
            read_region(&region_path)?
                .into_iter()
                .map(|p| ((p.local_x, p.local_z), p))
                .collect()
        } else {
            HashMap::new()
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);

        for &cpos in positions {
            let chunk = self
                .cache
                .get(&cpos)
                .expect("dirty position must still be in cache");
            let payload = chunk_to_payload_with_items(
                chunk,
                &self.registry,
                self.item_registry.as_deref(),
                now,
            )?;
            by_slot.insert((payload.local_x, payload.local_z), payload);
        }

        let mut payloads: Vec<ChunkPayload> = by_slot.into_values().collect();
        payloads.sort_by_key(|p| (p.local_z, p.local_x));

        let tmp_path = region_path.with_extension("mca.tmp");
        write_region(&tmp_path, &payloads)?;
        std::fs::rename(&tmp_path, &region_path).map_err(|e| {
            WorldError::Region(RegionError::Io {
                path: region_path.clone(),
                source: e,
            })
        })?;

        // Mark flushed chunks clean.
        for &cpos in positions {
            if let Some(chunk) = self.cache.get_mut(&cpos) {
                chunk.dirty = false;
            }
        }
        // Drop the region cache for this region so a subsequent read
        // sees the freshly-written bytes.
        self.regions.remove(&(rx, rz));
        self.region_lru.retain(|&k| k != (rx, rz));

        Ok(positions.len())
    }

    /// Number of dirty chunks currently in the cache. Used by tests
    /// and the Ctrl-C shutdown log.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.cache.values().filter(|c| c.dirty).count()
    }
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
    use super::*;
    use crate::chunk::{MAX_Y, MIN_Y};
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
        };
        furnace.slots[1] = crate::chunk::FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
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
