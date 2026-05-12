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

use crate::anvil::{ChunkNbtError, ChunkPayload, RegionError, chunk_from_nbt, read_region};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, Chunk, ChunkPos};
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
            region_capacity: DEFAULT_REGION_LRU_CAPACITY,
        })
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

    fn ensure_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return Ok(self.cache.get(&cpos));
        }
        let (rx, rz) = region_of(cpos);
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;

        let Some(region) = self.ensure_region(rx, rz)? else {
            return Ok(None);
        };
        let Some(payload) = region.get(&(local_x, local_z)).cloned() else {
            return Ok(None);
        };
        drop(region);

        let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cursor)?;
        let chunk = chunk_from_nbt(&root, &self.registry)?;
        self.insert_chunk(cpos, chunk);
        Ok(self.cache.get(&cpos))
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

    fn insert_chunk(&mut self, cpos: ChunkPos, chunk: Chunk) {
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return;
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
    use std::path::{Path, PathBuf};

    fn workspace_path(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join(rel)
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

    /// End-to-end: open the generated flat test world, query known
    /// coordinates of the vanilla default flat preset (Y=-64 bedrock,
    /// Y=-61 grass_block, Y>=−60 air), assert out-of-range / missing
    /// chunks return None instead of erroring, and confirm the LRU
    /// stays bounded.
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
        let mut world = WorldStorage::open_with_capacity(&world_dir, registry, 4).unwrap();

        let resolve = |w: &WorldStorage, id: BlockStateId| {
            w.registry()
                .by_id(id)
                .unwrap()
                .block
                .id
                .as_str()
                .to_string()
        };

        let bedrock = world
            .get_block(BlockPos { x: 0, y: -64, z: 0 })
            .unwrap()
            .unwrap();
        let grass = world
            .get_block(BlockPos { x: 0, y: -61, z: 0 })
            .unwrap()
            .unwrap();
        let air_above = world
            .get_block(BlockPos { x: 0, y: 5, z: 0 })
            .unwrap()
            .unwrap();
        assert_eq!(resolve(&world, bedrock), "minecraft:bedrock");
        assert_eq!(resolve(&world, grass), "minecraft:grass_block");
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
        let mut world = WorldStorage::open_with_capacity(&world_dir, registry, 4).unwrap();

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
        let mut world = WorldStorage::open(&world_dir, registry).unwrap();

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
