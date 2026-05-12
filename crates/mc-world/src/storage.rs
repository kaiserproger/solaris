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

use mc_data::blocks::BlockReport;
use thiserror::Error;

use crate::anvil::{ChunkNbtError, RegionError, chunk_from_nbt, read_region};
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{BlockPos, Chunk, ChunkPos};
use crate::section::SECTION_DIM;

const REGION_AXIS_CHUNKS: i32 = 32;
const DEFAULT_LRU_CAPACITY: usize = 16;

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
    registry: BlockRegistry,
    /// LRU of fully decoded chunks, keyed by chunk position.
    cache: HashMap<ChunkPos, Chunk>,
    /// MRU at the back, LRU at the front. On `get_chunk` we move
    /// the accessed key to the back.
    lru: VecDeque<ChunkPos>,
    capacity: usize,
}

impl WorldStorage {
    /// Open a world directory. Tries the 1.20+ layout
    /// (`dimensions/minecraft/overworld/region/`) first, falls back
    /// to the pre-1.20 flat layout (`region/`). Loads the block
    /// registry from `blocks_report` so block queries can resolve
    /// palette entries.
    pub fn open(
        world_dir: impl AsRef<Path>,
        blocks_report: &[BlockReport],
    ) -> Result<Self, WorldError> {
        Self::open_with_capacity(world_dir, blocks_report, DEFAULT_LRU_CAPACITY)
    }

    pub fn open_with_capacity(
        world_dir: impl AsRef<Path>,
        blocks_report: &[BlockReport],
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

        let registry = BlockRegistry::from_report(blocks_report).expect("registry must build");

        Ok(Self {
            region_root,
            registry,
            cache: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
        })
    }

    #[must_use]
    pub fn registry(&self) -> &BlockRegistry {
        &self.registry
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

    fn ensure_chunk(&mut self, cpos: ChunkPos) -> Result<Option<&Chunk>, WorldError> {
        if self.cache.contains_key(&cpos) {
            self.touch(cpos);
            return Ok(self.cache.get(&cpos));
        }
        let (rx, rz) = region_of(cpos);
        let region_path = self.region_root.join(format!("r.{rx}.{rz}.mca"));
        if !region_path.is_file() {
            return Ok(None);
        }
        let local_x = cpos.x.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        let local_z = cpos.z.rem_euclid(REGION_AXIS_CHUNKS) as u8;
        // TODO(M3 perf): cache parsed regions so we don't re-read the
        // whole .mca for every cache miss in the same region. For M2,
        // correctness over throughput.
        let payloads = read_region(&region_path)?;
        let Some(payload) = payloads
            .iter()
            .find(|p| p.local_x == local_x && p.local_z == local_z)
        else {
            return Ok(None);
        };
        let mut cursor = std::io::Cursor::new(&payload.uncompressed_nbt[..]);
        let (_, root) = mc_nbt::read_named(&mut cursor)?;
        let chunk = chunk_from_nbt(&root, &self.registry)?;
        self.insert_chunk(cpos, chunk);
        Ok(self.cache.get(&cpos))
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
        let mut world = WorldStorage::open_with_capacity(&world_dir, &report, 4).unwrap();

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
}
