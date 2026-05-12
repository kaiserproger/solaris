//! Baseline terrain generator (M7).
//!
//! Produces a fully-formed [`Chunk`] from `(ChunkPos, seed)` using
//! Solaris's own hash-noise — no vanilla algorithm involved (per
//! ADR 0001 / PROJECT_SPEC §8.1). One biome (plains), no
//! structures, no caves, no ores. Vertical layers:
//!
//! - `y = MIN_Y` → bedrock
//! - `MIN_Y < y < height - 3` → stone
//! - `height - 3 ≤ y < height` → dirt
//! - `y = height` → grass_block
//! - `y > height` → air
//!
//! `height` is sampled from the multi-octave noise centred on
//! `BASE_HEIGHT` with `±HEIGHT_AMPLITUDE` swing. The result is
//! deterministic in `(seed, world_x, world_z)`.

use std::sync::Arc;

use mc_data::Identifier;
use mc_world::chunk::{Chunk, ChunkPos, Heightmap, MIN_Y};
use mc_world::{BlockRegistry, BlockStateId};

use crate::noise::fbm_2d;

/// Production interface every generator implements.
///
/// `Send + Sync` because the world handle that owns the generator is
/// shared across the network listener's connection tasks via
/// `Arc<Mutex<WorldStorage>>`.
pub trait ChunkGenerator: Send + Sync {
    /// Build a brand-new `Chunk` for the given position. The
    /// returned chunk is "fresh" — `dirty` is set so the M6 flush
    /// path persists it to disk before the cache evicts it.
    fn generate(&self, pos: ChunkPos) -> Chunk;
}

/// Default terrain centre. Chosen so the player spawns on top of
/// the surface without needing to fall.
const BASE_HEIGHT: f64 = 70.0;
/// Peak-to-trough amplitude of the height field (in blocks above /
/// below `BASE_HEIGHT`).
const HEIGHT_AMPLITUDE: f64 = 12.0;
/// Lattice spacing of the noise. Smaller = lumpier; this gives
/// ~24-block hills.
const NOISE_FREQUENCY: f64 = 1.0 / 24.0;
/// Octaves of fbm noise. Three is enough to round off the smooth
/// blobs of single-octave value-noise into something hill-shaped.
const NOISE_OCTAVES: u32 = 3;
const NOISE_PERSISTENCE: f64 = 0.5;
/// Number of dirt cells between grass cap and stone.
const DIRT_DEPTH: i32 = 3;

/// Hill-noise terrain. Holds the resolved state ids of the four
/// block types it emits so `generate` is allocation-free past the
/// `Chunk::empty` it returns.
pub struct TerrainGenerator {
    seed: i64,
    air: BlockStateId,
    bedrock: BlockStateId,
    stone: BlockStateId,
    dirt: BlockStateId,
    grass_block: BlockStateId,
    plains: Identifier,
    // Kept so the generator's lifetime is bounded by something
    // sensible if the storage drops the only other reference.
    #[allow(dead_code)]
    registry: Arc<BlockRegistry>,
}

impl TerrainGenerator {
    /// Build a generator from a seed plus a block registry. Panics
    /// if any of the four required blocks (air, bedrock, stone,
    /// dirt, grass_block) is missing from the registry — they are
    /// vanilla-mandatory for any 26.1.2 world and resolving them
    /// once at construction time keeps `generate` hot-path-free.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        let resolve = |name: &str| -> BlockStateId {
            let id = Identifier::parse(name).expect("static identifier");
            registry
                .block(&id)
                .map(|b| b.default)
                .unwrap_or_else(|| panic!("registry missing required block {name}"))
        };
        let plains = Identifier::parse("minecraft:plains").expect("static identifier");
        Self {
            seed,
            air: resolve("minecraft:air"),
            bedrock: resolve("minecraft:bedrock"),
            stone: resolve("minecraft:stone"),
            dirt: resolve("minecraft:dirt"),
            grass_block: resolve("minecraft:grass_block"),
            plains,
            registry,
        }
    }

    /// Sample the terrain height for an absolute world `(x, z)`.
    /// Public so tests + spawn-position picking can use the same
    /// function the generator does.
    #[must_use]
    pub fn surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        let n = fbm_2d(
            world_x as f64 * NOISE_FREQUENCY,
            world_z as f64 * NOISE_FREQUENCY,
            self.seed,
            NOISE_OCTAVES,
            NOISE_PERSISTENCE,
        );
        let raw = BASE_HEIGHT + n * HEIGHT_AMPLITUDE;
        // Guard against extreme outputs even though fbm_2d is bounded.
        raw.round().clamp(MIN_Y as f64 + 2.0, 250.0) as i32
    }

    fn fill_column(&self, chunk: &mut Chunk, lx: u8, lz: u8, height: i32) {
        let _ = chunk.set_block(lx, MIN_Y, lz, self.bedrock);
        let dirt_start = (height - DIRT_DEPTH).max(MIN_Y + 1);
        for y in (MIN_Y + 1)..dirt_start {
            let _ = chunk.set_block(lx, y, lz, self.stone);
        }
        for y in dirt_start..height {
            let _ = chunk.set_block(lx, y, lz, self.dirt);
        }
        let _ = chunk.set_block(lx, height, lz, self.grass_block);
        // Air above stays as-is from Chunk::empty.
        let _ = self.air;
    }
}

impl ChunkGenerator for TerrainGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(pos, self.air, self.plains.clone());
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let wx = pos.x * 16 + lx as i32;
                let wz = pos.z * 16 + lz as i32;
                let height = self.surface_height(wx, wz);
                self.fill_column(&mut chunk, lx, lz, height);
                // Heightmap value: Y of the first air cell above the
                // top non-air block, expressed as `(top + 1) - MIN_Y`.
                let hm = (height + 1 - MIN_Y) as u32;
                if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                    mb.set(lx, lz, hm);
                }
                if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                    ws.set(lx, lz, hm);
                }
            }
        }
        chunk.status = "minecraft:full".into();
        chunk.dirty = true;
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn tiny_registry() -> Arc<BlockRegistry> {
        use mc_data::blocks::{BlockReport, BlockStateReport};
        let report = vec![
            BlockReport {
                id: Identifier::parse("minecraft:air").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 0,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:bedrock").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 1,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 2,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:dirt").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 3,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:grass_block").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 4,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ];
        Arc::new(BlockRegistry::from_report(&report).unwrap())
    }

    #[test]
    fn generated_column_has_bedrock_stone_dirt_grass() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let chunk = g.generate(ChunkPos { x: 0, z: 0 });
        let air = BlockStateId(0);
        let bedrock = BlockStateId(1);
        let grass = BlockStateId(4);

        // Bedrock at MIN_Y.
        assert_eq!(chunk.get_block(8, MIN_Y, 8), Some(bedrock));
        // Find the top non-air cell. It must be grass.
        let height = g.surface_height(8, 8);
        assert_eq!(chunk.get_block(8, height, 8), Some(grass));
        assert_eq!(chunk.get_block(8, height + 1, 8), Some(air));

        // Heightmap value matches the height field.
        let hm = chunk.heightmaps.get("WORLD_SURFACE").unwrap();
        assert_eq!(hm.get(8, 8), (height + 1 - MIN_Y) as u32);

        // Dirty flag set so M6 flush picks it up.
        assert!(chunk.dirty);
    }

    #[test]
    fn determinism_across_repeated_generate_calls() {
        let g = TerrainGenerator::new(99, tiny_registry());
        let a = g.generate(ChunkPos { x: 5, z: -3 });
        let b = g.generate(ChunkPos { x: 5, z: -3 });
        for y in MIN_Y..=80 {
            for x in 0..16u8 {
                for z in 0..16u8 {
                    assert_eq!(a.get_block(x, y, z), b.get_block(x, y, z));
                }
            }
        }
    }

    #[test]
    fn far_chunks_still_have_terrain() {
        let g = TerrainGenerator::new(1234, tiny_registry());
        let chunk = g.generate(ChunkPos {
            x: 1_000,
            z: -1_000,
        });
        let grass = BlockStateId(4);
        let height = g.surface_height(1_000 * 16 + 8, -1_000 * 16 + 8);
        assert_eq!(chunk.get_block(8, height, 8), Some(grass));
        assert_eq!(chunk.status, "minecraft:full");
    }
}
