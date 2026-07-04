//! Lazy world storage on top of the Anvil codec.
//!
//! Opens a vanilla world directory (the one containing
//! `dimensions/minecraft/overworld/region/` or, on older saves,
//! `region/` directly), and serves block queries by loading the
//! covering region file on demand. Chunk and decoded-region LRUs keep
//! recent data resident; dirty chunks are flushed back through region
//! planning/write/commit paths.

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    ScheduledBlockTick, ScheduledFluidTick,
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
    /// LRU of fully decoded chunks, keyed by chunk position.
    cache: HashMap<ChunkPos, ChunkSnapshot>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldStorageStats {
    pub chunk_cache_len: usize,
    pub chunk_cache_capacity: usize,
    pub region_cache_len: usize,
    pub region_cache_capacity: usize,
    pub dirty_chunks: usize,
    pub dirty_chunk_cache_saturated: bool,
}

#[derive(Debug, Clone)]
pub struct DirtyFlushPlan {
    regions: Vec<DirtyFlushRegionPlan>,
    chunks: usize,
}

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
    #[cfg(test)]
    payload_digest: u64,
    payload: ChunkPayload,
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

fn can_fast_clean_chunk(
    chunk: &ChunkSnapshot,
    planned_generation: u64,
    planned_snapshot: &ChunkSnapshot,
) -> bool {
    planned_generation != 0
        && chunk.dirty_generation == planned_generation
        && Arc::ptr_eq(chunk, planned_snapshot)
}

pub enum ChunkSnapshotPlan {
    Cached(ChunkSnapshot),
    Load(ChunkDiskLoadPlan),
}

pub struct ChunkDiskLoadPlan {
    local: (u8, u8),
    region_path: PathBuf,
    cached_region: Option<Arc<DecodedRegion>>,
    registry: Arc<BlockRegistry>,
    item_registry: Option<Arc<ItemRegistry>>,
}

impl ChunkDiskLoadPlan {
    pub fn load(self) -> Result<Option<Chunk>, WorldError> {
        let payload = if let Some(region) = self.cached_region {
            region.get(&self.local).cloned()
        } else if self.region_path.is_file() {
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
        let mut commits = Vec::with_capacity(self.regions.len());
        for region in self.regions {
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

            for planned in &region.dirty_payloads {
                by_slot.insert(
                    (planned.payload.local_x, planned.payload.local_z),
                    planned.payload.clone(),
                );
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
                chunks: region
                    .dirty_payloads
                    .into_iter()
                    .map(|planned| CommittedChunkPayload {
                        pos: planned.pos,
                        current_tick: planned.current_tick,
                        dirty_generation: planned.dirty_generation,
                        snapshot: planned.snapshot,
                        #[cfg(test)]
                        snapshot_token: planned.snapshot_token,
                        #[cfg(test)]
                        payload_digest: planned.payload_digest,
                        uncompressed_nbt: planned.payload.uncompressed_nbt,
                    })
                    .collect(),
            });
        }

        Ok(DirtyFlushCommit { regions: commits })
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

        Ok(Self {
            world_root: Some(dir.to_path_buf()),
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
            world_root: None,
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
        let chunk = self.cache.get(&cpos)?;
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, pos.y, local_z)
    }

    /// Borrow a cached chunk; loads its region on demand.
    pub fn get_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        self.ensure_chunk(cpos)
    }

    /// Clone a chunk if it is already resident or present on disk, but do not
    /// invoke the fallback generator. Background chunk streaming uses this to
    /// keep expensive terrain generation outside the shared world mutex.
    pub fn get_chunk_without_generation(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<ChunkSnapshot>, WorldError> {
        self.ensure_chunk_loaded(cpos, false)?;
        Ok(self.cache.get(&cpos).cloned())
    }

    pub fn plan_chunk_snapshot_without_generation(&self, cpos: ChunkPos) -> ChunkSnapshotPlan {
        if let Some(chunk) = self.cache.get(&cpos) {
            return ChunkSnapshotPlan::Cached(Arc::clone(chunk));
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        ChunkSnapshotPlan::Load(ChunkDiskLoadPlan {
            local: (local_x, local_z),
            region_path: self.region_root.join(format!("r.{rx}.{rz}.mca")),
            cached_region: self.regions.get(&(rx, rz)).cloned(),
            registry: Arc::clone(&self.registry),
            item_registry: self.item_registry.clone(),
        })
    }

    pub fn commit_chunk_snapshot(
        &mut self,
        cpos: ChunkPos,
        chunk: Chunk,
    ) -> Result<ChunkSnapshot, WorldError> {
        if !self.cache.contains_key(&cpos) {
            self.insert_chunk(cpos, chunk)?;
        } else {
            self.touch(cpos);
        }
        Ok(self
            .cache
            .get(&cpos)
            .expect("chunk snapshot commit leaves chunk cached")
            .clone())
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
        self.cache.contains_key(&cpos) || !self.dirty_chunk_cache_saturated()
    }

    /// Clone a resident chunk without disk IO or generation.
    #[must_use]
    pub fn cached_chunk(&self, cpos: ChunkPos) -> Option<Chunk> {
        self.cache.get(&cpos).map(|chunk| chunk.as_ref().clone())
    }

    /// Return a resident chunk snapshot without disk IO, generation, or full chunk cloning.
    #[must_use]
    pub fn cached_chunk_snapshot(&self, cpos: ChunkPos) -> Option<ChunkSnapshot> {
        self.cache.get(&cpos).cloned()
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
        let cpos = chunk_pos_of(pos);
        let air = self
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|b| b.default)
            .unwrap_or(BlockStateId(0));
        let registry = Arc::clone(&self.registry);
        let local_x = pos.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = pos.z.rem_euclid(SECTION_DIM as i32) as u8;
        let Some(chunk) = self.ensure_chunk_mut(cpos)? else {
            return Ok(None);
        };
        let prev = chunk.set_block_and_update(local_x, pos.y, local_z, state, air);
        if prev.is_some_and(|prev| prev != state) {
            prune_incompatible_block_entities(chunk, pos, &registry, state);
        }
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
        let Some(chunk) = self.ensure_chunk_mut(cpos)? else {
            return Ok(());
        };
        chunk.update_highest_opaque_column(local_x, local_z, table);
        Ok(())
    }

    pub fn get_chunk_mut(&mut self, cpos: ChunkPos) -> Result<Option<&mut Chunk>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        self.touch(cpos);
        Ok(self.cache.get_mut(&cpos).map(Arc::make_mut))
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
        let Some(chunk) = self.ensure_chunk_mut_at(pos)? else {
            return Ok(false);
        };
        if chunk.furnaces.get(&pos) == Some(&furnace) {
            return Ok(true);
        }
        chunk.furnaces.insert(pos, furnace);
        chunk.mark_dirty();
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
        let Some(chunk) = self.ensure_chunk_mut_at(pos)? else {
            return Ok(false);
        };
        if chunk.chests.get(&pos) == Some(&chest) {
            return Ok(true);
        }
        chunk.chests.insert(pos, chest);
        chunk.mark_dirty();
        Ok(true)
    }

    pub fn set_opaque_block_entity(
        &mut self,
        pos: BlockPos,
        bytes: Vec<u8>,
    ) -> Result<bool, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut_at(pos)? else {
            return Ok(false);
        };
        if chunk.block_entities.get(&pos) == Some(&bytes) {
            return Ok(true);
        }
        chunk.block_entities.insert(pos, bytes);
        chunk.mark_dirty();
        Ok(true)
    }

    pub fn scheduled_block_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledBlockTick]>, WorldError> {
        let Some(chunk) = self.ensure_chunk(cpos)? else {
            return Ok(None);
        };
        Ok(Some(chunk.scheduled_block_ticks()))
    }

    pub fn schedule_block_tick(&mut self, tick: ScheduledBlockTick) -> Result<bool, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut_at(tick.pos)? else {
            return Ok(false);
        };
        Ok(chunk.schedule_block_tick(tick))
    }

    pub fn remove_scheduled_block_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledBlockTick>, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut_at(pos)? else {
            return Ok(Vec::new());
        };
        Ok(chunk.remove_scheduled_block_ticks_at(pos))
    }

    pub fn drain_due_block_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Result<Vec<ScheduledBlockTick>, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut(cpos)? else {
            return Ok(Vec::new());
        };
        Ok(chunk.drain_due_block_ticks(world_tick, max_ticks))
    }

    pub fn scheduled_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
    ) -> Result<Option<&[ScheduledFluidTick]>, WorldError> {
        let Some(chunk) = self.ensure_chunk(cpos)? else {
            return Ok(None);
        };
        Ok(Some(chunk.scheduled_fluid_ticks()))
    }

    pub fn schedule_fluid_tick(&mut self, tick: ScheduledFluidTick) -> Result<bool, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut_at(tick.pos)? else {
            return Ok(false);
        };
        Ok(chunk.schedule_fluid_tick(tick))
    }

    pub fn remove_scheduled_fluid_ticks_at(
        &mut self,
        pos: BlockPos,
    ) -> Result<Vec<ScheduledFluidTick>, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut_at(pos)? else {
            return Ok(Vec::new());
        };
        Ok(chunk.remove_scheduled_fluid_ticks_at(pos))
    }

    pub fn drain_due_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Result<Vec<ScheduledFluidTick>, WorldError> {
        let Some(chunk) = self.ensure_chunk_mut(cpos)? else {
            return Ok(Vec::new());
        };
        Ok(chunk.drain_due_fluid_ticks(world_tick, max_ticks))
    }

    pub fn drain_due_cached_fluid_ticks(
        &mut self,
        cpos: ChunkPos,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledFluidTick> {
        let Some(chunk) = self.cache.get_mut(&cpos) else {
            return Vec::new();
        };
        let chunk = Arc::make_mut(chunk);
        chunk.drain_due_fluid_ticks(world_tick, max_ticks)
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

    fn ensure_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        self.ensure_chunk_loaded(cpos, true)
    }

    fn ensure_chunk_mut(&mut self, cpos: ChunkPos) -> Result<Option<&mut Chunk>, WorldError> {
        if self.ensure_chunk(cpos)?.is_none() {
            return Ok(None);
        }
        Ok(self.cache.get_mut(&cpos).map(Arc::make_mut))
    }

    fn ensure_chunk_mut_at(&mut self, pos: BlockPos) -> Result<Option<&mut Chunk>, WorldError> {
        self.ensure_chunk_mut(chunk_pos_of(pos))
    }

    fn ensure_chunk_loaded(
        &mut self,
        cpos: ChunkPos,
        allow_generation: bool,
    ) -> Result<Option<&Chunk>, WorldError> {
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return Ok(self.cache.get(&cpos).map(Arc::as_ref));
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
            return Ok(self.cache.get(&cpos).map(Arc::as_ref));
        }

        // M7: no on-disk chunk → ask the generator (if any).
        if allow_generation && let Some(generator) = self.generator.as_ref().map(Arc::clone) {
            let mut chunk = generator.generate(cpos);
            chunk.mark_dirty(); // belt-and-braces; generator already sets this
            self.insert_chunk(cpos, chunk)?;
            return Ok(self.cache.get(&cpos).map(Arc::as_ref));
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
        // Dirty chunks are never evicted here: flushing them can rewrite
        // region files while callers hold the shared world mutex. If every
        // resident chunk is dirty, the cache grows until the save pipeline
        // commits them clean.
        while self.cache.len() >= self.capacity && self.evict_clean_chunk() {}
        self.cache.insert(cpos, Arc::new(chunk));
        self.lru.push_back(cpos);
        Ok(())
    }

    fn evict_clean_chunk(&mut self) -> bool {
        let scan_len = self.lru.len();
        for _ in 0..scan_len {
            let Some(evict) = self.lru.pop_front() else {
                return false;
            };
            if self.cache.get(&evict).is_some_and(|chunk| chunk.dirty) {
                self.lru.push_back(evict);
                continue;
            }
            self.cache.remove(&evict);
            return true;
        }
        false
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

    #[must_use]
    pub fn stats(&self) -> WorldStorageStats {
        WorldStorageStats {
            chunk_cache_len: self.cache.len(),
            chunk_cache_capacity: self.capacity,
            region_cache_len: self.regions.len(),
            region_cache_capacity: self.region_capacity,
            dirty_chunks: self.dirty_count(),
            dirty_chunk_cache_saturated: self.dirty_chunk_cache_saturated(),
        }
    }

    #[must_use]
    pub fn dirty_chunk_cache_saturated(&self) -> bool {
        self.cache.len() >= self.capacity && self.cache.values().all(|chunk| chunk.dirty)
    }

    /// Build a dirty chunk flush plan. The plan owns the encoded chunk
    /// payloads and the region versions observed while planning so callers can
    /// write region files after releasing any outer world mutex without
    /// replacing a newer region snapshot.
    pub fn plan_dirty_flush(&self) -> Result<DirtyFlushPlan, WorldError> {
        self.plan_dirty_flush_at_tick(0)
    }

    pub fn plan_dirty_flush_at_tick(
        &self,
        current_tick: u64,
    ) -> Result<DirtyFlushPlan, WorldError> {
        let dirty_positions: Vec<ChunkPos> = self
            .cache
            .iter()
            .filter_map(|(pos, chunk)| chunk.dirty.then_some(*pos))
            .collect();
        if dirty_positions.is_empty() {
            return Ok(DirtyFlushPlan {
                regions: Vec::new(),
                chunks: 0,
            });
        }
        let mut by_region: HashMap<(i32, i32), Vec<ChunkPos>> = HashMap::new();
        for pos in dirty_positions {
            by_region.entry(region_of(pos)).or_default().push(pos);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let mut regions = Vec::with_capacity(by_region.len());
        let mut chunks = 0usize;
        for ((rx, rz), mut positions) in by_region {
            positions.sort_by_key(|pos| (pos.z, pos.x));
            let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
            let expected_version = region_file_version(&region_path)?;
            let mut dirty_payloads = Vec::with_capacity(positions.len());
            for cpos in positions {
                let chunk = self
                    .cache
                    .get(&cpos)
                    .expect("dirty position must still be in cache");
                let payload = chunk_to_payload_with_items_at_tick(
                    chunk,
                    &self.registry,
                    self.item_registry.as_deref(),
                    now,
                    current_tick,
                )?;
                #[cfg(test)]
                let payload_digest = payload_digest(&payload.uncompressed_nbt);
                dirty_payloads.push(PlannedChunkPayload {
                    pos: cpos,
                    current_tick,
                    dirty_generation: chunk.dirty_generation,
                    snapshot: Arc::clone(chunk),
                    #[cfg(test)]
                    snapshot_token: chunk_snapshot_token(chunk),
                    #[cfg(test)]
                    payload_digest,
                    payload,
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

        Ok(DirtyFlushPlan { regions, chunks })
    }

    /// Commit a written flush plan. Chunks are marked clean only if their dirty
    /// generation still permits the comparison and the encoded payload still
    /// matches the payload that was written. Chunks changed after planning
    /// remain dirty.
    pub fn commit_dirty_flush(&mut self, commit: DirtyFlushCommit) -> Result<usize, WorldError> {
        let mut clean = Vec::new();
        let mut written_regions = Vec::new();
        for region in commit.regions {
            written_regions.push(region.region);
            for planned in region.chunks {
                let Some(chunk) = self.cache.get(&planned.pos) else {
                    continue;
                };
                if !chunk.dirty {
                    continue;
                }
                if planned.dirty_generation != 0
                    && chunk.dirty_generation != planned.dirty_generation
                {
                    continue;
                }
                let clean_chunk =
                    if can_fast_clean_chunk(chunk, planned.dirty_generation, &planned.snapshot) {
                        true
                    } else {
                        let current = chunk_to_payload_with_items_at_tick(
                            chunk,
                            &self.registry,
                            self.item_registry.as_deref(),
                            0,
                            planned.current_tick,
                        )?;
                        current.uncompressed_nbt == planned.uncompressed_nbt
                    };
                if clean_chunk {
                    clean.push(planned.pos);
                }
            }
        }

        for cpos in &clean {
            if let Some(chunk) = self.cache.get_mut(cpos) {
                let chunk = Arc::make_mut(chunk);
                chunk.dirty = false;
            }
        }
        for region in written_regions {
            self.regions.remove(&region);
            self.region_lru.retain(|&k| k != region);
        }

        Ok(clean.len())
    }

    /// M6.b: write every dirty chunk in the cache back to its
    /// `.mca` region file. Returns the number of chunks flushed.
    /// Groups dirty chunks by region so each `r.X.Z.mca` is rewritten
    /// at most once per call.
    pub fn flush_dirty(&mut self) -> Result<usize, WorldError> {
        self.flush_dirty_at_tick(0)
    }

    pub fn flush_dirty_at_tick(&mut self, current_tick: u64) -> Result<usize, WorldError> {
        let plan = self.plan_dirty_flush_at_tick(current_tick)?;
        if plan.is_empty() {
            return Ok(0);
        }
        let commit = plan.write()?;
        self.commit_dirty_flush(commit)
    }

    /// Number of dirty chunks currently in the cache. Used by tests
    /// and the Ctrl-C shutdown log.
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.cache.values().filter(|c| c.dirty).count()
    }
}

fn prune_incompatible_block_entities(
    chunk: &mut Chunk,
    pos: BlockPos,
    registry: &BlockRegistry,
    state: BlockStateId,
) {
    let path = registry.by_id(state).map(|state| state.block.id.path());
    let keeps_chest = path.is_some_and(|path| matches!(path, "chest" | "barrel"));
    let keeps_furnace =
        path.is_some_and(|path| matches!(path, "furnace" | "blast_furnace" | "smoker"));
    let keeps_opaque = path.is_some_and(block_path_may_have_opaque_block_entity);

    let removed = (!keeps_chest && chunk.chests.remove(&pos).is_some())
        | (!keeps_furnace && chunk.furnaces.remove(&pos).is_some())
        | (!keeps_opaque && chunk.block_entities.remove(&pos).is_some());
    if removed {
        chunk.mark_dirty();
    }
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
        };
        chest.slots[26] = crate::chunk::FurnaceSlot {
            count: 3,
            item_id: 11,
            damage: None,
        };
        world.set_chest_block_entity(pos, chest.clone()).unwrap();
        assert_eq!(world.flush_dirty().unwrap(), 1);

        let mut fresh = WorldStorage::open(tmp.path(), Arc::clone(&registry))
            .unwrap()
            .with_item_registry(items);
        assert_eq!(fresh.chest_block_entity(pos).unwrap(), Some(chest));
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
        };
        world.set_chest_block_entity(pos, chest).unwrap();

        world.set_block_at(pos, BlockStateId(1)).unwrap();

        let chunk = world.cache.get(&cpos).unwrap();
        assert!(!chunk.chests.contains_key(&pos));
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
    fn dirty_flush_plan_tracks_retained_snapshot_token_and_payload_digest() {
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
        let snapshot = world.cache.get(&cpos).unwrap();

        assert!(Arc::ptr_eq(&planned.snapshot, snapshot));
        assert_eq!(planned.snapshot_token, chunk_snapshot_token(snapshot));
        assert_eq!(
            planned.payload_digest,
            payload_digest(&planned.payload.uncompressed_nbt)
        );
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
        let expected_payload_digest = planned.payload_digest;
        let commit = plan.write().unwrap();
        let committed = &commit.regions[0].chunks[0];

        assert!(Arc::ptr_eq(&committed.snapshot, &expected_snapshot));
        assert_eq!(committed.snapshot_token, expected_snapshot_token);
        assert_eq!(committed.payload_digest, expected_payload_digest);
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
    fn dirty_flush_fast_path_rejects_copy_on_write_fork_without_generation_bump() {
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

        let live_snapshot = world.cache.get(&cpos).unwrap();
        assert_eq!(live_snapshot.dirty_generation, planned_generation);
        assert!(!Arc::ptr_eq(live_snapshot, &planned_snapshot));
        assert_eq!(
            planned_snapshot.get_block(1, 0, 1).unwrap(),
            BlockStateId(0)
        );
        assert_eq!(live_snapshot.get_block(1, 0, 1).unwrap(), BlockStateId(1));
        assert!(!can_fast_clean_chunk(
            live_snapshot,
            planned_generation,
            &planned_snapshot,
        ));
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
            world.cache.get(&cpos).unwrap(),
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
        let before_token = chunk_snapshot_token(world.cache.get(&cpos).unwrap());
        let commit = plan.write().unwrap();

        assert_eq!(world.commit_dirty_flush(commit).unwrap(), 1);

        let live = world.cache.get(&cpos).unwrap();
        assert!(!live.dirty);
        assert_eq!(chunk_snapshot_token(live), before_token);
    }

    #[test]
    fn dirty_flush_commit_falls_back_to_payload_compare_for_derived_only_snapshot_change() {
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

        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .update_highest_opaque_column(1, 1, &table);

        let live_snapshot = world.cache.get(&cpos).unwrap();
        let commit = plan.write().unwrap();

        assert_eq!(live_snapshot.dirty_generation, planned_generation);
        assert!(!Arc::ptr_eq(live_snapshot, &planned_snapshot));
        assert_eq!(planned_snapshot.highest_opaque_y(1, 1), None);
        assert_eq!(live_snapshot.highest_opaque_y(1, 1), Some(0));
        assert!(!can_fast_clean_chunk(
            live_snapshot,
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
        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .update_highest_opaque_column(1, 1, &table);
        let live_snapshot = world.cache.get(&cpos).unwrap();
        let mut commit = plan.write().unwrap();
        commit.regions[0].chunks[0].uncompressed_nbt.clear();

        assert_eq!(live_snapshot.dirty_generation, planned_generation);
        assert!(!can_fast_clean_chunk(
            live_snapshot,
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
        let planned_generation = world.cache.get(&cpos).unwrap().dirty_generation;
        world
            .get_chunk_mut(cpos)
            .unwrap()
            .unwrap()
            .set_block(1, 0, 1, BlockStateId(1));
        assert_eq!(
            world.cache.get(&cpos).unwrap().dirty_generation,
            planned_generation
        );

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
            world.cache.get(&cpos).unwrap(),
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
            world.cache.get(&cpos).unwrap(),
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
