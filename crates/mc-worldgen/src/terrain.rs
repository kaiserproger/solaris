//! Baseline terrain generator (M7).
//!
//! Produces a fully-formed [`Chunk`] from `(ChunkPos, seed)` using
//! Solaris's own hash-noise — no vanilla algorithm involved (per
//! ADR 0001 / PROJECT_SPEC §8.1). One biome (plains), no structures.
//! M17 adds deterministic cave, ore, and fluid placement primitives.
//! Vertical base layers:
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
use mc_world::{BlockRegistry, BlockStateId, ChunkGenerator};

use crate::noise::fbm_2d;

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
const CAVE_MIN_Y: i32 = MIN_Y + 8;
const CAVE_SURFACE_CLEARANCE: i32 = 8;
const CAVE_FREQUENCY: f64 = 1.0 / 34.0;
const CAVE_THRESHOLD: f64 = 0.24;
const DEEPSLATE_TOP_Y: i32 = 0;
const DEEPSLATE_SOLID_Y: i32 = -8;
const COAL_MIN_Y: i32 = 0;
const COAL_MAX_Y: i32 = 192;
const IRON_MIN_Y: i32 = -24;
const IRON_MAX_Y: i32 = 72;
const COPPER_MIN_Y: i32 = -16;
const COPPER_MAX_Y: i32 = 112;

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
    deepslate: BlockStateId,
    water: BlockStateId,
    lava: BlockStateId,
    coal_ore: BlockStateId,
    iron_ore: BlockStateId,
    copper_ore: BlockStateId,
    deepslate_coal_ore: BlockStateId,
    deepslate_iron_ore: BlockStateId,
    deepslate_copper_ore: BlockStateId,
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
        let resolve_or = |name: &str, fallback: BlockStateId| -> BlockStateId {
            let id = Identifier::parse(name).expect("static identifier");
            registry.block(&id).map(|b| b.default).unwrap_or(fallback)
        };
        let plains = Identifier::parse("minecraft:plains").expect("static identifier");
        let air = resolve("minecraft:air");
        let stone = resolve("minecraft:stone");
        Self {
            seed,
            air,
            bedrock: resolve("minecraft:bedrock"),
            stone,
            dirt: resolve("minecraft:dirt"),
            grass_block: resolve("minecraft:grass_block"),
            deepslate: resolve_or("minecraft:deepslate", stone),
            water: resolve_or("minecraft:water", air),
            lava: resolve_or("minecraft:lava", air),
            coal_ore: resolve_or("minecraft:coal_ore", stone),
            iron_ore: resolve_or("minecraft:iron_ore", stone),
            copper_ore: resolve_or("minecraft:copper_ore", stone),
            deepslate_coal_ore: resolve_or("minecraft:deepslate_coal_ore", stone),
            deepslate_iron_ore: resolve_or("minecraft:deepslate_iron_ore", stone),
            deepslate_copper_ore: resolve_or("minecraft:deepslate_copper_ore", stone),
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
            let _ = chunk.set_block(lx, y, lz, self.base_stone_for_y(lx, y, lz, chunk.pos));
        }
        for y in dirt_start..height {
            let _ = chunk.set_block(lx, y, lz, self.dirt);
        }
        let _ = chunk.set_block(lx, height, lz, self.grass_block);
        // Air above stays as-is from Chunk::empty.
        let _ = self.air;
    }

    fn apply_features(&self, chunk: &mut Chunk, lx: u8, lz: u8, height: i32) {
        let wx = chunk.pos.x * 16 + lx as i32;
        let wz = chunk.pos.z * 16 + lz as i32;
        let cave_max_y = (height - CAVE_SURFACE_CLEARANCE).max(CAVE_MIN_Y);
        for y in (MIN_Y + 1)..height {
            if y >= CAVE_MIN_Y && y <= cave_max_y && self.is_cave_cell(wx, y, wz) {
                let fluid = self.cave_fluid(wx, y, wz);
                let _ = chunk.set_block(lx, y, lz, fluid.unwrap_or(self.air));
                continue;
            }

            if matches!(chunk.get_block(lx, y, lz), Some(state) if state == self.stone || state == self.deepslate)
            {
                let ore = self.ore_for(wx, y, wz, chunk.get_block(lx, y, lz).unwrap_or(self.stone));
                if ore != self.stone && ore != self.deepslate {
                    let _ = chunk.set_block(lx, y, lz, ore);
                }
            }
        }
    }

    fn base_stone_for_y(&self, lx: u8, y: i32, lz: u8, pos: ChunkPos) -> BlockStateId {
        if y <= DEEPSLATE_SOLID_Y {
            return self.deepslate;
        }
        if y > DEEPSLATE_TOP_Y {
            return self.stone;
        }
        let wx = pos.x * 16 + lx as i32;
        let wz = pos.z * 16 + lz as i32;
        let deepslate_chance = (DEEPSLATE_TOP_Y - y + 1) as u64;
        if feature_hash(self.seed, wx, y, wz, 0xD33F).is_multiple_of(9 - deepslate_chance) {
            self.deepslate
        } else {
            self.stone
        }
    }

    fn is_cave_cell(&self, x: i32, y: i32, z: i32) -> bool {
        let n = fbm_2d(
            x as f64 * CAVE_FREQUENCY,
            (z as f64 + y as f64 * 0.73) * CAVE_FREQUENCY,
            self.seed ^ 0x4341_5645,
            3,
            0.55,
        );
        n > CAVE_THRESHOLD
    }

    fn cave_fluid(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        let h = feature_hash(self.seed, x, y, z, 0xF17D);
        if y < MIN_Y + 16 && h.is_multiple_of(31) {
            Some(self.lava)
        } else if y < 48 && h.is_multiple_of(53) {
            Some(self.water)
        } else {
            None
        }
    }

    fn ore_for(&self, x: i32, y: i32, z: i32, base: BlockStateId) -> BlockStateId {
        let h = feature_hash(self.seed, x, y, z, 0x0A_E0);
        if (IRON_MIN_Y..=IRON_MAX_Y).contains(&y) && h.is_multiple_of(iron_spacing(y)) {
            self.ore_variant(base, self.iron_ore, self.deepslate_iron_ore)
        } else if (COPPER_MIN_Y..=COPPER_MAX_Y).contains(&y) && h.is_multiple_of(copper_spacing(y))
        {
            self.ore_variant(base, self.copper_ore, self.deepslate_copper_ore)
        } else if (COAL_MIN_Y..=COAL_MAX_Y).contains(&y) && h.is_multiple_of(coal_spacing(y)) {
            self.ore_variant(base, self.coal_ore, self.deepslate_coal_ore)
        } else {
            base
        }
    }

    fn ore_variant(
        &self,
        base: BlockStateId,
        stone_ore: BlockStateId,
        deepslate_ore: BlockStateId,
    ) -> BlockStateId {
        if base == self.deepslate {
            deepslate_ore
        } else {
            stone_ore
        }
    }
}

fn peaked_spacing(
    y: i32,
    min_y: i32,
    max_y: i32,
    peak_y: i32,
    min_spacing: u64,
    range: u64,
) -> u64 {
    let max_distance = (peak_y - min_y).abs().max((max_y - peak_y).abs()).max(1) as f64;
    let distance = (y - peak_y).abs() as f64 / max_distance;
    min_spacing + (distance * range as f64).round() as u64
}

fn coal_spacing(y: i32) -> u64 {
    peaked_spacing(y, COAL_MIN_Y, COAL_MAX_Y, 96, 83, 120)
}

fn iron_spacing(y: i32) -> u64 {
    peaked_spacing(y, IRON_MIN_Y, IRON_MAX_Y, 16, 97, 140)
}

fn copper_spacing(y: i32) -> u64 {
    peaked_spacing(y, COPPER_MIN_Y, COPPER_MAX_Y, 48, 89, 130)
}

fn feature_hash(seed: i64, x: i32, y: i32, z: i32, salt: u64) -> u64 {
    let mut h = seed as u64 ^ salt;
    h ^= (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = h.rotate_left(17);
    h ^= (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = h.rotate_left(23);
    h ^= (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h.wrapping_mul(0x94D0_49BB_1331_11EB) ^ (h >> 31)
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
                self.apply_features(&mut chunk, lx, lz, height);
                // Heightmap value: Y of the first air cell above the
                // top non-air block, expressed as `(top + 1) - MIN_Y`.
                let hm = (height + 1 - MIN_Y) as u32;
                if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                    mb.set(lx, lz, hm);
                }
                if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                    ws.set(lx, lz, hm);
                }
                chunk.highest_opaque.set(lx, lz, hm);
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
            BlockReport {
                id: Identifier::parse("minecraft:water").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 5,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:lava").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 6,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 7,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:coal_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 8,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:iron_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 9,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:copper_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 10,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_coal_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 11,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_iron_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 12,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_copper_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 13,
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
        assert_eq!(chunk.highest_opaque_y(8, 8), Some(height));

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

    #[test]
    fn feature_layer_adds_caves_ores_and_fluids() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let chunks = [
            g.generate(ChunkPos { x: 0, z: 0 }),
            g.generate(ChunkPos { x: 1, z: 0 }),
            g.generate(ChunkPos { x: 0, z: 1 }),
            g.generate(ChunkPos { x: -1, z: 0 }),
        ];
        let mut saw_cave_air = false;
        let mut saw_ore = false;
        let mut saw_deepslate = false;
        let mut saw_fluid = false;
        for chunk in chunks {
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = chunk.pos.x * 16 + lx as i32;
                    let wz = chunk.pos.z * 16 + lz as i32;
                    let top = g.surface_height(wx, wz);
                    for y in (MIN_Y + 1)..top - CAVE_SURFACE_CLEARANCE {
                        match chunk.get_block(lx, y, lz) {
                            Some(BlockStateId(0)) => saw_cave_air = true,
                            Some(BlockStateId(5)) | Some(BlockStateId(6)) => saw_fluid = true,
                            Some(BlockStateId(7)) => saw_deepslate = true,
                            Some(BlockStateId(8..=13)) => saw_ore = true,
                            _ => {}
                        }
                    }
                }
            }
        }

        assert!(saw_cave_air, "expected at least one carved cave cell");
        assert!(saw_ore, "expected at least one ore cell");
        assert!(
            saw_deepslate,
            "expected deepslate below the transition band"
        );
        assert!(saw_fluid, "expected at least one water/lava cave pocket");
    }
}
