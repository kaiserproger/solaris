//! Nether chunk router (generation only).
//!
//! Bedrock floor + ceiling, netherrack body shaped by a smooth height field,
//! lava sea where the field dips below [`NETHER_LAVA_LEVEL`]. Deterministic
//! from seed and world coordinates; independent of chunk generation order.
//!
//! Single-biome (`minecraft:nether_wastes`) for now. Multi-biome regions
//! (basalt deltas, soul sand valley, crimson/warped forests), ores, glowstone
//! and fortresses are queued. No portals, travel, or respawn: the server is
//! still single-dimension (see the dimensions scout in `docs/MEMORY.md`).

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkGeometry, ChunkPos};

use super::noise::fbm_2d;
use std::sync::Arc;

pub const NETHER_MIN_Y: i32 = 0;
pub const NETHER_HEIGHT: i32 = 256;
/// Lava sea level (vanilla Nether dimension constant).
pub const NETHER_LAVA_LEVEL: i32 = 32;
/// Top bedrock cap (vanilla Nether dimension constant).
pub const NETHER_CEILING_Y: i32 = 127;
/// Rough underside band under the cap (mixed bedrock/netherrack).
pub const NETHER_CEILING_BAND: i32 = 4;

#[must_use]
pub fn nether_geometry() -> ChunkGeometry {
    ChunkGeometry::new(NETHER_MIN_Y, NETHER_HEIGHT).expect("static nether geometry")
}

/// Resolved Nether block ids plus generation constants. Holds the resolved
/// state ids it emits so `generate` stays allocation-free past `Chunk::empty`.
pub struct NetherGenerator {
    seed: i64,
    geometry: ChunkGeometry,
    air: BlockStateId,
    bedrock: BlockStateId,
    netherrack: BlockStateId,
    lava: BlockStateId,
    biome: Identifier,
}

#[derive(Debug, thiserror::Error)]
pub enum NetherGeneratorError {
    #[error("block registry is missing {0}")]
    MissingRequiredBlock(&'static str),
}

impl NetherGenerator {
    /// Build a generator, failing on missing required vanilla blocks instead
    /// of panicking, so startup can report configuration errors.
    ///
    /// # Errors
    ///
    /// Returns [`NetherGeneratorError::MissingRequiredBlock`] when the block
    /// registry lacks `minecraft:air`, `minecraft:bedrock`,
    /// `minecraft:netherrack`, or source `minecraft:lava`.
    pub fn try_new(seed: i64, registry: Arc<BlockRegistry>) -> Result<Self, NetherGeneratorError> {
        let air = try_resolve(registry.as_ref(), "minecraft:air")?;
        let bedrock = try_resolve(registry.as_ref(), "minecraft:bedrock")?;
        let netherrack = try_resolve(registry.as_ref(), "minecraft:netherrack")?;
        let lava_id = Identifier::parse("minecraft:lava").expect("static identifier");
        let lava = registry
            .by_name_and_props(&lava_id, &[("level".to_string(), "0".to_string())])
            .ok_or(NetherGeneratorError::MissingRequiredBlock("minecraft:lava"))?;
        Ok(Self {
            seed,
            geometry: nether_geometry(),
            air,
            bedrock,
            netherrack,
            lava,
            biome: Identifier::parse("minecraft:nether_wastes").expect("static identifier"),
        })
    }

    /// Build a generator, panicking on missing required blocks.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla Nether blocks. Use
    /// [`NetherGenerator::try_new`] for fallible startup validation.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::try_new(seed, registry).unwrap_or_else(|err| panic!("{err}"))
    }

    fn column_height(&self, wx: i32, wz: i32) -> i32 {
        let field = fbm_2d(wx as f64 / 64.0, wz as f64 / 64.0, self.seed, 3, 0.5);
        // Solaris-owned tuning range (not a vanilla claim): dips at/below the
        // lava level form lava lakes, higher ground rises toward the ceiling.
        // Vanilla side-by-side terrain comparison is queued.
        (52.0 + 30.0 * field).round().clamp(22.0, 82.0) as i32
    }

    fn ceiling_is_bedrock(&self, wx: i32, y: i32, wz: i32) -> bool {
        if y == NETHER_CEILING_Y {
            return true;
        }
        // Rough underside: deterministic hash picks bedrock teeth.
        let hash = splitmix64(
            (self.seed as u64)
                .wrapping_add(wx as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(wz as u64)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                .wrapping_add(y as u64),
        );
        hash & 0b11 == 0
    }
}

fn try_resolve(
    registry: &BlockRegistry,
    name: &'static str,
) -> Result<BlockStateId, NetherGeneratorError> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|block| block.default)
        .ok_or(NetherGeneratorError::MissingRequiredBlock(name))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

impl ChunkGenerator for NetherGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk =
            Chunk::empty_with_geometry(pos, self.air, self.biome.clone(), self.geometry);
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                let wx = pos.x * 16 + i32::from(lx);
                let wz = pos.z * 16 + i32::from(lz);
                let height = self.column_height(wx, wz);
                let _ = chunk.set_block(lx, NETHER_MIN_Y, lz, self.bedrock);
                // Solid body: land columns are netherrack up to the field;
                // lake columns (field at/below sea) hold lava up to sea level.
                // Never bury lava under rock, never leave hollow air to the roof.
                let lake = height <= NETHER_LAVA_LEVEL;
                for y in 1..NETHER_CEILING_Y - NETHER_CEILING_BAND {
                    let state = if lake {
                        if y < NETHER_LAVA_LEVEL {
                            self.lava
                        } else {
                            self.air
                        }
                    } else if y < height {
                        self.netherrack
                    } else {
                        self.air
                    };
                    let _ = chunk.set_block(lx, y, lz, state);
                }
                for y in NETHER_CEILING_Y - NETHER_CEILING_BAND..=NETHER_CEILING_Y {
                    let state = if self.ceiling_is_bedrock(wx, y, wz) {
                        self.bedrock
                    } else {
                        self.netherrack
                    };
                    let _ = chunk.set_block(lx, y, lz, state);
                }
                for y in NETHER_CEILING_Y + 1..NETHER_MIN_Y + NETHER_HEIGHT {
                    let _ = chunk.set_block(lx, y, lz, self.air);
                }
            }
        }
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        chunk
    }
}

#[cfg(test)]
#[path = "nether/tests.rs"]
mod tests;
