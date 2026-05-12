//! Chunk: a 16×384×16 column at a fixed (x, z) coordinate, made of 24
//! stacked sections plus per-chunk side data (heightmaps, biomes,
//! block entities, generation status).
//!
//! Y range is hard-coded to the vanilla Overworld layout (Y=-64..320)
//! for M2. Dimensions that ship a different range — the Nether and
//! the End in vanilla, or a custom dimension type from a data pack —
//! aren't supported yet; this widens in M5 when dimensions become a
//! thing. (TODO(M5): replace MIN_Y/MAX_Y with per-dimension data.)
//!
//! For M2.d the chunk is mostly *storage*: a constructor that
//! produces an air-filled column, `get_block` / `set_block` that
//! route into the right section, and types ready for M2.e's Anvil
//! codec to populate (`heightmaps`, `biomes`, `block_entities`,
//! `status`).

use std::collections::HashMap;

use mc_data::Identifier;

use crate::block::BlockStateId;
use crate::section::{ChunkSection, PackedBitArray, SECTION_DIM};

/// Lowest world Y coordinate (inclusive).
pub const MIN_Y: i32 = -64;
/// One past the highest world Y coordinate (exclusive).
pub const MAX_Y: i32 = 320;
/// Number of stacked sections per chunk.
pub const SECTION_COUNT: usize = 24;
/// Section-y index of the bottom section. `section_y = section_index + MIN_SECTION_Y`.
pub const MIN_SECTION_Y: i32 = MIN_Y / SECTION_DIM as i32;
/// Bits needed to address a Y value in `0..=MAX_Y - MIN_Y` for heightmap
/// packing. `MAX_Y - MIN_Y = 384` so 9 bits suffice (`2^9 = 512`).
pub const HEIGHTMAP_BITS: u8 = 9;
/// One heightmap entry per (x, z) cell in the 16×16 chunk footprint.
pub const HEIGHTMAP_LEN: usize = SECTION_DIM * SECTION_DIM;
/// One biome cell per 4×4×4 sub-cube; vanilla packs biomes at 1/4 the
/// block resolution.
pub const BIOME_DIM: usize = 4;
pub const BIOME_VOLUME: usize = BIOME_DIM * BIOME_DIM * BIOME_DIM;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Biome storage for one section (4³ cells, palette of `Identifier`s).
///
/// Almost every section in a generated world has a single biome, so
/// `Single` is the overwhelmingly common case.
#[derive(Debug, Clone)]
pub enum BiomeSection {
    Single(Identifier),
    Indirect {
        palette: Vec<Identifier>,
        indices: PackedBitArray,
    },
}

impl BiomeSection {
    #[must_use]
    pub fn filled(biome: Identifier) -> Self {
        BiomeSection::Single(biome)
    }

    /// Build directly from a palette + packed indices, as the Anvil
    /// codec gets them from disk.
    #[must_use]
    pub fn from_indirect(palette: Vec<Identifier>, indices: PackedBitArray) -> Self {
        assert_eq!(indices.len(), BIOME_VOLUME);
        BiomeSection::Indirect { palette, indices }
    }

    /// Read a biome cell at the section-local 4×4×4 coordinate.
    #[must_use]
    pub fn get(&self, x: u8, y: u8, z: u8) -> &Identifier {
        debug_assert!((x as usize) < BIOME_DIM);
        debug_assert!((y as usize) < BIOME_DIM);
        debug_assert!((z as usize) < BIOME_DIM);
        match self {
            BiomeSection::Single(id) => id,
            BiomeSection::Indirect { palette, indices } => {
                let idx = (y as usize * BIOME_DIM + z as usize) * BIOME_DIM + x as usize;
                let p = indices.get(idx) as usize;
                &palette[p]
            }
        }
    }

    #[must_use]
    pub fn palette(&self) -> &[Identifier] {
        match self {
            BiomeSection::Single(id) => std::slice::from_ref(id),
            BiomeSection::Indirect { palette, .. } => palette,
        }
    }
}

/// One named heightmap (e.g. `MOTION_BLOCKING`, `WORLD_SURFACE`).
///
/// Stores 256 entries of `HEIGHTMAP_BITS` bits each, indexed by
/// `(z * 16) + x` (vanilla's heightmap indexing — note Z is outer, X
/// inner; this differs from the section's (y, z, x) order and matches
/// what `.mca` files contain).
#[derive(Debug, Clone)]
pub struct Heightmap {
    data: PackedBitArray,
}

impl Heightmap {
    #[must_use]
    pub fn zeroed() -> Self {
        Self {
            data: PackedBitArray::zeroed(HEIGHTMAP_BITS, HEIGHTMAP_LEN),
        }
    }

    /// Build a heightmap directly from the packed `i64[]` Anvil stores
    /// on disk (a single NBT `LongArray` per heightmap kind).
    #[must_use]
    pub fn from_long_array(longs: &[i64]) -> Self {
        let words: Vec<u64> = longs.iter().map(|&l| l as u64).collect();
        Self {
            data: PackedBitArray::from_words(HEIGHTMAP_BITS, HEIGHTMAP_LEN, words),
        }
    }

    /// Raw long-array form for emission back to NBT.
    #[must_use]
    pub fn to_long_array(&self) -> Vec<i64> {
        self.data.words().iter().map(|&w| w as i64).collect()
    }

    #[must_use]
    pub fn get(&self, x: u8, z: u8) -> u32 {
        debug_assert!((x as usize) < SECTION_DIM);
        debug_assert!((z as usize) < SECTION_DIM);
        self.data.get(z as usize * SECTION_DIM + x as usize)
    }

    pub fn set(&mut self, x: u8, z: u8, height: u32) {
        debug_assert!((x as usize) < SECTION_DIM);
        debug_assert!((z as usize) < SECTION_DIM);
        self.data.set(z as usize * SECTION_DIM + x as usize, height);
    }

    /// Raw packed words, for the Anvil codec.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        self.data.words()
    }
}

/// A 16×384×16 column.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    pub sections: Vec<ChunkSection>,
    pub biomes: Vec<BiomeSection>,
    /// Named heightmaps as Anvil stores them: `MOTION_BLOCKING`,
    /// `MOTION_BLOCKING_NO_LEAVES`, `OCEAN_FLOOR`, `WORLD_SURFACE`,
    /// plus the worldgen-time `*_WG` variants. We don't enforce the
    /// set here — the Anvil codec stores whatever it reads.
    pub heightmaps: HashMap<String, Heightmap>,
    /// Block-entity NBT, keyed by absolute world position. Stored
    /// opaque (raw Java-standard NBT bytes) until M3 needs typed
    /// access.
    pub block_entities: HashMap<BlockPos, Vec<u8>>,
    /// Vanilla generation status (`"full"`, `"biomes"`, `"structure_starts"`, …).
    pub status: String,
}

impl Chunk {
    /// A column filled with `air` blocks and `biome` everywhere, no
    /// block entities, no heightmaps, status `"full"`.
    #[must_use]
    pub fn empty(pos: ChunkPos, air: BlockStateId, biome: Identifier) -> Self {
        Self {
            pos,
            sections: (0..SECTION_COUNT)
                .map(|_| ChunkSection::filled(air, air))
                .collect(),
            biomes: (0..SECTION_COUNT)
                .map(|_| BiomeSection::filled(biome.clone()))
                .collect(),
            heightmaps: HashMap::new(),
            block_entities: HashMap::new(),
            status: "full".to_string(),
        }
    }

    /// Look up a block by chunk-local (x, z) and absolute world y.
    /// `None` when `y` is outside `MIN_Y..MAX_Y`.
    #[must_use]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<BlockStateId> {
        let (idx, sy) = world_y_to_section(y)?;
        Some(self.sections[idx].get(x, sy, z))
    }

    /// Set a block; returns the previous state, or `None` if `y` is
    /// outside the chunk.
    pub fn set_block(&mut self, x: u8, y: i32, z: u8, state: BlockStateId) -> Option<BlockStateId> {
        let (idx, sy) = world_y_to_section(y)?;
        Some(self.sections[idx].set(x, sy, z, state))
    }
}

fn world_y_to_section(y: i32) -> Option<(usize, u8)> {
    if !(MIN_Y..MAX_Y).contains(&y) {
        return None;
    }
    let local = y - MIN_Y;
    Some(((local / SECTION_DIM as i32) as usize, (local % 16) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn air() -> BlockStateId {
        BlockStateId(0)
    }
    fn stone() -> BlockStateId {
        BlockStateId(1)
    }
    fn plains() -> Identifier {
        Identifier::parse("minecraft:plains").unwrap()
    }

    #[test]
    fn y_mapping_covers_full_range() {
        assert_eq!(world_y_to_section(MIN_Y), Some((0, 0)));
        assert_eq!(world_y_to_section(MAX_Y - 1), Some((SECTION_COUNT - 1, 15)));
        assert_eq!(world_y_to_section(0), Some((4, 0)));
        assert_eq!(world_y_to_section(MIN_Y - 1), None);
        assert_eq!(world_y_to_section(MAX_Y), None);
    }

    #[test]
    fn empty_chunk_is_all_air() {
        let c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        assert_eq!(c.sections.len(), SECTION_COUNT);
        assert_eq!(c.biomes.len(), SECTION_COUNT);
        assert_eq!(c.get_block(0, MIN_Y, 0), Some(air()));
        assert_eq!(c.get_block(15, MAX_Y - 1, 15), Some(air()));
        assert_eq!(c.get_block(0, MIN_Y - 1, 0), None);
        assert_eq!(c.get_block(0, MAX_Y, 0), None);
    }

    #[test]
    fn set_block_round_trips_across_sections() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        // Set at the bottom, the build-height boundary, and the top.
        let probes = [MIN_Y, -1, 0, 63, 319];
        for &y in &probes {
            assert_eq!(c.set_block(3, y, 7, stone()), Some(air()));
            assert_eq!(c.get_block(3, y, 7), Some(stone()));
            assert_eq!(c.get_block(2, y, 7), Some(air()));
        }
        // Non-probed columns still air.
        assert_eq!(c.get_block(0, 0, 0), Some(air()));
    }

    #[test]
    fn biome_section_default_is_single() {
        let b = BiomeSection::filled(plains());
        for y in 0..4 {
            for z in 0..4 {
                for x in 0..4 {
                    assert_eq!(b.get(x, y, z).as_str(), "minecraft:plains");
                }
            }
        }
    }

    #[test]
    fn heightmap_round_trip() {
        let mut h = Heightmap::zeroed();
        h.set(0, 0, 64);
        h.set(15, 15, 320);
        h.set(7, 3, 0);
        assert_eq!(h.get(0, 0), 64);
        assert_eq!(h.get(15, 15), 320);
        assert_eq!(h.get(7, 3), 0);
        assert_eq!(h.get(1, 1), 0);
    }

    #[test]
    fn min_section_y_constant_is_correct() {
        assert_eq!(MIN_SECTION_Y, -4);
        assert_eq!(MIN_SECTION_Y + SECTION_COUNT as i32, 20);
    }
}
