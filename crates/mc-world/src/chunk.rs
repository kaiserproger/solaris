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
use mc_nbt::Tag;

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
/// Bytes per per-section light layer: `16³` cells × 4 bits per cell.
pub const LIGHT_LAYER_BYTES: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM / 2;

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

/// Per-section 4-bit-per-cell light arrays. Both layers are `None`
/// when Anvil didn't write them — vanilla omits the array when the
/// section is uniformly default (sky-15 above terrain, block-0
/// everywhere), and our test world is mostly in pre-`light`
/// generation status so most sections come back as `None`.
///
/// When present, the buffer holds `LIGHT_LAYER_BYTES` (2048) bytes
/// in the same `(y, z, x)` linear order the section block-state
/// container uses, two cells packed per byte (low nibble first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionLight {
    pub block: Option<Vec<u8>>,
    pub sky: Option<Vec<u8>>,
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
    /// Baked light arrays read from Anvil, one entry per
    /// `sections[i]`. Decoded by `anvil::chunk_nbt` when present;
    /// `Chunk::empty` initialises everything to `None` (sections
    /// have no baked light). The wire encoder may use these as the
    /// source of truth or fall back to recomputed light — see
    /// `mc_world::light`.
    pub section_lights: Vec<SectionLight>,
    /// Root-level NBT fields M5.c keeps for byte-stable round-trip
    /// through the Anvil codec without modelling them yet. Covers
    /// what M2 dropped on decode: structures, `block_ticks`,
    /// `fluid_ticks`, `PostProcessing`, `InhabitedTime`,
    /// `LastUpdate`, `DataVersion`, plus any future field whose key
    /// isn't in our modelled set. Order preserved from the original
    /// compound so a load-then-save produces a stable byte stream
    /// (modulo NBT compound ordering being unspecified — vanilla
    /// reads it back identically either way).
    pub extras: Vec<(String, Tag)>,
    /// M6.b: set when an edit changes a stored block-state id; cleared
    /// after the chunk is flushed back to its `.mca`. Decoders default
    /// this to `false` so loading a fresh region from disk does not
    /// re-trigger a write.
    pub dirty: bool,
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
            section_lights: vec![SectionLight::default(); SECTION_COUNT],
            extras: Vec::new(),
            dirty: false,
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

    /// Set a block *and* refresh every heightmap currently attached
    /// to this chunk so the new top-non-air-column matches reality.
    /// Returns the previous state (or `None` if `y` is outside the
    /// chunk). `air` is the registry's default air state — the
    /// heightmap predicate is `state != air`, matching vanilla's
    /// `MOTION_BLOCKING` / `WORLD_SURFACE` / `OCEAN_FLOOR`
    /// definitions closely enough for M5's flat-preset world.
    ///
    /// Recomputes *every* heightmap currently present on the chunk,
    /// not a hardcoded set — different vanilla generators produce
    /// different subsets (e.g. mid-generation `*_WG` variants), so
    /// touching only known names would silently leave stale values
    /// on partial chunks.
    pub fn set_block_and_update(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        air: BlockStateId,
    ) -> Option<BlockStateId> {
        let prev = self.set_block(x, y, z, state)?;
        if prev == state {
            return Some(prev);
        }
        self.dirty = true;
        // Heightmap entries store `height + 1`-style values (the Y of
        // the first air cell above the column), matching vanilla's
        // on-disk packing. Recompute the column for every present
        // heightmap. Cheap: one walk down per heightmap per edit.
        let names: Vec<String> = self.heightmaps.keys().cloned().collect();
        for name in names {
            let new_top = top_non_air_column(self, x, z, air);
            if let Some(hm) = self.heightmaps.get_mut(&name) {
                hm.set(x, z, new_top);
            }
        }
        Some(prev)
    }
}

/// Walk column `(x, z)` from `MAX_Y - 1` down to `MIN_Y` and return
/// the heightmap value vanilla stores: `top_y + 1 - MIN_Y` (the Y
/// of the first air cell above the topmost non-air block, expressed
/// as an offset from the bottom of the world). For an entirely-air
/// column the value is `0`, matching vanilla's "empty column"
/// convention.
fn top_non_air_column(chunk: &Chunk, x: u8, z: u8, air: BlockStateId) -> u32 {
    for y in (MIN_Y..MAX_Y).rev() {
        if chunk.get_block(x, y, z) != Some(air) {
            // vanilla packs heightmap values as `height - MIN_Y + 1`
            // so a top block at world Y=64 stores as `64 - (-64) + 1`
            // = 129. The +1 makes "the lowest cell above the top
            // block" the value, matching the meaning vanilla uses
            // for chunk-spawning and skylight shortcuts.
            return (y - MIN_Y + 1) as u32;
        }
    }
    0
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

    #[test]
    fn set_block_and_update_recomputes_present_heightmaps() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        // Pre-existing heightmap (e.g. surface-Y from generation):
        // empty column → value 0. Insert it explicitly so the helper
        // has something to recompute.
        c.heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        c.heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

        // Drop a stone at world Y=64. The heightmap value is
        // (Y - MIN_Y + 1) = 64 - (-64) + 1 = 129.
        let prev = c.set_block_and_update(3, 64, 7, stone(), air()).unwrap();
        assert_eq!(prev, air());
        assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 129);
        assert_eq!(c.heightmaps.get("WORLD_SURFACE").unwrap().get(3, 7), 129);

        // Other columns stay at 0.
        assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(0, 0), 0);

        // Break it back to air — heightmap returns to 0.
        c.set_block_and_update(3, 64, 7, air(), air()).unwrap();
        assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 0);
    }

    #[test]
    fn set_block_and_update_picks_next_lower_top_after_break() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        c.heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        // Stack two blocks at Y=63 and Y=64; heightmap should track
        // the higher one and fall back to the lower on break.
        c.set_block_and_update(3, 63, 7, stone(), air()).unwrap();
        c.set_block_and_update(3, 64, 7, stone(), air()).unwrap();
        assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 129);

        c.set_block_and_update(3, 64, 7, air(), air()).unwrap();
        assert_eq!(c.heightmaps.get("MOTION_BLOCKING").unwrap().get(3, 7), 128);
    }

    #[test]
    fn set_block_and_update_with_no_heightmaps_is_a_noop_for_them() {
        // Some chunks (Status: empty / structure_starts in our test
        // world) ship no heightmaps. The helper must not panic and
        // must still mutate the underlying block.
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        assert!(c.heightmaps.is_empty());
        let prev = c.set_block_and_update(0, 0, 0, stone(), air()).unwrap();
        assert_eq!(prev, air());
        assert_eq!(c.get_block(0, 0, 0), Some(stone()));
        assert!(c.heightmaps.is_empty());
    }
}
