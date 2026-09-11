//! End chunk router (generation only).
//!
//! End-stone island body shaped by a smooth height field that falls to void
//! past the island edge, plus an obsidian pillar ring with bedrock caps near
//! the origin island. Deterministic from seed and world coordinates;
//! independent of chunk generation order.
//!
//! Single-biome (`minecraft:the_end`) for now, chorus-free: no chorus plants,
//! end cities, or gateway portals. No portals, travel, or respawn: the server
//! is still single-dimension (see the dimensions scout in `docs/MEMORY.md`).

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkGeometry, ChunkPos};

use super::noise::fbm_2d;
use std::f64::consts::PI;
use std::sync::Arc;

pub const END_MIN_Y: i32 = 0;
pub const END_HEIGHT: i32 = 256;
/// Main-island radius in blocks (Solaris-owned tuning, not a vanilla claim:
/// controls where the height field falls to void. Vanilla side-by-side
/// terrain comparison is queued).
pub const END_ISLAND_RADIUS: f64 = 64.0;
/// Island surface baseline in blocks before noise and edge falloff
/// (Solaris-owned tuning, not a vanilla claim).
pub const END_ISLAND_BASE_Y: f64 = 64.0;
/// Height-field amplitude in blocks (Solaris-owned tuning, not a vanilla claim).
pub const END_ISLAND_AMPLITUDE: f64 = 12.0;
/// Quadratic edge falloff in blocks at the island rim, i.e. what drops the
/// rim to void (Solaris-owned tuning, not a vanilla claim).
pub const END_ISLAND_FALLOFF: f64 = 96.0;
/// Noise input scale divisor (Solaris-owned tuning, not a vanilla claim).
pub const END_NOISE_SCALE: f64 = 96.0;
/// Pillar ring size. The shaft/cap materials (obsidian with bedrock caps)
/// match the vanilla End island pillars; the count, positions, heights, and
/// square footprint below are Solaris-owned simplifications (not vanilla
/// claims): no crystals, iron bars, or cages. Vanilla side-by-side
/// comparison is queued.
pub const END_PILLAR_COUNT: usize = 10;
/// Pillar ring radius in blocks (Solaris-owned tuning, not a vanilla claim).
pub const END_PILLAR_RADIUS: f64 = 42.0;
/// Shortest pillar shaft in blocks (Solaris-owned tuning, not a vanilla claim).
pub const END_PILLAR_MIN_HEIGHT: i32 = 6;
/// Shaft height span above the minimum, i.e. heights `MIN..MIN + SPAN`
/// (Solaris-owned tuning, not a vanilla claim).
pub const END_PILLAR_HEIGHT_SPAN: i32 = 12;
/// Pillar half-width: shafts and caps are `(2 * HALF + 1)` blocks square
/// (Solaris-owned tuning, not a vanilla claim).
pub const END_PILLAR_HALF_WIDTH: i32 = 1;

#[must_use]
pub fn end_geometry() -> ChunkGeometry {
    ChunkGeometry::new(END_MIN_Y, END_HEIGHT).expect("static end geometry")
}

/// Resolved End block ids plus generation constants. Holds the resolved
/// state ids it emits so `generate` stays allocation-free past `Chunk::empty`.
pub struct EndGenerator {
    seed: i64,
    geometry: ChunkGeometry,
    air: BlockStateId,
    end_stone: BlockStateId,
    obsidian: BlockStateId,
    bedrock: BlockStateId,
    biome: Identifier,
}

#[derive(Debug, thiserror::Error)]
pub enum EndGeneratorError {
    #[error("block registry is missing {0}")]
    MissingRequiredBlock(&'static str),
}

/// One pillar shaft plus its cap row. Derived purely from `(seed, index)`.
struct Pillar {
    cx: i32,
    cz: i32,
    base_y: i32,
    top_y: i32,
}

impl EndGenerator {
    /// Build a generator, failing on missing required vanilla blocks instead
    /// of panicking, so startup can report configuration errors.
    ///
    /// # Errors
    ///
    /// Returns [`EndGeneratorError::MissingRequiredBlock`] when the block
    /// registry lacks `minecraft:air`, `minecraft:end_stone`,
    /// `minecraft:obsidian`, or `minecraft:bedrock`.
    pub fn try_new(seed: i64, registry: Arc<BlockRegistry>) -> Result<Self, EndGeneratorError> {
        let air = try_resolve(registry.as_ref(), "minecraft:air")?;
        let end_stone = try_resolve(registry.as_ref(), "minecraft:end_stone")?;
        let obsidian = try_resolve(registry.as_ref(), "minecraft:obsidian")?;
        let bedrock = try_resolve(registry.as_ref(), "minecraft:bedrock")?;
        Ok(Self {
            seed,
            geometry: end_geometry(),
            air,
            end_stone,
            obsidian,
            bedrock,
            biome: Identifier::parse("minecraft:the_end").expect("static identifier"),
        })
    }

    /// Build a generator, panicking on missing required blocks.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla End blocks. Use
    /// [`EndGenerator::try_new`] for fallible startup validation.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::try_new(seed, registry).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Island surface height in world `y`. Columns whose surface drops below
    /// [`END_MIN_Y`] are void (all air).
    fn surface_height(&self, wx: i32, wz: i32) -> i32 {
        let distance = (wx as f64).hypot(wz as f64);
        let edge = distance / END_ISLAND_RADIUS;
        let field = fbm_2d(
            wx as f64 / END_NOISE_SCALE,
            wz as f64 / END_NOISE_SCALE,
            self.seed,
            3,
            0.5,
        );
        // Solaris-owned shaping (not a vanilla claim): gentle noise dome minus
        // a quadratic rim falloff. Vanilla side-by-side terrain comparison is
        // queued.
        (END_ISLAND_BASE_Y + END_ISLAND_AMPLITUDE * field - END_ISLAND_FALLOFF * edge * edge)
            .round() as i32
    }

    /// Pillar `index` placement, or `None` when its center column is void
    /// (never happens on the origin island; guards far-ring reuse).
    fn pillar(&self, index: usize) -> Option<Pillar> {
        let angle = 2.0 * PI * index as f64 / END_PILLAR_COUNT as f64;
        let cx = (END_PILLAR_RADIUS * angle.cos()).round() as i32;
        let cz = (END_PILLAR_RADIUS * angle.sin()).round() as i32;
        let base_y = self.surface_height(cx, cz);
        if base_y < END_MIN_Y {
            return None;
        }
        // Deterministic per-seed shaft height; order-free like everything else.
        let hash = splitmix64(
            (self.seed as u64)
                .wrapping_add(0x51AB_6D4E_77C2_1B77)
                .wrapping_add(index as u64),
        );
        let height = END_PILLAR_MIN_HEIGHT + (hash % END_PILLAR_HEIGHT_SPAN as u64) as i32;
        Some(Pillar {
            cx,
            cz,
            base_y,
            top_y: base_y + height,
        })
    }
}

fn try_resolve(
    registry: &BlockRegistry,
    name: &'static str,
) -> Result<BlockStateId, EndGeneratorError> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|block| block.default)
        .ok_or(EndGeneratorError::MissingRequiredBlock(name))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

impl ChunkGenerator for EndGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk =
            Chunk::empty_with_geometry(pos, self.air, self.biome.clone(), self.geometry);
        // Island body: solid end stone from the floor up to the field, air
        // above; void columns (surface below the floor) stay all air.
        for lx in 0..16u8 {
            for lz in 0..16u8 {
                let wx = pos.x * 16 + i32::from(lx);
                let wz = pos.z * 16 + i32::from(lz);
                let surface = self.surface_height(wx, wz);
                for y in END_MIN_Y..END_MIN_Y + END_HEIGHT {
                    let state = if y <= surface {
                        self.end_stone
                    } else {
                        self.air
                    };
                    let _ = chunk.set_block(lx, y, lz, state);
                }
            }
        }
        // Pillar ring overlay: pure function of (seed, index), so any chunk
        // covering a footprint paints the same blocks regardless of order.
        for index in 0..END_PILLAR_COUNT {
            if let Some(pillar) = self.pillar(index) {
                let cap_y = pillar.top_y + 1;
                for dx in -END_PILLAR_HALF_WIDTH..=END_PILLAR_HALF_WIDTH {
                    for dz in -END_PILLAR_HALF_WIDTH..=END_PILLAR_HALF_WIDTH {
                        let wx = pillar.cx + dx;
                        let wz = pillar.cz + dz;
                        let lx = wx - pos.x * 16;
                        let lz = wz - pos.z * 16;
                        if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
                            continue;
                        }
                        let (lx, lz) = (lx as u8, lz as u8);
                        for y in pillar.base_y + 1..=pillar.top_y {
                            let _ = chunk.set_block(lx, y, lz, self.obsidian);
                        }
                        let _ = chunk.set_block(lx, cap_y, lz, self.bedrock);
                    }
                }
            }
        }
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        chunk
    }
}

#[cfg(test)]
#[path = "end/tests.rs"]
mod tests;
