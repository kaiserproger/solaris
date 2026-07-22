//! Chunk: a 16×height×16 column at a fixed (x, z) coordinate, made of
//! stacked sections plus per-chunk side data (heightmaps, biomes,
//! block entities, generation status).
//!
//! `Chunk::empty` keeps the vanilla Overworld layout (Y=-64..320).
//! `Chunk::empty_with_geometry` supports another section-aligned block
//! storage range. Anvil, wire, and light geometry are migrated separately.
//!
//! For M2.d the chunk is mostly *storage*: a constructor that
//! produces an air-filled column, `get_block` / `set_block` that
//! route into the right section, and types ready for M2.e's Anvil
//! codec to populate (`heightmaps`, `biomes`, `block_entities`,
//! `status`).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_nbt::Tag;

use crate::block::BlockStateId;
use crate::light::ChunkLight;
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
const WORLD_JOURNAL_LSN_KEY: &str = "SolarisJournalLsn";
/// One biome cell per 4×4×4 sub-cube; vanilla packs biomes at 1/4 the
/// block resolution.
pub const BIOME_DIM: usize = 4;
pub const BIOME_VOLUME: usize = BIOME_DIM * BIOME_DIM * BIOME_DIM;
/// Bytes per per-section light layer: `16³` cells × 4 bits per cell.
pub const LIGHT_LAYER_BYTES: usize = SECTION_DIM * SECTION_DIM * SECTION_DIM / 2;
static NEXT_CHUNK_RUNTIME_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkGeometry {
    min_y: i32,
    max_y: i32,
}

impl ChunkGeometry {
    /// Returns `None` unless the range is section-aligned and fits the
    /// current 9-bit heightmap representation.
    #[must_use]
    pub fn new(min_y: i32, height: i32) -> Option<Self> {
        if min_y.rem_euclid(SECTION_DIM as i32) != 0
            || height <= 0
            || height > (1_i32 << HEIGHTMAP_BITS) - 1
            || height.rem_euclid(SECTION_DIM as i32) != 0
        {
            return None;
        }
        let max_y = min_y.checked_add(height)?;
        Some(Self { min_y, max_y })
    }

    #[must_use]
    pub const fn min_y(self) -> i32 {
        self.min_y
    }

    #[must_use]
    pub const fn max_y(self) -> i32 {
        self.max_y
    }

    #[must_use]
    pub const fn height(self) -> i32 {
        self.max_y - self.min_y
    }

    #[must_use]
    pub const fn section_count(self) -> usize {
        (self.height() / SECTION_DIM as i32) as usize
    }

    fn world_y_to_section(self, y: i32) -> Option<(usize, u8)> {
        if !(self.min_y..self.max_y).contains(&y) {
            return None;
        }
        let local = y - self.min_y;
        Some((
            (local / SECTION_DIM as i32) as usize,
            (local % SECTION_DIM as i32) as u8,
        ))
    }
}

pub const OVERWORLD_GEOMETRY: ChunkGeometry = ChunkGeometry {
    min_y: MIN_Y,
    max_y: MAX_Y,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockMutationToken {
    pub chunk_instance_id: u64,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkLightSourceToken {
    chunk_instance_id: u64,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledBlockTick {
    pub pos: BlockPos,
    pub block: Identifier,
    pub trigger_tick: u64,
    pub priority: i32,
    sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledFluidTick {
    pub pos: BlockPos,
    pub fluid: Identifier,
    pub trigger_tick: u64,
    pub priority: i32,
    sequence: u64,
}

impl ScheduledFluidTick {
    #[must_use]
    pub fn new(pos: BlockPos, fluid: Identifier, trigger_tick: u64, priority: i32) -> Self {
        Self {
            pos,
            fluid,
            trigger_tick,
            priority,
            sequence: 0,
        }
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn from_storage(
        pos: BlockPos,
        fluid: Identifier,
        trigger_tick: u64,
        priority: i32,
        sequence: u64,
    ) -> Self {
        Self {
            pos,
            fluid,
            trigger_tick,
            priority,
            sequence,
        }
    }

    fn sort_key(&self) -> (u64, i32, u64) {
        (self.trigger_tick, self.priority, self.sequence)
    }

    fn same_request(&self, other: &ScheduledFluidTick) -> bool {
        self.pos == other.pos
            && self.fluid == other.fluid
            && self.trigger_tick == other.trigger_tick
            && self.priority == other.priority
    }
}

impl ScheduledBlockTick {
    #[must_use]
    pub fn new(pos: BlockPos, block: Identifier, trigger_tick: u64, priority: i32) -> Self {
        Self {
            pos,
            block,
            trigger_tick,
            priority,
            sequence: 0,
        }
    }

    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub(crate) fn from_storage(
        pos: BlockPos,
        block: Identifier,
        trigger_tick: u64,
        priority: i32,
        sequence: u64,
    ) -> Self {
        Self {
            pos,
            block,
            trigger_tick,
            priority,
            sequence,
        }
    }

    fn sort_key(&self) -> (u64, i32, u64) {
        (self.trigger_tick, self.priority, self.sequence)
    }

    fn same_request(&self, other: &ScheduledBlockTick) -> bool {
        self.pos == other.pos
            && self.block == other.block
            && self.trigger_tick == other.trigger_tick
            && self.priority == other.priority
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FurnaceSlot {
    pub count: i32,
    pub item_id: u32,
    pub damage: Option<i32>,
    pub enchantments: Vec<mc_data::ItemEnchantment>,
}

impl FurnaceSlot {
    pub const EMPTY: FurnaceSlot = FurnaceSlot {
        count: 0,
        item_id: 0,
        damage: None,
        enchantments: Vec::new(),
    };

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FurnaceBlockEntity {
    pub slots: [FurnaceSlot; 3],
    pub burn_remaining: i16,
    pub burn_total: i16,
    pub cook_progress: i16,
    pub cook_total: i16,
    pub recipes_used: BTreeMap<String, i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChestBlockEntity {
    pub slots: [FurnaceSlot; 27],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopperBlockEntity {
    pub slots: [FurnaceSlot; 5],
    pub transfer_cooldown: i32,
}

impl Default for ChestBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
        }
    }
}

impl Default for HopperBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            transfer_cooldown: -1,
        }
    }
}

impl Default for FurnaceBlockEntity {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| FurnaceSlot::EMPTY),
            burn_remaining: 0,
            burn_total: 1600,
            cook_progress: 0,
            cook_total: 200,
            recipes_used: BTreeMap::new(),
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockLightMutation {
    Invalidate,
    PreserveInert,
    RetainForRelight,
}

/// A 16×height×16 column.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    geometry: ChunkGeometry,
    pub sections: Vec<ChunkSection>,
    pub biomes: Vec<BiomeSection>,
    /// Named heightmaps as Anvil stores them: `MOTION_BLOCKING`,
    /// `MOTION_BLOCKING_NO_LEAVES`, `OCEAN_FLOOR`, `WORLD_SURFACE`,
    /// plus the worldgen-time `*_WG` variants. We don't enforce the
    /// set here — the Anvil codec stores whatever it reads.
    pub heightmaps: HashMap<String, Heightmap>,
    /// Derived highest sky-blocking cell per column, stored as the
    /// same `top + 1 - MIN_Y` offset as vanilla heightmaps. This is not
    /// serialized as an Anvil heightmap; it is maintained from the
    /// light table for spawn picking and sky-relight shortcuts.
    pub highest_opaque: Heightmap,
    /// Block-entity NBT, keyed by absolute world position. Stored
    /// opaque (raw Java-standard NBT bytes) until M3 needs typed
    /// access.
    pub block_entities: HashMap<BlockPos, Vec<u8>>,
    /// Runtime-typed furnace state keyed by absolute world position.
    pub furnaces: HashMap<BlockPos, FurnaceBlockEntity>,
    /// Runtime-typed single chest state keyed by absolute world position.
    pub chests: HashMap<BlockPos, ChestBlockEntity>,
    /// Runtime-typed hopper state keyed by absolute world position.
    pub hoppers: HashMap<BlockPos, HopperBlockEntity>,
    /// Chunk-owned scheduled block ticks. Runtime code orders by
    /// trigger tick, priority, and insertion sequence; Anvil persistence
    /// is wired in separately once the local 26.1.2 shape is verified.
    pub scheduled_block_ticks: Vec<ScheduledBlockTick>,
    next_scheduled_block_tick_sequence: u64,
    /// Chunk-owned scheduled fluid ticks. Mirrors block tick ordering but is
    /// kept separate because vanilla persists them under `fluid_ticks`.
    pub scheduled_fluid_ticks: Vec<ScheduledFluidTick>,
    next_scheduled_fluid_tick_sequence: u64,
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
    /// what M2 dropped on decode: structures, `PostProcessing`, `InhabitedTime`,
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
    /// Monotonic token bumped when runtime-owned state marks this chunk dirty.
    /// Dirty flush commit can compare this token instead of re-encoding under
    /// the world lock when no post-plan mutation happened.
    pub dirty_generation: u64,
    runtime_instance_id: u64,
    light_source_generation: u64,
    block_mutation_versions: HashMap<u32, u64>,
}

/// M7: production interface every chunk generator implements. Lives
/// in `mc-world` rather than `mc-worldgen` so `WorldStorage` can hold
/// an `Arc<dyn ChunkGenerator>` without taking a dep cycle. The
/// concrete implementation (`mc_worldgen::TerrainGenerator`) is
/// supplied by the binary at startup.
///
/// `Send + Sync` because the world is shared across the network
/// listener's connection tasks via `Arc<Mutex<WorldStorage>>`.
pub trait ChunkGenerator: Send + Sync {
    /// Build a brand-new `Chunk` for the given position. Generated
    /// chunks must come back with `dirty = true` so the M6 flush
    /// pipeline persists them before the cache evicts them — re-
    /// running the generator on every miss would be a perf
    /// regression on a world the player has already touched.
    fn generate(&self, pos: ChunkPos) -> Chunk;
}

impl Chunk {
    /// A column filled with `air` blocks and `biome` everywhere, no
    /// block entities, no heightmaps, status `"full"`.
    #[must_use]
    pub fn empty(pos: ChunkPos, air: BlockStateId, biome: Identifier) -> Self {
        Self::empty_with_geometry(pos, air, biome, OVERWORLD_GEOMETRY)
    }

    #[must_use]
    pub fn empty_with_geometry(
        pos: ChunkPos,
        air: BlockStateId,
        biome: Identifier,
        geometry: ChunkGeometry,
    ) -> Self {
        let section_count = geometry.section_count();
        Self {
            pos,
            geometry,
            sections: (0..section_count)
                .map(|_| ChunkSection::filled(air, air))
                .collect(),
            biomes: (0..section_count)
                .map(|_| BiomeSection::filled(biome.clone()))
                .collect(),
            heightmaps: HashMap::new(),
            highest_opaque: Heightmap::zeroed(),
            block_entities: HashMap::new(),
            furnaces: HashMap::new(),
            chests: HashMap::new(),
            hoppers: HashMap::new(),
            scheduled_block_ticks: Vec::new(),
            next_scheduled_block_tick_sequence: 0,
            scheduled_fluid_ticks: Vec::new(),
            next_scheduled_fluid_tick_sequence: 0,
            status: "full".to_string(),
            section_lights: vec![SectionLight::default(); section_count],
            extras: Vec::new(),
            dirty: false,
            dirty_generation: 0,
            runtime_instance_id: next_chunk_runtime_instance_id(),
            light_source_generation: 0,
            block_mutation_versions: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn geometry(&self) -> ChunkGeometry {
        self.geometry
    }

    #[must_use]
    pub fn block_mutation_token(&self, x: u8, y: i32, z: u8) -> Option<BlockMutationToken> {
        let key = block_mutation_key(self.geometry, x, y, z)?;
        Some(BlockMutationToken {
            chunk_instance_id: self.runtime_instance_id,
            version: self.block_mutation_versions.get(&key).copied().unwrap_or(0),
        })
    }

    #[must_use]
    pub fn light_source_token(&self) -> ChunkLightSourceToken {
        ChunkLightSourceToken {
            chunk_instance_id: self.runtime_instance_id,
            generation: self.light_source_generation,
        }
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.dirty_generation = self.dirty_generation.wrapping_add(1).max(1);
    }

    #[must_use]
    pub fn world_journal_lsn(&self) -> u64 {
        self.extras
            .iter()
            .find_map(|(key, value)| (key == WORLD_JOURNAL_LSN_KEY).then_some(value))
            .and_then(|value| match value {
                Tag::Long(value) if *value >= 0 => Some(*value as u64),
                _ => None,
            })
            .unwrap_or(0)
    }

    pub(crate) fn set_world_journal_lsn(&mut self, lsn: u64) {
        let value = i64::try_from(lsn).expect("world journal LSN must fit a nonnegative NBT long");
        if self
            .extras
            .iter()
            .any(|(key, tag)| key == WORLD_JOURNAL_LSN_KEY && *tag == Tag::Long(value))
        {
            return;
        }
        self.extras.retain(|(key, _)| key != WORLD_JOURNAL_LSN_KEY);
        self.extras
            .push((WORLD_JOURNAL_LSN_KEY.to_string(), Tag::Long(value)));
        self.mark_dirty();
    }

    pub fn set_baked_light(&mut self, light: &ChunkLight) {
        light.write_section_lights(&mut self.section_lights);
        self.mark_dirty();
    }

    fn clear_baked_light(&mut self) {
        for section in &mut self.section_lights {
            *section = SectionLight::default();
        }
    }

    /// Look up a block by chunk-local (x, z) and absolute world y.
    /// `None` when `y` is outside this chunk's geometry.
    #[must_use]
    pub fn get_block(&self, x: u8, y: i32, z: u8) -> Option<BlockStateId> {
        let (idx, sy) = self.geometry.world_y_to_section(y)?;
        Some(self.sections[idx].get(x, sy, z))
    }

    /// Set a block; returns the previous state, or `None` if `y` is
    /// outside the chunk.
    pub fn set_block(&mut self, x: u8, y: i32, z: u8, state: BlockStateId) -> Option<BlockStateId> {
        self.set_block_inner(x, y, z, state, true)
    }

    fn set_block_preserving_light_source(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
    ) -> Option<BlockStateId> {
        self.set_block_inner(x, y, z, state, false)
    }

    fn set_block_inner(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        changes_light_source: bool,
    ) -> Option<BlockStateId> {
        let (idx, sy) = self.geometry.world_y_to_section(y)?;
        let previous = self.sections[idx].set(x, sy, z, state);
        if previous != state && changes_light_source {
            self.light_source_generation = self.light_source_generation.wrapping_add(1).max(1);
        }
        Some(previous)
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
        self.set_block_and_update_inner(x, y, z, state, air, BlockLightMutation::Invalidate)
    }

    /// Mutate a block without discarding baked light. The caller must prove
    /// that old and new states have identical light behavior.
    pub fn set_block_and_update_preserving_light(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        air: BlockStateId,
    ) -> Option<BlockStateId> {
        self.set_block_and_update_inner(x, y, z, state, air, BlockLightMutation::PreserveInert)
    }

    /// Retain the old baked light as input for an immediate incremental relight.
    /// Unlike a light-inert update, this still advances the light-source token.
    pub fn set_block_and_update_retaining_baked_light(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        air: BlockStateId,
    ) -> Option<BlockStateId> {
        self.set_block_and_update_inner(x, y, z, state, air, BlockLightMutation::RetainForRelight)
    }

    fn set_block_and_update_inner(
        &mut self,
        x: u8,
        y: i32,
        z: u8,
        state: BlockStateId,
        air: BlockStateId,
        light_mutation: BlockLightMutation,
    ) -> Option<BlockStateId> {
        let prev = if light_mutation != BlockLightMutation::PreserveInert {
            self.set_block(x, y, z, state)?
        } else {
            self.set_block_preserving_light_source(x, y, z, state)?
        };
        if prev == state {
            return Some(prev);
        }
        let key = block_mutation_key(self.geometry, x, y, z).expect("validated block coordinates");
        let version = self.block_mutation_versions.entry(key).or_default();
        *version = version
            .checked_add(1)
            .expect("block mutation version exhausted");
        self.mark_dirty();
        if light_mutation == BlockLightMutation::Invalidate {
            self.clear_baked_light();
        }
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

    pub fn rebuild_highest_opaque(&mut self, table: &BlockLightTable) {
        for z in 0..SECTION_DIM as u8 {
            for x in 0..SECTION_DIM as u8 {
                self.update_highest_opaque_column(x, z, table);
            }
        }
    }

    pub fn update_highest_opaque_column(&mut self, x: u8, z: u8, table: &BlockLightTable) {
        let top = top_opaque_column(self, x, z, table);
        self.highest_opaque.set(x, z, top);
    }

    #[must_use]
    pub fn highest_opaque_y(&self, x: u8, z: u8) -> Option<i32> {
        heightmap_value_to_world_y_at(self.geometry.min_y(), self.highest_opaque.get(x, z))
    }

    #[must_use]
    pub fn scheduled_block_ticks(&self) -> &[ScheduledBlockTick] {
        &self.scheduled_block_ticks
    }

    pub fn schedule_block_tick(&mut self, mut tick: ScheduledBlockTick) -> bool {
        if !self.contains_block_pos(tick.pos)
            || self
                .scheduled_block_ticks
                .iter()
                .any(|existing| existing.same_request(&tick))
        {
            return false;
        }
        tick.sequence = self.next_scheduled_block_tick_sequence;
        self.next_scheduled_block_tick_sequence =
            self.next_scheduled_block_tick_sequence.wrapping_add(1);
        self.scheduled_block_ticks.push(tick);
        self.scheduled_block_ticks
            .sort_by_key(ScheduledBlockTick::sort_key);
        self.mark_dirty();
        true
    }

    pub(crate) fn load_scheduled_block_ticks(&mut self, mut ticks: Vec<ScheduledBlockTick>) {
        ticks.sort_by_key(ScheduledBlockTick::sort_key);
        self.next_scheduled_block_tick_sequence = ticks
            .iter()
            .map(ScheduledBlockTick::sequence)
            .max()
            .map_or(0, |sequence| sequence.wrapping_add(1));
        self.scheduled_block_ticks = ticks;
    }

    pub fn remove_scheduled_block_ticks_at(&mut self, pos: BlockPos) -> Vec<ScheduledBlockTick> {
        let before = self.scheduled_block_ticks.len();
        let mut removed = Vec::new();
        self.scheduled_block_ticks.retain(|tick| {
            if tick.pos == pos {
                removed.push(tick.clone());
                false
            } else {
                true
            }
        });
        if self.scheduled_block_ticks.len() != before {
            self.mark_dirty();
        }
        removed
    }

    pub fn drain_due_block_ticks(
        &mut self,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledBlockTick> {
        if max_ticks == 0 {
            return Vec::new();
        }
        let due_count = self
            .scheduled_block_ticks
            .partition_point(|tick| tick.trigger_tick <= world_tick)
            .min(max_ticks);
        if due_count == 0 {
            return Vec::new();
        }
        self.mark_dirty();
        self.scheduled_block_ticks.drain(0..due_count).collect()
    }

    pub(crate) fn drain_scheduled_block_tick_prefix(
        &mut self,
        expected: &[ScheduledBlockTick],
    ) -> bool {
        if expected.is_empty() || !self.scheduled_block_ticks.starts_with(expected) {
            return false;
        }
        self.scheduled_block_ticks.drain(0..expected.len());
        self.mark_dirty();
        true
    }

    #[must_use]
    pub fn scheduled_fluid_ticks(&self) -> &[ScheduledFluidTick] {
        &self.scheduled_fluid_ticks
    }

    pub fn schedule_fluid_tick(&mut self, mut tick: ScheduledFluidTick) -> bool {
        if !self.contains_block_pos(tick.pos)
            || self
                .scheduled_fluid_ticks
                .iter()
                .any(|existing| existing.same_request(&tick))
        {
            return false;
        }
        tick.sequence = self.next_scheduled_fluid_tick_sequence;
        self.next_scheduled_fluid_tick_sequence =
            self.next_scheduled_fluid_tick_sequence.wrapping_add(1);
        self.scheduled_fluid_ticks.push(tick);
        self.scheduled_fluid_ticks
            .sort_by_key(ScheduledFluidTick::sort_key);
        self.mark_dirty();
        true
    }

    pub(crate) fn load_scheduled_fluid_ticks(&mut self, mut ticks: Vec<ScheduledFluidTick>) {
        ticks.sort_by_key(ScheduledFluidTick::sort_key);
        self.next_scheduled_fluid_tick_sequence = ticks
            .iter()
            .map(ScheduledFluidTick::sequence)
            .max()
            .map_or(0, |sequence| sequence.wrapping_add(1));
        self.scheduled_fluid_ticks = ticks;
    }

    pub fn remove_scheduled_fluid_ticks_at(&mut self, pos: BlockPos) -> Vec<ScheduledFluidTick> {
        let before = self.scheduled_fluid_ticks.len();
        let mut removed = Vec::new();
        self.scheduled_fluid_ticks.retain(|tick| {
            if tick.pos == pos {
                removed.push(tick.clone());
                false
            } else {
                true
            }
        });
        if self.scheduled_fluid_ticks.len() != before {
            self.mark_dirty();
        }
        removed
    }

    pub fn drain_due_fluid_ticks(
        &mut self,
        world_tick: u64,
        max_ticks: usize,
    ) -> Vec<ScheduledFluidTick> {
        if max_ticks == 0 {
            return Vec::new();
        }
        let due_count = self
            .scheduled_fluid_ticks
            .partition_point(|tick| tick.trigger_tick <= world_tick)
            .min(max_ticks);
        if due_count == 0 {
            return Vec::new();
        }
        self.mark_dirty();
        self.scheduled_fluid_ticks.drain(0..due_count).collect()
    }

    pub(crate) fn drain_scheduled_fluid_tick_prefix(
        &mut self,
        expected: &[ScheduledFluidTick],
    ) -> bool {
        if expected.is_empty() || !self.scheduled_fluid_ticks.starts_with(expected) {
            return false;
        }
        self.scheduled_fluid_ticks.drain(0..expected.len());
        self.mark_dirty();
        true
    }

    #[must_use]
    pub fn contains_block_pos(&self, pos: BlockPos) -> bool {
        pos.y >= self.geometry.min_y()
            && pos.y < self.geometry.max_y()
            && pos.x.div_euclid(SECTION_DIM as i32) == self.pos.x
            && pos.z.div_euclid(SECTION_DIM as i32) == self.pos.z
    }
}

fn next_chunk_runtime_instance_id() -> u64 {
    NEXT_CHUNK_RUNTIME_INSTANCE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .expect("chunk runtime instance id exhausted")
}

fn block_mutation_key(geometry: ChunkGeometry, x: u8, y: i32, z: u8) -> Option<u32> {
    if usize::from(x) >= SECTION_DIM
        || usize::from(z) >= SECTION_DIM
        || !(geometry.min_y()..geometry.max_y()).contains(&y)
    {
        return None;
    }
    let y = u32::try_from(y - geometry.min_y()).ok()?;
    Some((y * SECTION_DIM as u32 + u32::from(z)) * SECTION_DIM as u32 + u32::from(x))
}

/// Walk column `(x, z)` from `MAX_Y - 1` down to `MIN_Y` and return
/// the heightmap value vanilla stores: `top_y + 1 - MIN_Y` (the Y
/// of the first air cell above the topmost non-air block, expressed
/// as an offset from the bottom of the world). For an entirely-air
/// column the value is `0`, matching vanilla's "empty column"
/// convention.
fn top_non_air_column(chunk: &Chunk, x: u8, z: u8, air: BlockStateId) -> u32 {
    let geometry = chunk.geometry();
    for y in (geometry.min_y()..geometry.max_y()).rev() {
        if chunk.get_block(x, y, z) != Some(air) {
            // vanilla packs heightmap values as `height - MIN_Y + 1`
            // so a top block at world Y=64 stores as `64 - (-64) + 1`
            // = 129. The +1 makes "the lowest cell above the top
            // block" the value, matching the meaning vanilla uses
            // for chunk-spawning and skylight shortcuts.
            return (y - geometry.min_y() + 1) as u32;
        }
    }
    0
}

pub fn top_opaque_column(chunk: &Chunk, x: u8, z: u8, table: &BlockLightTable) -> u32 {
    let geometry = chunk.geometry();
    for y in (geometry.min_y()..geometry.max_y()).rev() {
        let Some(state) = chunk.get_block(x, y, z) else {
            continue;
        };
        if !table.propagates_sky(state.0).unwrap_or(true) {
            return (y - geometry.min_y() + 1) as u32;
        }
    }
    0
}

#[must_use]
pub fn heightmap_value_to_world_y(value: u32) -> Option<i32> {
    heightmap_value_to_world_y_at(MIN_Y, value)
}

fn heightmap_value_to_world_y_at(min_y: i32, value: u32) -> Option<i32> {
    if value == 0 {
        None
    } else {
        Some(min_y + value as i32 - 1)
    }
}

#[cfg(test)]
fn world_y_to_section(y: i32) -> Option<(usize, u8)> {
    OVERWORLD_GEOMETRY.world_y_to_section(y)
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
    fn wheat() -> Identifier {
        Identifier::parse("minecraft:wheat").unwrap()
    }
    fn water() -> Identifier {
        Identifier::parse("minecraft:water").unwrap()
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
    fn custom_geometry_controls_chunk_sections_and_block_bounds() {
        assert!(ChunkGeometry::new(0, 512).is_none());
        let geometry = ChunkGeometry::new(0, 256).unwrap();
        let mut chunk =
            Chunk::empty_with_geometry(ChunkPos { x: 2, z: -3 }, air(), plains(), geometry);

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), 16);
        assert_eq!(chunk.biomes.len(), 16);
        assert_eq!(chunk.get_block(0, 0, 0), Some(air()));
        assert_eq!(chunk.set_block(15, 255, 15, stone()), Some(air()));
        assert_eq!(chunk.get_block(15, 255, 15), Some(stone()));
        assert_eq!(chunk.get_block(0, -1, 0), None);
        assert_eq!(chunk.get_block(0, 256, 0), None);

        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());
        assert_eq!(
            chunk.set_block_and_update(3, 200, 7, stone(), air()),
            Some(air())
        );
        assert_eq!(chunk.heightmaps["WORLD_SURFACE"].get(3, 7), 201);
        chunk.highest_opaque.set(3, 7, 201);
        assert_eq!(chunk.highest_opaque_y(3, 7), Some(200));

        assert!(chunk.contains_block_pos(BlockPos {
            x: 32,
            y: 0,
            z: -48,
        }));
        assert!(!chunk.contains_block_pos(BlockPos {
            x: 32,
            y: -1,
            z: -48,
        }));
        assert!(chunk.block_mutation_token(15, 255, 15).is_some());
        assert!(chunk.block_mutation_token(15, 256, 15).is_none());
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
    fn light_source_token_tracks_blocks_but_not_baked_light() {
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let initial = chunk.light_source_token();

        chunk.set_baked_light(&crate::light::ChunkLight::filled(15, 0));
        assert_eq!(chunk.light_source_token(), initial);
        assert_eq!(chunk.set_block(3, 64, 7, air()), Some(air()));
        assert_eq!(chunk.light_source_token(), initial);

        assert_eq!(chunk.set_block(3, 64, 7, stone()), Some(air()));
        let edited = chunk.light_source_token();
        assert_ne!(edited, initial);
        assert_eq!(chunk.clone().light_source_token(), edited);
        assert_ne!(
            Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains()).light_source_token(),
            edited
        );
    }

    #[test]
    fn runtime_block_mutation_tokens_are_sparse_clone_stable_and_chunk_scoped() {
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let initial = chunk.block_mutation_token(3, 64, 7).expect("initial token");
        assert_eq!(initial.version, 0);

        chunk
            .set_block(3, 64, 7, stone())
            .expect("worldgen-style set");
        assert_eq!(
            chunk.block_mutation_token(3, 64, 7),
            Some(initial),
            "raw construction writes must not populate runtime mutation history"
        );
        chunk
            .set_block_and_update(3, 64, 7, air(), air())
            .expect("runtime edit");
        let edited = chunk.block_mutation_token(3, 64, 7).expect("edited token");
        assert_eq!(edited.version, 1);
        assert_eq!(chunk.clone().block_mutation_token(3, 64, 7), Some(edited));

        let reloaded = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains())
            .block_mutation_token(3, 64, 7)
            .expect("reloaded token");
        assert_ne!(reloaded.chunk_instance_id, edited.chunk_instance_id);
        assert_eq!(reloaded.version, 0);
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

    #[test]
    fn set_block_and_update_clears_stale_baked_light_layers() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        c.section_lights[0].sky = Some(vec![0xFF; LIGHT_LAYER_BYTES]);
        c.section_lights[0].block = Some(vec![0x11; LIGHT_LAYER_BYTES]);
        c.section_lights[3].sky = Some(vec![0x22; LIGHT_LAYER_BYTES]);

        let prev = c.set_block_and_update(0, 0, 0, stone(), air()).unwrap();

        assert_eq!(prev, air());
        assert!(
            c.section_lights
                .iter()
                .all(|section| section.sky.is_none() && section.block.is_none()),
            "block mutation must invalidate baked light arrays before they can be reused"
        );
    }

    #[test]
    fn proven_light_inert_block_update_preserves_baked_light_layers() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        c.section_lights[0].sky = Some(vec![0xFF; LIGHT_LAYER_BYTES]);
        c.section_lights[0].block = Some(vec![0x11; LIGHT_LAYER_BYTES]);
        c.section_lights[3].sky = Some(vec![0x22; LIGHT_LAYER_BYTES]);
        let expected = c.section_lights.clone();
        let light_source = c.light_source_token();

        let prev = c
            .set_block_and_update_preserving_light(0, 0, 0, stone(), air())
            .unwrap();

        assert_eq!(prev, air());
        assert_eq!(c.section_lights, expected);
        assert_eq!(c.light_source_token(), light_source);
    }

    #[test]
    fn incremental_relight_update_retains_baked_light_and_changes_source_token() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        c.section_lights[0].sky = Some(vec![0xFF; LIGHT_LAYER_BYTES]);
        c.section_lights[0].block = Some(vec![0x11; LIGHT_LAYER_BYTES]);
        let expected = c.section_lights.clone();
        let light_source = c.light_source_token();

        let prev = c
            .set_block_and_update_retaining_baked_light(0, 0, 0, stone(), air())
            .unwrap();

        assert_eq!(prev, air());
        assert_eq!(c.section_lights, expected);
        assert_ne!(c.light_source_token(), light_source);
    }

    #[test]
    fn highest_opaque_uses_light_table_sky_predicate() {
        let table = BlockLightTable::from_arrays(
            "test",
            vec![0, 0, 0],
            vec![0, 15, 0],
            vec![true, false, true],
        );
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let glass = BlockStateId(2);

        c.set_block(3, 80, 7, glass);
        c.set_block(3, 64, 7, stone());
        c.rebuild_highest_opaque(&table);
        assert_eq!(c.highest_opaque_y(3, 7), Some(64));

        c.set_block(3, 64, 7, air());
        c.update_highest_opaque_column(3, 7, &table);
        assert_eq!(c.highest_opaque_y(3, 7), None);
    }

    #[test]
    fn scheduled_block_ticks_drain_in_deterministic_order() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let pos_a = BlockPos { x: 1, y: 64, z: 1 };
        let pos_b = BlockPos { x: 2, y: 64, z: 1 };
        let pos_c = BlockPos { x: 3, y: 64, z: 1 };

        assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_a, wheat(), 10, 0)));
        assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_b, wheat(), 5, 0)));
        assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos_c, wheat(), 10, -1)));

        c.dirty = false;
        let due = c.drain_due_block_ticks(10, usize::MAX);

        assert_eq!(
            due.iter().map(|tick| tick.pos).collect::<Vec<_>>(),
            vec![pos_b, pos_c, pos_a]
        );
        assert!(c.scheduled_block_ticks().is_empty());
        assert!(c.dirty);
    }

    #[test]
    fn scheduled_block_ticks_skip_duplicates_and_mark_dirty_only_on_change() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let tick = ScheduledBlockTick::new(BlockPos { x: 1, y: 64, z: 1 }, wheat(), 10, 0);

        assert!(c.schedule_block_tick(tick.clone()));
        c.dirty = false;
        assert!(!c.schedule_block_tick(tick));
        assert_eq!(c.scheduled_block_ticks().len(), 1);
        assert!(!c.dirty);

        assert!(c.drain_due_block_ticks(9, usize::MAX).is_empty());
        assert!(!c.dirty);
        assert!(c.drain_due_block_ticks(10, 0).is_empty());
        assert!(!c.dirty);
    }

    #[test]
    fn scheduled_block_ticks_reject_positions_outside_chunk() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());

        assert!(!c.schedule_block_tick(ScheduledBlockTick::new(
            BlockPos { x: 16, y: 64, z: 1 },
            wheat(),
            10,
            0,
        )));
        assert!(!c.schedule_block_tick(ScheduledBlockTick::new(
            BlockPos {
                x: 1,
                y: MAX_Y,
                z: 1
            },
            wheat(),
            10,
            0,
        )));
        assert!(c.scheduled_block_ticks().is_empty());
        assert!(!c.dirty);
    }

    #[test]
    fn scheduled_block_ticks_remove_by_position() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let pos = BlockPos { x: 1, y: 64, z: 1 };
        let other = BlockPos { x: 2, y: 64, z: 1 };
        assert!(c.schedule_block_tick(ScheduledBlockTick::new(pos, wheat(), 10, 0)));
        assert!(c.schedule_block_tick(ScheduledBlockTick::new(other, wheat(), 10, 0)));

        c.dirty = false;
        let removed = c.remove_scheduled_block_ticks_at(pos);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].pos, pos);
        assert_eq!(c.scheduled_block_ticks().len(), 1);
        assert_eq!(c.scheduled_block_ticks()[0].pos, other);
        assert!(c.dirty);

        c.dirty = false;
        assert!(c.remove_scheduled_block_ticks_at(pos).is_empty());
        assert!(!c.dirty);
    }

    #[test]
    fn scheduled_fluid_ticks_drain_in_deterministic_order() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let pos_a = BlockPos { x: 1, y: 64, z: 1 };
        let pos_b = BlockPos { x: 2, y: 64, z: 1 };
        let pos_c = BlockPos { x: 3, y: 64, z: 1 };

        assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_a, water(), 10, 0)));
        assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_b, water(), 5, 0)));
        assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos_c, water(), 10, -1)));

        c.dirty = false;
        let due = c.drain_due_fluid_ticks(10, usize::MAX);

        assert_eq!(
            due.iter().map(|tick| tick.pos).collect::<Vec<_>>(),
            vec![pos_b, pos_c, pos_a]
        );
        assert!(c.scheduled_fluid_ticks().is_empty());
        assert!(c.dirty);
    }

    #[test]
    fn scheduled_fluid_ticks_skip_duplicates_and_mark_dirty_only_on_change() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let tick = ScheduledFluidTick::new(BlockPos { x: 1, y: 64, z: 1 }, water(), 10, 0);

        assert!(c.schedule_fluid_tick(tick.clone()));
        c.dirty = false;
        assert!(!c.schedule_fluid_tick(tick));
        assert_eq!(c.scheduled_fluid_ticks().len(), 1);
        assert!(!c.dirty);

        assert!(c.drain_due_fluid_ticks(9, usize::MAX).is_empty());
        assert!(!c.dirty);
        assert!(c.drain_due_fluid_ticks(10, 0).is_empty());
        assert!(!c.dirty);
    }

    #[test]
    fn scheduled_fluid_ticks_reject_positions_outside_chunk() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());

        assert!(!c.schedule_fluid_tick(ScheduledFluidTick::new(
            BlockPos { x: 16, y: 64, z: 1 },
            water(),
            10,
            0,
        )));
        assert!(!c.schedule_fluid_tick(ScheduledFluidTick::new(
            BlockPos {
                x: 1,
                y: MAX_Y,
                z: 1,
            },
            water(),
            10,
            0,
        )));
        assert!(c.scheduled_fluid_ticks().is_empty());
        assert!(!c.dirty);
    }

    #[test]
    fn scheduled_fluid_ticks_remove_by_position() {
        let mut c = Chunk::empty(ChunkPos { x: 0, z: 0 }, air(), plains());
        let pos = BlockPos { x: 1, y: 64, z: 1 };
        let other = BlockPos { x: 2, y: 64, z: 1 };
        assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(pos, water(), 10, 0)));
        assert!(c.schedule_fluid_tick(ScheduledFluidTick::new(other, water(), 10, 0)));

        c.dirty = false;
        let removed = c.remove_scheduled_fluid_ticks_at(pos);

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].pos, pos);
        assert_eq!(c.scheduled_fluid_ticks().len(), 1);
        assert_eq!(c.scheduled_fluid_ticks()[0].pos, other);
        assert!(c.dirty);

        c.dirty = false;
        assert!(c.remove_scheduled_fluid_ticks_at(pos).is_empty());
        assert!(!c.dirty);
    }
}
