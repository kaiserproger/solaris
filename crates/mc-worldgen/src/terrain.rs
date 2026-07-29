//! Overworld chunk assembly.
//!
//! The density router owns continents, erosion, mountain ridges, rivers,
//! climate, and caves. This module resolves block palettes and assembles
//! chunks from that deterministic field.
//! Vertical base layers:
//!
//! - `y = geometry.min_y()` → bedrock
//! - `geometry.min_y() < y < height - 3` → stone
//! - `height - 3 ≤ y < height` → dirt
//! - `y = height` → grass_block
//! - `y > height` → air
//!
//! Output is deterministic in `(seed, world_x, world_z)` and independent of
//! chunk generation order.

use std::sync::Arc;

use mc_data::Identifier;
use mc_world::chunk::{Chunk, ChunkGeometry, ChunkPos, Heightmap, OVERWORLD_GEOMETRY};
use mc_world::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockRegistry, BlockStateId, ChunkGenerator,
    PackedBitArray, SettlementInhabitantMarker, SettlementVacantHomeMarker,
};

use crate::noise::fbm_2d;
use crate::structures::{StructureRules, StructureTemplate};

mod biome_rules;
mod geological_ores;
mod ore_rules;
mod overworld;

pub use biome_rules::BiomeRules;
use geological_ores::GeologicalOreRules;
pub use ore_rules::{
    BiomeScope, MAX_ORE_RULES, MAX_ORE_WORK_UNITS_PER_CHUNK, OreRule, OreRules, OreRulesError,
    OreSpacing, YRange,
};
use ore_rules::{
    MAX_ORE_ANCHORS_PER_CELL, MAX_ORE_VEIN_SIZE, ORE_ANCHOR_CELL_EDGE, ORE_ANCHOR_CELL_VOLUME,
};
use overworld::{OverworldRouter, TerrainSample};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerrainGeneratorError {
    #[error("block registry missing required terrain block {name}")]
    MissingRequiredBlock { name: &'static str },
}

pub const SEA_LEVEL: i32 = 63;
const RIVER_BIOME_WIDTH: f64 = 0.025;
const BEACH_HEIGHT_ABOVE_SEA: i32 = 2;
/// Number of dirt cells between grass cap and stone.
const DIRT_DEPTH: i32 = 3;
const ORE_VEIN_RADIUS: i32 = 4;
const ORE_COLUMN_HALO: i32 = ORE_VEIN_RADIUS * 2 + 1;
const ORE_GROWTH_ATTEMPTS: usize = 12;
const ORE_DIRECTIONS: [[i8; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];
const CAVE_SURFACE_CLEARANCE: i32 = 32;
const CAVE_VERTICAL_SAMPLE_STEP: i32 = 2;
const DEEPSLATE_TOP_Y: i32 = 0;
const DEEPSLATE_SOLID_Y: i32 = -8;
const VEGETATION_REGION_SCALE: f64 = 192.0;
const SPAWN_SEARCH_STEP_BLOCKS: i32 = 64;
const SPAWN_SEARCH_MAX_RADIUS_BLOCKS: i32 = 8_192;
const SPAWN_SITE_SAMPLE_RADIUS_BLOCKS: i32 = 8;
const SPAWN_SITE_MAX_RELIEF: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnLocation {
    pub block_x: i32,
    pub surface_y: i32,
    pub block_z: i32,
}

impl SpawnLocation {
    #[must_use]
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.block_x.div_euclid(16),
            z: self.block_z.div_euclid(16),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TellusWorldgenSettings {
    pub world_scale_meters_per_block: f64,
    pub terrestrial_height_scale: f64,
    pub oceanic_height_scale: f64,
    pub sea_level: i32,
    pub climate_strength: f64,
    pub water_enabled: bool,
}

impl Default for TellusWorldgenSettings {
    fn default() -> Self {
        Self {
            world_scale_meters_per_block: 30.0,
            terrestrial_height_scale: 1.0,
            oceanic_height_scale: 1.0,
            sea_level: SEA_LEVEL,
            climate_strength: 1.0,
            water_enabled: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WorldgenMode {
    #[default]
    VanillaLike,
    TellusLike(TellusWorldgenSettings),
}

impl WorldgenMode {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::VanillaLike => "vanilla_like",
            Self::TellusLike(_) => "tellus_like",
        }
    }
}

/// Hill-noise terrain. Holds the resolved state ids of the four
/// block types it emits so `generate` is allocation-free past the
/// `Chunk::empty` it returns.
pub struct TerrainGenerator {
    seed: i64,
    geometry: ChunkGeometry,
    air: BlockStateId,
    bedrock: BlockStateId,
    stone: BlockStateId,
    dirt: BlockStateId,
    grass_block: BlockStateId,
    sand: BlockStateId,
    red_sand: BlockStateId,
    gravel: BlockStateId,
    podzol: BlockStateId,
    snow_block: BlockStateId,
    deepslate: BlockStateId,
    water: BlockStateId,
    biomes: BiomeRules,
    ores: OreRules,
    geological_ores: Option<GeologicalOreRules>,
    structures: StructureRules,
    decorations: DecorationBlocks,
    worldgen_mode: WorldgenMode,
}

#[derive(Clone)]
struct ColumnPlan {
    lx: u8,
    lz: u8,
    wx: i32,
    wz: i32,
    height: i32,
    top_non_air: i32,
    dirt_start: i32,
    biome: Identifier,
    surface: BlockStateId,
    fill: BlockStateId,
    vegetation_density: f64,
    hash: u64,
}

struct OreColumnCache {
    min_x: i64,
    min_z: i64,
    side: usize,
    columns: Vec<Option<ColumnPlan>>,
    cave_min_y: i32,
    cave_layers: usize,
    cave_samples: Vec<u8>,
}

impl OreColumnCache {
    fn column_index(&self, world_x: i32, world_z: i32) -> Option<usize> {
        let local_x = i64::from(world_x).checked_sub(self.min_x)?;
        let local_z = i64::from(world_z).checked_sub(self.min_z)?;
        let local_x = usize::try_from(local_x).ok()?;
        let local_z = usize::try_from(local_z).ok()?;
        if local_x >= self.side || local_z >= self.side {
            return None;
        }
        Some(local_z * self.side + local_x)
    }

    fn get_or_plan<'a>(
        &'a mut self,
        generator: &TerrainGenerator,
        world_x: i32,
        world_z: i32,
    ) -> Option<&'a ColumnPlan> {
        let index = self.column_index(world_x, world_z)?;
        if self.columns[index].is_none() {
            self.columns[index] = Some(generator.plan_column(
                ChunkPos {
                    x: world_x.div_euclid(16),
                    z: world_z.div_euclid(16),
                },
                world_x.rem_euclid(16) as u8,
                world_z.rem_euclid(16) as u8,
            ));
        }
        self.columns[index].as_ref()
    }

    fn get_or_cave(
        &mut self,
        generator: &TerrainGenerator,
        world_x: i32,
        sample_y: i32,
        world_z: i32,
        surface_y: i32,
    ) -> bool {
        let Some(column_index) = self.column_index(world_x, world_z) else {
            return generator.is_cave_cell(world_x, sample_y, world_z, surface_y);
        };
        let delta_y = sample_y.saturating_sub(self.cave_min_y);
        if delta_y < 0 || delta_y % CAVE_VERTICAL_SAMPLE_STEP != 0 {
            return generator.is_cave_cell(world_x, sample_y, world_z, surface_y);
        }
        let layer = usize::try_from(delta_y / CAVE_VERTICAL_SAMPLE_STEP).unwrap_or(usize::MAX);
        if layer >= self.cave_layers {
            return generator.is_cave_cell(world_x, sample_y, world_z, surface_y);
        }
        let index = layer * self.side * self.side + column_index;
        match self.cave_samples[index] {
            1 => false,
            2 => true,
            _ => {
                let cave = generator.is_cave_cell(world_x, sample_y, world_z, surface_y);
                self.cave_samples[index] = if cave { 2 } else { 1 };
                cave
            }
        }
    }
}

fn cached_raw_cave(
    cache: &mut [u8],
    side: usize,
    router: OverworldRouter,
    chunk_min: (i64, i64),
    sample_y: i32,
    raw: (usize, usize),
) -> bool {
    let (raw_x, raw_z) = raw;
    let index = raw_z * side + raw_x;
    match cache[index] {
        1 => false,
        2 => true,
        _ => {
            let world_x = chunk_min.0 + raw_x as i64 - 1;
            let world_z = chunk_min.1 + raw_z as i64 - 1;
            let cave = i32::try_from(world_x)
                .ok()
                .zip(i32::try_from(world_z).ok())
                .is_some_and(|(world_x, world_z)| router.raw_cave(world_x, sample_y, world_z));
            cache[index] = if cave { 2 } else { 1 };
            cave
        }
    }
}

#[derive(Clone)]
struct DecorationBlocks {
    oak_log: Option<BlockStateId>,
    oak_leaves: Option<BlockStateId>,
    forest_log: Option<BlockStateId>,
    forest_leaves: Option<BlockStateId>,
    cold_log: Option<BlockStateId>,
    cold_leaves: Option<BlockStateId>,
    jungle_log: Option<BlockStateId>,
    jungle_leaves: Option<BlockStateId>,
    acacia_log: Option<BlockStateId>,
    acacia_leaves: Option<BlockStateId>,
    short_grass: Option<BlockStateId>,
    dandelion: Option<BlockStateId>,
    poppy: Option<BlockStateId>,
    pumpkin: Option<BlockStateId>,
    sugar_cane: Option<BlockStateId>,
    cactus: Option<BlockStateId>,
    seagrass: Option<BlockStateId>,
    kelp_plant: Option<BlockStateId>,
    kelp: Option<BlockStateId>,
}

impl DecorationBlocks {
    fn new(registry: &BlockRegistry) -> Self {
        Self {
            oak_log: optional_block(registry, "minecraft:oak_log"),
            oak_leaves: optional_generated_leaves(registry, "minecraft:oak_leaves"),
            forest_log: optional_block(registry, "minecraft:birch_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            forest_leaves: optional_generated_leaves(registry, "minecraft:birch_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            cold_log: optional_block(registry, "minecraft:spruce_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            cold_leaves: optional_generated_leaves(registry, "minecraft:spruce_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            jungle_log: optional_block(registry, "minecraft:jungle_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            jungle_leaves: optional_generated_leaves(registry, "minecraft:jungle_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            acacia_log: optional_block(registry, "minecraft:acacia_log")
                .or_else(|| optional_block(registry, "minecraft:oak_log")),
            acacia_leaves: optional_generated_leaves(registry, "minecraft:acacia_leaves")
                .or_else(|| optional_generated_leaves(registry, "minecraft:oak_leaves")),
            short_grass: optional_block(registry, "minecraft:short_grass"),
            dandelion: optional_block(registry, "minecraft:dandelion"),
            poppy: optional_block(registry, "minecraft:poppy"),
            pumpkin: optional_block(registry, "minecraft:pumpkin"),
            sugar_cane: optional_block(registry, "minecraft:sugar_cane"),
            cactus: optional_block(registry, "minecraft:cactus"),
            seagrass: optional_block(registry, "minecraft:seagrass"),
            kelp_plant: optional_block(registry, "minecraft:kelp_plant"),
            kelp: optional_block(registry, "minecraft:kelp"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Oak,
    Birch,
    Spruce,
    Jungle,
    Acacia,
}

#[derive(Debug, Clone, Copy)]
struct TreeBlocks {
    kind: TreeKind,
    log: BlockStateId,
    leaves: BlockStateId,
}

#[derive(Debug, Clone, Copy)]
struct TreeLeafOffset {
    relative_y: i32,
    dx: i8,
    dz: i8,
    radius: i8,
}

fn tree_canopy_radius(kind: TreeKind, relative_y: i32) -> Option<i8> {
    match (kind, relative_y) {
        (TreeKind::Oak, -2 | -1) => Some(2),
        (TreeKind::Oak, 0 | 1) => Some(1),
        (TreeKind::Birch, -2 | -1) => Some(2),
        (TreeKind::Birch, 0) => Some(1),
        (TreeKind::Birch, 1) => Some(0),
        (TreeKind::Spruce, -4) => Some(1),
        (TreeKind::Spruce, -3 | -2) => Some(2),
        (TreeKind::Spruce, -1 | 0) => Some(1),
        (TreeKind::Spruce, 1) => Some(0),
        (TreeKind::Jungle, -2..=0) => Some(2),
        (TreeKind::Jungle, 1) => Some(1),
        (TreeKind::Acacia, -1 | 0) => Some(2),
        (TreeKind::Acacia, 1) => Some(1),
        _ => None,
    }
}

fn try_resolve_block(
    registry: &BlockRegistry,
    name: &'static str,
) -> Result<BlockStateId, TerrainGeneratorError> {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|b| b.default)
        .ok_or(TerrainGeneratorError::MissingRequiredBlock { name })
}

fn resolve_block_or(registry: &BlockRegistry, name: &str, fallback: BlockStateId) -> BlockStateId {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default).unwrap_or(fallback)
}

fn optional_block(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default)
}

fn optional_generated_leaves(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    generated_leaf_state(registry, &id)
}

fn generated_leaf_state(registry: &BlockRegistry, id: &Identifier) -> Option<BlockStateId> {
    registry
        .by_name_and_props(
            id,
            &[
                ("distance".to_string(), "1".to_string()),
                ("persistent".to_string(), "false".to_string()),
                ("waterlogged".to_string(), "false".to_string()),
            ],
        )
        .or_else(|| registry.block(id).map(|block| block.default))
}

fn checked_y_offset(y: i32, offset: i32) -> Option<i32> {
    i32::try_from(i64::from(y) + i64::from(offset)).ok()
}

fn heightmap_value_for_top(geometry: ChunkGeometry, top: i32) -> Option<u32> {
    let value = i64::from(top) + 1 - i64::from(geometry.min_y());
    let value = u32::try_from(value).ok()?;
    (value <= geometry.height() as u32).then_some(value)
}

fn world_block_coordinate(chunk: i32, local: u8) -> i32 {
    let coordinate = i64::from(chunk) * 16 + i64::from(local);
    i32::try_from(coordinate).expect("chunk lies outside the supported i32 block-coordinate range")
}

impl TerrainGenerator {
    /// Build a generator from a seed plus a block registry.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_biome_rules`] for fallible startup validation.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::try_with_biome_rules(seed, registry, BiomeRules::vanilla_overworld())
            .unwrap_or_else(|err| panic!("{err}"))
    }

    pub fn try_with_biome_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
    ) -> Result<Self, TerrainGeneratorError> {
        let stone = try_resolve_block(registry.as_ref(), "minecraft:stone")?;
        let ores = OreRules::solaris_default(registry.as_ref(), &biomes, stone);
        Self::try_with_rules(seed, registry, biomes, ores)
    }

    /// Build a generator and set its worldgen mode.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_biome_rules`] for fallible startup validation.
    #[must_use]
    pub fn with_worldgen_mode(seed: i64, registry: Arc<BlockRegistry>, mode: WorldgenMode) -> Self {
        let mut generator = Self::new(seed, registry);
        generator.worldgen_mode = mode;
        generator
    }

    #[must_use]
    pub fn with_mode(mut self, mode: WorldgenMode) -> Self {
        self.worldgen_mode = mode;
        self
    }

    /// Build a generator with explicit biome and ore rules.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_rules`] for fallible startup validation.
    #[must_use]
    pub fn with_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
        ores: OreRules,
    ) -> Self {
        Self::try_with_rules(seed, registry, biomes, ores).unwrap_or_else(|err| panic!("{err}"))
    }

    /// Build a generator with explicit biome and ore rules, returning startup
    /// validation errors instead of panicking.
    ///
    /// Required terrain blocks fail construction when absent. Optional surface,
    /// fluid, and decoration blocks keep Solaris' existing fallback behavior so
    /// reduced test registries can still exercise generation.
    ///
    /// # Errors
    ///
    /// Returns [`TerrainGeneratorError::MissingRequiredBlock`] when the block
    /// registry is missing `minecraft:air`, `minecraft:bedrock`,
    /// `minecraft:stone`, `minecraft:dirt`, `minecraft:grass_block`, or
    /// `minecraft:iron_ore`.
    pub fn try_with_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
        ores: OreRules,
    ) -> Result<Self, TerrainGeneratorError> {
        let air = try_resolve_block(registry.as_ref(), "minecraft:air")?;
        let stone = try_resolve_block(registry.as_ref(), "minecraft:stone")?;
        Ok(Self {
            seed,
            geometry: OVERWORLD_GEOMETRY,
            air,
            bedrock: try_resolve_block(registry.as_ref(), "minecraft:bedrock")?,
            stone,
            dirt: try_resolve_block(registry.as_ref(), "minecraft:dirt")?,
            grass_block: try_resolve_block(registry.as_ref(), "minecraft:grass_block")?,
            sand: resolve_block_or(registry.as_ref(), "minecraft:sand", stone),
            red_sand: resolve_block_or(registry.as_ref(), "minecraft:red_sand", stone),
            gravel: resolve_block_or(registry.as_ref(), "minecraft:gravel", stone),
            podzol: resolve_block_or(registry.as_ref(), "minecraft:podzol", stone),
            snow_block: resolve_block_or(registry.as_ref(), "minecraft:snow_block", stone),
            deepslate: resolve_block_or(registry.as_ref(), "minecraft:deepslate", stone),
            water: resolve_block_or(registry.as_ref(), "minecraft:water", air),
            biomes,
            ores,
            geological_ores: None,
            structures: StructureRules::none(),
            decorations: DecorationBlocks::new(registry.as_ref()),
            worldgen_mode: WorldgenMode::VanillaLike,
        })
    }

    #[must_use]
    pub fn with_structures(mut self, structures: StructureRules) -> Self {
        self.structures = structures;
        self
    }

    #[must_use]
    pub fn with_geological_deposits(mut self, registry: &BlockRegistry) -> Self {
        self.ores = OreRules::new(Vec::new()).expect("empty ore rules are valid");
        self.geological_ores = Some(GeologicalOreRules::new(registry, self.stone));
        self
    }

    #[must_use]
    pub const fn ore_generation_profile(&self) -> &'static str {
        if self.geological_ores.is_some() {
            "geological_deposits"
        } else {
            "vanilla"
        }
    }

    #[must_use]
    pub fn with_geometry(mut self, geometry: ChunkGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// Sample the terrain height for an absolute world `(x, z)`.
    /// Public so tests + spawn-position picking can use the same
    /// function the generator does.
    #[must_use]
    pub fn surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        self.density_router().sample(world_x, world_z).surface_y
    }

    /// Find a deterministic natural spawn centre without modifying terrain.
    ///
    /// The search walks expanding 64-block rings around the origin and accepts
    /// only dry, low-relief inland terrain away from river centres and strong
    /// mountain ridges. The network layer performs the final block-level support
    /// and body-space check inside the prepared window.
    #[must_use]
    pub fn locate_safe_spawn(&self) -> Option<SpawnLocation> {
        let max_ring = SPAWN_SEARCH_MAX_RADIUS_BLOCKS / SPAWN_SEARCH_STEP_BLOCKS;
        for ring in 0..=max_ring {
            if ring == 0 {
                if let Some(spawn) = self.spawn_candidate(0, 0) {
                    return Some(spawn);
                }
                continue;
            }
            let radius = ring * SPAWN_SEARCH_STEP_BLOCKS;
            for offset in -ring..=ring {
                let coordinate = offset * SPAWN_SEARCH_STEP_BLOCKS;
                for (x, z) in [(coordinate, -radius), (coordinate, radius)] {
                    if let Some(spawn) = self.spawn_candidate(x, z) {
                        return Some(spawn);
                    }
                }
            }
            for offset in (-ring + 1)..ring {
                let coordinate = offset * SPAWN_SEARCH_STEP_BLOCKS;
                for (x, z) in [(-radius, coordinate), (radius, coordinate)] {
                    if let Some(spawn) = self.spawn_candidate(x, z) {
                        return Some(spawn);
                    }
                }
            }
        }
        None
    }

    fn spawn_candidate(&self, block_x: i32, block_z: i32) -> Option<SpawnLocation> {
        let router = self.density_router();
        let centre = router.sample(block_x, block_z);
        let sea_level = match self.worldgen_mode {
            WorldgenMode::VanillaLike => SEA_LEVEL,
            WorldgenMode::TellusLike(settings) => settings.sea_level,
        };
        if centre.surface_y < sea_level + 4
            || centre.continentalness < 0.02
            || centre.river < 0.08
            || centre.ridges > 0.24
        {
            return None;
        }

        let mut minimum = centre.surface_y;
        let mut maximum = centre.surface_y;
        for dz in [
            -SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
            0,
            SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
        ] {
            for dx in [
                -SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
                0,
                SPAWN_SITE_SAMPLE_RADIUS_BLOCKS,
            ] {
                let sample = router.sample(block_x.checked_add(dx)?, block_z.checked_add(dz)?);
                if sample.surface_y < sea_level + 3 || sample.river < 0.04 {
                    return None;
                }
                minimum = minimum.min(sample.surface_y);
                maximum = maximum.max(sample.surface_y);
            }
        }
        if maximum - minimum > SPAWN_SITE_MAX_RELIEF {
            return None;
        }
        Some(SpawnLocation {
            block_x,
            surface_y: centre.surface_y,
            block_z,
        })
    }

    fn density_router(&self) -> OverworldRouter {
        OverworldRouter::new(self.seed, self.geometry, self.worldgen_mode)
    }

    fn biome_for(&self, world_x: i32, world_z: i32, height: i32) -> Identifier {
        let sample = self.density_router().sample(world_x, world_z);
        match self.worldgen_mode {
            WorldgenMode::VanillaLike => self.vanilla_biome_for(world_x, world_z, height, sample),
            WorldgenMode::TellusLike(settings) => {
                self.tellus_biome_for(world_x, world_z, height, settings, sample)
            }
        }
    }

    fn vanilla_biome_for(
        &self,
        world_x: i32,
        world_z: i32,
        height: i32,
        sample: TerrainSample,
    ) -> Identifier {
        let continental = sample.continentalness;
        let temperature = sample.temperature;
        let moisture = sample.moisture;
        let ridges = sample.ridges;
        let river = sample.river;

        if height < SEA_LEVEL - 8 {
            return self
                .biomes
                .pick_region_band(&self.biomes.deep_ocean, world_x, world_z);
        }
        if river.abs() < RIVER_BIOME_WIDTH && continental > -0.05 && height <= SEA_LEVEL {
            return self
                .biomes
                .pick(&self.biomes.river, world_x, world_z, 0x5249_5645);
        }
        if height < SEA_LEVEL - 1 {
            return self
                .biomes
                .pick(&self.biomes.ocean, world_x, world_z, 0x4F43_4541);
        }
        if height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA {
            return self
                .biomes
                .pick(&self.biomes.beach, world_x, world_z, 0x4245_4143);
        }
        if height > 118 || ridges > 0.22 {
            return self
                .biomes
                .pick(&self.biomes.mountain, world_x, world_z, 0x4D4F_554E);
        }
        if height < 18 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, 0x4341_5645);
        }
        if moisture > 0.62 && height <= SEA_LEVEL + 8 {
            return self
                .biomes
                .pick(&self.biomes.swamp, world_x, world_z, 0x5357_414D);
        }
        if temperature < -0.25 {
            return self
                .biomes
                .pick(&self.biomes.cold, world_x, world_z, 0x434F_4C44);
        }
        if temperature > 0.38 && moisture < -0.08 {
            return self
                .biomes
                .pick(&self.biomes.hot_dry, world_x, world_z, 0x484F_5444);
        }
        if temperature > 0.22 && moisture > 0.2 {
            return self
                .biomes
                .pick(&self.biomes.jungle, world_x, world_z, 0x4A55_4E47);
        }
        if moisture > 0.04 {
            self.biomes
                .pick(&self.biomes.temperate_forest, world_x, world_z, 0x464F_5253)
        } else {
            self.biomes
                .pick(&self.biomes.grassland, world_x, world_z, 0x4752_4153)
        }
    }

    fn tellus_biome_for(
        &self,
        world_x: i32,
        world_z: i32,
        height: i32,
        settings: TellusWorldgenSettings,
        sample: TerrainSample,
    ) -> Identifier {
        let sea_level = settings.sea_level;
        let height_y = i64::from(height);
        let sea_y = i64::from(sea_level);
        let land_mask = sample.continentalness;
        let mountain = sample.ridges;
        let river = sample.river;

        if settings.water_enabled {
            if river.abs() < RIVER_BIOME_WIDTH * 0.65 && land_mask > -0.02 && height_y <= sea_y {
                return self
                    .biomes
                    .pick(&self.biomes.river, world_x, world_z, 0x5452_4956);
            }
            if height_y < sea_y - 18 {
                return self
                    .biomes
                    .pick(&self.biomes.deep_ocean, world_x, world_z, 0x5444_4545);
            }
            if height_y < sea_y - 1 {
                return self
                    .biomes
                    .pick(&self.biomes.ocean, world_x, world_z, 0x544F_434E);
            }
        }
        let near_coast = land_mask.abs() < 0.025 && height_y <= sea_y + 6;
        if near_coast || height_y <= sea_y + i64::from(BEACH_HEIGHT_ABOVE_SEA) {
            return self
                .biomes
                .pick(&self.biomes.beach, world_x, world_z, 0x5442_4541);
        }
        // A ridge field may cross its threshold on a low coastal shelf. Only
        // route that shelf to a rocky mountain surface once the terrain has
        // actually risen above ordinary lowland.
        if height_y > sea_y + 86 || (mountain > 0.22 && land_mask > 0.08 && height_y >= sea_y + 18)
        {
            return self
                .biomes
                .pick(&self.biomes.mountain, world_x, world_z, 0x544D_4F55);
        }
        if height < 18 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, 0x5443_4156);
        }
        if sample.moisture > 0.62 && height_y <= sea_y + 8 {
            return self
                .biomes
                .pick(&self.biomes.swamp, world_x, world_z, 0x5453_5741);
        }
        if sample.temperature < -0.25 {
            return self
                .biomes
                .pick(&self.biomes.cold, world_x, world_z, 0x5443_4F4C);
        }
        if sample.temperature > 0.38 && sample.moisture < -0.08 {
            return self
                .biomes
                .pick(&self.biomes.hot_dry, world_x, world_z, 0x5448_4F54);
        }
        if sample.temperature > 0.22 && sample.moisture > 0.2 {
            return self
                .biomes
                .pick(&self.biomes.jungle, world_x, world_z, 0x544A_554E);
        }
        if sample.moisture > 0.04 {
            self.biomes
                .pick(&self.biomes.temperate_forest, world_x, world_z, 0x5446_4F52)
        } else {
            self.biomes
                .pick(&self.biomes.grassland, world_x, world_z, 0x5447_5241)
        }
    }

    #[cfg(test)]
    fn biome_for_cell(
        &self,
        world_x: i32,
        y: i32,
        world_z: i32,
        surface_height: i32,
    ) -> Identifier {
        if i64::from(y) < i64::from(surface_height) - 24 && y < 32 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, 0x554E_4447);
        }
        self.biome_for(world_x, world_z, surface_height)
    }

    #[cfg(test)]
    fn ridges(&self, world_x: i32, world_z: i32) -> f64 {
        self.density_router().sample(world_x, world_z).ridges
    }

    fn vegetation_density(&self, world_x: i32, world_z: i32, moisture: f64) -> f64 {
        let regional = fbm_2d(
            f64::from(world_x) / VEGETATION_REGION_SCALE,
            f64::from(world_z) / VEGETATION_REGION_SCALE,
            self.seed ^ 0x5645_4745,
            2,
            0.5,
        );
        (regional * 0.72 + moisture * 0.28).clamp(-1.0, 1.0)
    }

    fn plan_column(&self, pos: ChunkPos, lx: u8, lz: u8) -> ColumnPlan {
        let wx = world_block_coordinate(pos.x, lx);
        let wz = world_block_coordinate(pos.z, lz);
        let sample = self.density_router().sample(wx, wz);
        let height = sample.surface_y;
        let biome = match self.worldgen_mode {
            WorldgenMode::VanillaLike => self.vanilla_biome_for(wx, wz, height, sample),
            WorldgenMode::TellusLike(settings) => {
                self.tellus_biome_for(wx, wz, height, settings, sample)
            }
        };
        let vegetation_density = if self.biomes.grassland.contains(&biome)
            || self.biomes.temperate_forest.contains(&biome)
            || self.biomes.jungle.contains(&biome)
            || Self::is_cold_forest(&biome)
            || Self::is_savanna(&biome)
        {
            self.vegetation_density(wx, wz, sample.moisture)
        } else {
            -1.0
        };
        let (sea_level, water_enabled) = match self.worldgen_mode {
            WorldgenMode::VanillaLike => (SEA_LEVEL, true),
            WorldgenMode::TellusLike(settings) => (settings.sea_level, settings.water_enabled),
        };
        let (mut surface, mut fill) = self.surface_materials(&biome);
        if self.biomes.mountain.contains(&biome) && height >= sea_level + 112 {
            surface = self.snow_block;
            fill = self.stone;
        }
        let top_non_air = if water_enabled && (height < sea_level || self.biomes.is_river(&biome)) {
            let inclusive_top = checked_y_offset(self.geometry.max_y(), -1).unwrap_or(height);
            sea_level.clamp(height, inclusive_top)
        } else {
            height
        };
        let minimum_fill_y =
            checked_y_offset(self.geometry.min_y(), 1).unwrap_or(self.geometry.min_y());
        ColumnPlan {
            lx,
            lz,
            wx,
            wz,
            height,
            top_non_air,
            dirt_start: checked_y_offset(height, -DIRT_DEPTH)
                .unwrap_or(minimum_fill_y)
                .max(minimum_fill_y),
            biome,
            surface,
            fill,
            vegetation_density,
            hash: feature_hash(self.seed, wx, height, wz, 0xDEC0_0001),
        }
    }

    fn fill_column(&self, chunk: &mut Chunk, plan: &ColumnPlan) {
        let min_y = self.geometry.min_y();
        let _ = chunk.set_block(plan.lx, min_y, plan.lz, self.bedrock);
        let Some(first_fill_y) = checked_y_offset(min_y, 1) else {
            return;
        };
        for y in first_fill_y..plan.dirt_start {
            let _ = chunk.set_block(
                plan.lx,
                y,
                plan.lz,
                self.base_stone_for_y(plan.lx, y, plan.lz, chunk.pos),
            );
        }
        for y in plan.dirt_start..plan.height {
            let _ = chunk.set_block(plan.lx, y, plan.lz, plan.fill);
        }
        let _ = chunk.set_block(plan.lx, plan.height, plan.lz, plan.surface);
        if plan.top_non_air > plan.height {
            let Some(first_water_y) = checked_y_offset(plan.height, 1) else {
                return;
            };
            for y in first_water_y..=plan.top_non_air {
                let _ = chunk.set_block(plan.lx, y, plan.lz, self.water);
            }
        }
    }

    fn surface_materials(&self, biome: &Identifier) -> (BlockStateId, BlockStateId) {
        let path = biome.path();
        if self.biomes.is_surface_water(biome) || self.biomes.is_beach_or_shore(biome) {
            return (self.sand, self.sand);
        }
        if path.contains("badlands") {
            return (self.red_sand, self.red_sand);
        }
        if path == "desert" {
            return (self.sand, self.sand);
        }
        if self.biomes.mountain.contains(biome) || path.contains("stony") {
            return (self.gravel, self.stone);
        }
        if self.biomes.cold.contains(biome) || path.contains("snow") || path.contains("frozen") {
            return (self.snow_block, self.dirt);
        }
        if self.biomes.jungle.contains(biome) {
            return (self.grass_block, self.dirt);
        }
        if path.contains("taiga") {
            return (self.podzol, self.dirt);
        }
        (self.grass_block, self.dirt)
    }

    fn assign_biomes(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        for (section_idx, section) in chunk.biomes.iter_mut().enumerate() {
            let mut palette: Vec<Identifier> = Vec::with_capacity(4);
            let mut indices = PackedBitArray::zeroed(6, BIOME_VOLUME);
            for cy in 0..BIOME_DIM {
                for cz in 0..BIOME_DIM {
                    for cx in 0..BIOME_DIM {
                        let lx = cx * 4 + 2;
                        let lz = cz * 4 + 2;
                        let column = &columns[lz * 16 + lx];
                        let y = i64::from(self.geometry.min_y())
                            + section_idx as i64 * 16
                            + cy as i64 * 4
                            + 2;
                        let Ok(y) = i32::try_from(y) else {
                            continue;
                        };
                        let biome = if i64::from(y) < i64::from(column.height) - 24 && y < 32 {
                            self.biomes
                                .pick(&self.biomes.cave, column.wx, column.wz, 0x554E_4447)
                        } else {
                            column.biome.clone()
                        };
                        let palette_idx = palette
                            .iter()
                            .position(|entry| entry == &biome)
                            .unwrap_or_else(|| {
                                palette.push(biome);
                                palette.len() - 1
                            });
                        let idx = (cy * BIOME_DIM + cz) * BIOME_DIM + cx;
                        indices.set(idx, palette_idx as u32);
                    }
                }
            }
            if palette.len() == 1 {
                *section = BiomeSection::filled(palette.pop().unwrap());
            } else {
                *section = BiomeSection::from_indirect(palette, indices);
            }
        }
    }

    fn apply_caves(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        const RAW_SIDE: usize = 18;
        let cave_min_y = self.geometry.min_y().saturating_add(8);
        let cave_max_y = columns
            .iter()
            .filter_map(|plan| self.cave_y_bounds(plan).map(|(_, max_y)| max_y))
            .max()
            .unwrap_or(cave_min_y.saturating_sub(1))
            .min(31);
        if cave_max_y < cave_min_y {
            return;
        }

        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        let chunk_min = (chunk_min_x, chunk_min_z);
        let router = self.density_router();
        let mut raw = [0_u8; RAW_SIDE * RAW_SIDE];
        for sample_y in (cave_min_y..=cave_max_y).step_by(CAVE_VERTICAL_SAMPLE_STEP as usize) {
            raw.fill(0);
            for raw_z in 1..=16 {
                for raw_x in 1..=16 {
                    let _ = cached_raw_cave(
                        &mut raw,
                        RAW_SIDE,
                        router,
                        chunk_min,
                        sample_y,
                        (raw_x, raw_z),
                    );
                }
            }

            for plan in columns {
                let Some((plan_min_y, plan_max_y)) = self.cave_y_bounds(plan) else {
                    continue;
                };
                let cave_limit = plan.height.saturating_sub(CAVE_SURFACE_CLEARANCE).min(32);
                if sample_y < plan_min_y || sample_y > plan_max_y || sample_y >= cave_limit {
                    continue;
                }
                let raw_x = usize::from(plan.lx) + 1;
                let raw_z = usize::from(plan.lz) + 1;
                let center = raw_z * RAW_SIDE + raw_x;
                if raw[center] != 2 {
                    continue;
                }
                let connected = cached_raw_cave(
                    &mut raw,
                    RAW_SIDE,
                    router,
                    chunk_min,
                    sample_y,
                    (raw_x - 1, raw_z),
                ) || cached_raw_cave(
                    &mut raw,
                    RAW_SIDE,
                    router,
                    chunk_min,
                    sample_y,
                    (raw_x + 1, raw_z),
                ) || cached_raw_cave(
                    &mut raw,
                    RAW_SIDE,
                    router,
                    chunk_min,
                    sample_y,
                    (raw_x, raw_z - 1),
                ) || cached_raw_cave(
                    &mut raw,
                    RAW_SIDE,
                    router,
                    chunk_min,
                    sample_y,
                    (raw_x, raw_z + 1),
                );
                if !connected {
                    continue;
                }
                for y in sample_y..sample_y.saturating_add(CAVE_VERTICAL_SAMPLE_STEP) {
                    if y > plan_max_y || y >= self.geometry.max_y() {
                        break;
                    }
                    let _ = chunk.set_block(plan.lx, y, plan.lz, self.air);
                }
            }
        }
    }

    fn cave_y_bounds(&self, plan: &ColumnPlan) -> Option<(i32, i32)> {
        let cave_min_y = i64::from(self.geometry.min_y()) + 8;
        let cave_max_y = (i64::from(plan.height) - i64::from(CAVE_SURFACE_CLEARANCE))
            .min(i64::from(plan.dirt_start) - 1);
        if cave_max_y < cave_min_y {
            return None;
        }
        Some((
            i32::try_from(cave_min_y).ok()?,
            i32::try_from(cave_max_y).ok()?,
        ))
    }

    fn ore_column_cache(&self, chunk: &Chunk, chunk_columns: &[ColumnPlan; 256]) -> OreColumnCache {
        let min_x_i64 = i64::from(chunk.pos.x) * 16 - i64::from(ORE_COLUMN_HALO);
        let min_z_i64 = i64::from(chunk.pos.z) * 16 - i64::from(ORE_COLUMN_HALO);
        let side = usize::try_from(16 + ORE_COLUMN_HALO * 2)
            .expect("bounded ore column cache side fits usize");
        let min_x = min_x_i64;
        let min_z = min_z_i64;
        let cave_min_y = self.geometry.min_y().saturating_add(8);
        let cave_max_y = self.geometry.max_y().saturating_sub(1).min(31);
        let cave_layers = if cave_max_y < cave_min_y {
            0
        } else {
            usize::try_from((cave_max_y - cave_min_y) / CAVE_VERTICAL_SAMPLE_STEP + 1)
                .expect("bounded cave layer count fits usize")
        };
        let mut cache = OreColumnCache {
            min_x,
            min_z,
            side,
            columns: vec![None; side * side],
            cave_min_y,
            cave_layers,
            cave_samples: vec![0; side * side * cave_layers],
        };
        let center_offset = usize::try_from(ORE_COLUMN_HALO).expect("ore halo fits usize");
        for plan in chunk_columns {
            let local_x = center_offset + usize::from(plan.lx);
            let local_z = center_offset + usize::from(plan.lz);
            cache.columns[local_z * side + local_x] = Some(plan.clone());
        }
        cache
    }

    fn apply_ores(&self, chunk: &mut Chunk, chunk_columns: &[ColumnPlan; 256]) {
        if let Some(geological_ores) = &self.geological_ores {
            geological_ores.apply(self, chunk);
            return;
        }
        let mut column_cache = self.ore_column_cache(chunk, chunk_columns);
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        // Re-evaluate nearby anchors so a vein crossing a chunk edge is derived
        // identically no matter which side is generated first.
        let radius = i64::from(ORE_VEIN_RADIUS);
        let cell_edge = i64::from(ORE_ANCHOR_CELL_EDGE);
        let min_cell_x = (chunk_min_x - radius).div_euclid(cell_edge);
        let max_cell_x = (chunk_min_x + 15 + radius).div_euclid(cell_edge);
        let min_cell_z = (chunk_min_z - radius).div_euclid(cell_edge);
        let max_cell_z = (chunk_min_z + 15 + radius).div_euclid(cell_edge);

        for (rule_index, rule) in self.ores.rules().iter().enumerate() {
            let Some(first_stone_y) = checked_y_offset(self.geometry.min_y(), 1) else {
                continue;
            };
            let Some(inclusive_top) = checked_y_offset(self.geometry.max_y(), -1) else {
                continue;
            };
            let min_y = rule.y.min.max(first_stone_y);
            let max_y = rule.y.max.min(inclusive_top);
            if min_y > max_y {
                continue;
            }
            let vein_size = usize::try_from(rule.size)
                .unwrap_or(MAX_ORE_VEIN_SIZE)
                .clamp(1, MAX_ORE_VEIN_SIZE);
            let min_cell_y = min_y.div_euclid(ORE_ANCHOR_CELL_EDGE);
            let max_cell_y = max_y.div_euclid(ORE_ANCHOR_CELL_EDGE);
            let rule_salt = 0x0AE0_0000_u64 ^ rule_index as u64;

            for cell_y in min_cell_y..=max_cell_y {
                let sample_y = i64::from(cell_y) * i64::from(ORE_ANCHOR_CELL_EDGE)
                    + i64::from(ORE_ANCHOR_CELL_EDGE / 2);
                let Ok(sample_y) = i32::try_from(sample_y) else {
                    continue;
                };
                let spacing = rule.spacing.at_y(sample_y, rule.y).max(1);
                let denominator = spacing.saturating_mul(vein_size as u64).max(1);
                for cell_z in min_cell_z..=max_cell_z {
                    for cell_x in min_cell_x..=max_cell_x {
                        let cell_x_i32 =
                            i32::try_from(cell_x).expect("valid chunk halo cell x fits i32");
                        let cell_z_i32 =
                            i32::try_from(cell_z).expect("valid chunk halo cell z fits i32");
                        let cell_hash =
                            feature_hash(self.seed, cell_x_i32, cell_y, cell_z_i32, rule_salt);
                        let guaranteed = ORE_ANCHOR_CELL_VOLUME / denominator;
                        let remainder = ORE_ANCHOR_CELL_VOLUME % denominator;
                        let extra = usize::from(
                            remainder != 0 && cell_hash.rotate_left(29) % denominator < remainder,
                        );
                        let anchor_count = usize::try_from(guaranteed)
                            .unwrap_or(MAX_ORE_ANCHORS_PER_CELL)
                            .saturating_add(extra)
                            .min(MAX_ORE_ANCHORS_PER_CELL);
                        if anchor_count == 0 {
                            continue;
                        }

                        let start = (cell_hash & 63) as usize;
                        let step = (((cell_hash >> 6) & 31) as usize) * 2 + 1;
                        for slot in 0..anchor_count {
                            let cell_index = (start + slot * step) & 63;
                            let anchor_x = cell_x * cell_edge + (cell_index & 3) as i64;
                            let anchor_z = cell_z * cell_edge + ((cell_index >> 2) & 3) as i64;
                            let anchor_y =
                                i64::from(cell_y) * cell_edge + ((cell_index >> 4) & 3) as i64;
                            let (Ok(anchor_x), Ok(anchor_y), Ok(anchor_z)) = (
                                i32::try_from(anchor_x),
                                i32::try_from(anchor_y),
                                i32::try_from(anchor_z),
                            ) else {
                                continue;
                            };
                            if !(min_y..=max_y).contains(&anchor_y) {
                                continue;
                            }
                            if !rule.biomes.is_any() {
                                let surface_y = self.surface_height(anchor_x, anchor_z);
                                let biome = self.biome_for(anchor_x, anchor_z, surface_y);
                                if !rule.biomes.matches(&biome) {
                                    continue;
                                }
                            }
                            let vein_hash = feature_hash(
                                self.seed,
                                anchor_x,
                                anchor_y,
                                anchor_z,
                                rule_salt ^ slot as u64,
                            );
                            self.place_ore_vein(
                                chunk,
                                &mut column_cache,
                                rule,
                                [anchor_x, anchor_y, anchor_z],
                                vein_hash,
                                vein_size,
                            );
                        }
                    }
                }
            }
        }
    }

    fn place_ore_vein(
        &self,
        chunk: &mut Chunk,
        column_cache: &mut OreColumnCache,
        rule: &OreRule,
        anchor: [i32; 3],
        vein_hash: u64,
        vein_size: usize,
    ) {
        let mut offsets = [[0_i8; 3]; MAX_ORE_VEIN_SIZE];
        let offset_count = connected_ore_offsets(
            &mut offsets,
            vein_hash,
            anchor[1],
            vein_size,
            |offset, cell_hash| {
                self.ore_cell_can_generate(column_cache, rule, anchor, offset, cell_hash)
            },
        );
        for &offset in &offsets[..offset_count] {
            self.place_ore_cell(chunk, rule, anchor, offset);
        }
    }

    fn ore_cell_can_generate(
        &self,
        column_cache: &mut OreColumnCache,
        rule: &OreRule,
        anchor: [i32; 3],
        offset: [i8; 3],
        cell_hash: u64,
    ) -> bool {
        let Some(world_x) = anchor[0].checked_add(i32::from(offset[0])) else {
            return false;
        };
        let Some(world_y) = anchor[1].checked_add(i32::from(offset[1])) else {
            return false;
        };
        let Some(world_z) = anchor[2].checked_add(i32::from(offset[2])) else {
            return false;
        };
        if !(rule.y.min..=rule.y.max).contains(&world_y) {
            return false;
        }
        let Some(base) = self.generated_cell_before_ores(column_cache, world_x, world_y, world_z)
        else {
            return false;
        };
        if base != self.stone && base != self.deepslate {
            return false;
        }
        !self.should_discard_exposed_ore(column_cache, world_x, world_y, world_z, rule, cell_hash)
    }

    fn place_ore_cell(&self, chunk: &mut Chunk, rule: &OreRule, anchor: [i32; 3], offset: [i8; 3]) {
        let world_x = i64::from(anchor[0]) + i64::from(offset[0]);
        let Some(world_y) = anchor[1].checked_add(i32::from(offset[1])) else {
            return;
        };
        let world_z = i64::from(anchor[2]) + i64::from(offset[2]);
        let chunk_min_x = i64::from(chunk.pos.x) * 16;
        let chunk_min_z = i64::from(chunk.pos.z) * 16;
        if !(chunk_min_x..chunk_min_x + 16).contains(&world_x)
            || !(chunk_min_z..chunk_min_z + 16).contains(&world_z)
        {
            return;
        }
        let lx = (world_x - chunk_min_x) as u8;
        let lz = (world_z - chunk_min_z) as u8;
        let Some(base) = chunk.get_block(lx, world_y, lz) else {
            return;
        };
        if base != self.stone && base != self.deepslate {
            return;
        }
        let ore = self.ore_variant(base, rule.normal, rule.deepslate);
        let _ = chunk.set_block(lx, world_y, lz, ore);
    }

    fn should_discard_exposed_ore(
        &self,
        column_cache: &mut OreColumnCache,
        world_x: i32,
        y: i32,
        world_z: i32,
        rule: &OreRule,
        hash: u64,
    ) -> bool {
        let chance = rule.discard_chance_on_air_exposure;
        if !chance.is_finite()
            || chance <= 0.0
            || !self.generated_cell_touches_air(column_cache, world_x, y, world_z)
        {
            return false;
        }
        if chance >= 1.0 {
            return true;
        }
        let sample = (hash >> 11) as f64 * (1.0 / ((1_u64 << 53) as f64));
        sample < chance
    }

    fn generated_cell_touches_air(
        &self,
        column_cache: &mut OreColumnCache,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> bool {
        ORE_DIRECTIONS.iter().any(|direction| {
            let neighbour = world_x
                .checked_add(i32::from(direction[0]))
                .zip(y.checked_add(i32::from(direction[1])))
                .zip(world_z.checked_add(i32::from(direction[2])));
            let Some(((nx, ny), nz)) = neighbour else {
                return true;
            };
            self.generated_cell_before_ores(column_cache, nx, ny, nz) == Some(self.air)
        })
    }

    fn generated_cell_before_ores(
        &self,
        column_cache: &mut OreColumnCache,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> Option<BlockStateId> {
        if y < self.geometry.min_y() || y >= self.geometry.max_y() {
            return None;
        }
        let (top_non_air, height, dirt_start, fill, surface, cave_bounds) = {
            let plan = column_cache.get_or_plan(self, world_x, world_z)?;
            (
                plan.top_non_air,
                plan.height,
                plan.dirt_start,
                plan.fill,
                plan.surface,
                self.cave_y_bounds(plan),
            )
        };
        let pos = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        if y > top_non_air {
            return Some(self.air);
        }
        if y > height {
            return Some(self.water);
        }
        if y == self.geometry.min_y() {
            return Some(self.bedrock);
        }

        if let Some((cave_min_y, cave_max_y)) = cave_bounds
            && (cave_min_y..=cave_max_y).contains(&y)
        {
            let step = i64::from(CAVE_VERTICAL_SAMPLE_STEP);
            let sample_y = i64::from(cave_min_y)
                + (i64::from(y) - i64::from(cave_min_y)).div_euclid(step) * step;
            let Ok(sample_y) = i32::try_from(sample_y) else {
                return None;
            };
            if column_cache.get_or_cave(self, world_x, sample_y, world_z, height) {
                return Some(self.air);
            }
        }
        if y < dirt_start {
            return Some(self.base_stone_for_y(
                world_x.rem_euclid(16) as u8,
                y,
                world_z.rem_euclid(16) as u8,
                pos,
            ));
        }
        if y < height {
            return Some(fill);
        }
        Some(surface)
    }

    fn apply_structures(&self, chunk: &mut Chunk) {
        if self.structures.is_empty() {
            return;
        }

        let grid = self.structures.grid_chunks();
        let cell_x = chunk.pos.x.div_euclid(grid);
        let cell_z = chunk.pos.z.div_euclid(grid);
        let mut touched = [false; 256];
        for gx in (cell_x - 1)..=(cell_x + 1) {
            for gz in (cell_z - 1)..=(cell_z + 1) {
                self.apply_structure_cell(chunk, gx, gz, &mut touched);
            }
        }
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                if touched[lz as usize * 16 + lx as usize] {
                    self.refresh_structure_column(chunk, lx, lz);
                }
            }
        }
    }

    fn apply_decorations(&self, chunk: &mut Chunk, columns: &[ColumnPlan; 256]) {
        let mut touched = [None; 256];
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let idx = lz as usize * 16 + lx as usize;
                let plan = &columns[idx];
                let height = plan.height;
                let Some(decoration_limit) = checked_y_offset(height, 8) else {
                    continue;
                };
                let Some(base_y) = checked_y_offset(height, 1) else {
                    continue;
                };
                if height <= self.geometry.min_y() || decoration_limit >= self.geometry.max_y() {
                    continue;
                }
                let biome = &plan.biome;
                let surface = plan.surface;
                let h = plan.hash;

                if chunk.get_block(lx, height, lz) != Some(surface) {
                    continue;
                }

                let tree_spacing = self.tree_spacing_for_biome(biome);
                if tree_spacing.is_some_and(|spacing| h.is_multiple_of(spacing))
                    && self.tree_density_allows(plan)
                    && self.tree_site_is_stable(plan)
                    && self.place_tree(chunk, plan, self.tree_blocks_for_biome(biome), &mut touched)
                {
                    continue;
                }
                if self.biomes.hot_dry.contains(biome)
                    && surface == self.sand
                    && h.is_multiple_of(47)
                    && self.place_cactus(chunk, lx, base_y, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.beach.contains(biome) || self.biomes.river.contains(biome))
                    && h.is_multiple_of(29)
                    && self.place_sugar_cane(chunk, lx, base_y, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.grassland.contains(biome)
                    || self.biomes.temperate_forest.contains(biome)
                    || self.biomes.jungle.contains(biome)
                    || Self::is_savanna(biome))
                    && (surface == self.grass_block || surface == self.podzol)
                    && self.ground_cover_density_allows(plan)
                {
                    let (grass_spacing, dandelion_spacing, poppy_spacing) =
                        self.plant_spacing_for_biome(biome);
                    let plant = if Self::is_savanna(biome) {
                        if h.is_multiple_of(grass_spacing) {
                            self.decorations.short_grass
                        } else {
                            None
                        }
                    } else if h.is_multiple_of(1021) {
                        self.decorations.pumpkin
                    } else if h.is_multiple_of(dandelion_spacing) {
                        self.decorations.dandelion
                    } else if h.is_multiple_of(poppy_spacing) {
                        self.decorations.poppy
                    } else if h.is_multiple_of(grass_spacing) {
                        self.decorations.short_grass
                    } else {
                        None
                    };
                    if let Some(plant) = plant
                        && chunk.get_block(lx, base_y, lz) == Some(self.air)
                    {
                        self.place_single(chunk, lx, base_y, lz, plant, &mut touched);
                    }
                }
                if self.biomes.is_ocean(biome)
                    && checked_y_offset(height, 2)
                        .is_some_and(|minimum_top| plan.top_non_air > minimum_top)
                {
                    if h.is_multiple_of(31) && self.place_kelp_column(chunk, plan, h, &mut touched)
                    {
                        continue;
                    }
                    if h.is_multiple_of(17)
                        && let Some(seagrass) = self.decorations.seagrass
                        && chunk.get_block(lx, base_y, lz) == Some(self.water)
                    {
                        self.place_single(chunk, lx, base_y, lz, seagrass, &mut touched);
                    }
                }
            }
        }
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                if let Some(top) = touched[lz as usize * 16 + lx as usize] {
                    self.refresh_known_top_column(chunk, lx, lz, top);
                }
            }
        }
    }

    fn tree_site_is_stable(&self, plan: &ColumnPlan) -> bool {
        for dz in -2..=2 {
            for dx in -2..=2 {
                let Some(wx) = plan.wx.checked_add(dx) else {
                    return false;
                };
                let Some(wz) = plan.wz.checked_add(dz) else {
                    return false;
                };
                let neighbour = self.surface_height(wx, wz);
                if (neighbour - plan.height).abs() > 1 {
                    return false;
                }
            }
        }
        true
    }

    fn is_savanna(biome: &Identifier) -> bool {
        biome.path().contains("savanna")
    }

    fn is_cold_forest(biome: &Identifier) -> bool {
        biome.path().contains("taiga") || biome.path() == "grove"
    }

    fn tree_density_allows(&self, plan: &ColumnPlan) -> bool {
        let threshold = if self.biomes.jungle.contains(&plan.biome) {
            -0.55
        } else if self.biomes.temperate_forest.contains(&plan.biome) {
            -0.25
        } else if Self::is_cold_forest(&plan.biome) {
            -0.05
        } else if Self::is_savanna(&plan.biome) {
            0.28
        } else if self.biomes.grassland.contains(&plan.biome) {
            0.48
        } else {
            return false;
        };
        plan.vegetation_density >= threshold
    }

    fn ground_cover_density_allows(&self, plan: &ColumnPlan) -> bool {
        let threshold = if self.biomes.jungle.contains(&plan.biome) {
            -0.75
        } else if self.biomes.temperate_forest.contains(&plan.biome) {
            -0.55
        } else if Self::is_savanna(&plan.biome) {
            -0.05
        } else if self.biomes.grassland.contains(&plan.biome) {
            -0.35
        } else {
            return false;
        };
        plan.vegetation_density >= threshold
    }

    fn tree_spacing_for_biome(&self, biome: &Identifier) -> Option<u64> {
        if self.biomes.jungle.contains(biome) {
            Some(23)
        } else if self.biomes.temperate_forest.contains(biome) {
            Some(37)
        } else if Self::is_cold_forest(biome) {
            Some(47)
        } else if Self::is_savanna(biome) {
            Some(113)
        } else if self.biomes.grassland.contains(biome) {
            Some(173)
        } else {
            None
        }
    }

    fn plant_spacing_for_biome(&self, biome: &Identifier) -> (u64, u64, u64) {
        if Self::is_savanna(biome) {
            (31, 127, 137)
        } else if self.biomes.jungle.contains(biome) {
            (17, 103, 109)
        } else if self.biomes.temperate_forest.contains(biome) {
            (29, 127, 137)
        } else {
            (19, 61, 67)
        }
    }

    fn place_tree(
        &self,
        chunk: &mut Chunk,
        plan: &ColumnPlan,
        blocks: Option<TreeBlocks>,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let lx = plan.lx;
        let lz = plan.lz;
        let Some(base_y) = checked_y_offset(plan.height, 1) else {
            return false;
        };
        let Some(blocks) = blocks else {
            return false;
        };
        let trunk_height = match blocks.kind {
            TreeKind::Oak => 4 + (plan.hash % 2) as i32,
            TreeKind::Birch => 5 + (plan.hash % 2) as i32,
            TreeKind::Spruce => 5 + (plan.hash % 3) as i32,
            TreeKind::Jungle => 6 + (plan.hash % 2) as i32,
            TreeKind::Acacia => 4 + (plan.hash % 3) as i32,
        };
        let Some(trunk_top_y) = checked_y_offset(base_y, trunk_height - 1) else {
            return false;
        };
        let Some(top_y) = checked_y_offset(trunk_top_y, 1) else {
            return false;
        };
        if !(2..=13).contains(&lx) || !(2..=13).contains(&lz) || top_y >= self.geometry.max_y() {
            return false;
        }
        let Some(support_y) = checked_y_offset(base_y, -1) else {
            return false;
        };
        if chunk.get_block(lx, support_y, lz) != Some(plan.surface) {
            return false;
        }
        for y in base_y..=top_y {
            if chunk.get_block(lx, y, lz) != Some(self.air) {
                return false;
            }
        }
        for y in base_y..=trunk_top_y {
            self.place_single(chunk, lx, y, lz, blocks.log, touched);
        }
        for relative_y in -4..=1 {
            let Some(radius) = tree_canopy_radius(blocks.kind, relative_y) else {
                continue;
            };
            let Some(y) = checked_y_offset(trunk_top_y, relative_y) else {
                continue;
            };
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let offset = TreeLeafOffset {
                        relative_y,
                        dx,
                        dz,
                        radius,
                    };
                    if !self.tree_leaf_is_present(plan, blocks.kind, trunk_top_y, offset) {
                        continue;
                    }
                    let x = lx.wrapping_add_signed(dx);
                    let z = lz.wrapping_add_signed(dz);
                    if chunk.get_block(x, y, z) == Some(self.air) {
                        self.place_single(chunk, x, y, z, blocks.leaves, touched);
                    }
                }
            }
        }
        true
    }

    fn tree_blocks_for_biome(&self, biome: &Identifier) -> Option<TreeBlocks> {
        let (kind, log, leaves) = if Self::is_savanna(biome) {
            (
                TreeKind::Acacia,
                self.decorations.acacia_log,
                self.decorations.acacia_leaves,
            )
        } else if self.biomes.jungle.contains(biome) {
            (
                TreeKind::Jungle,
                self.decorations.jungle_log,
                self.decorations.jungle_leaves,
            )
        } else if Self::is_cold_forest(biome) {
            (
                TreeKind::Spruce,
                self.decorations.cold_log,
                self.decorations.cold_leaves,
            )
        } else if self.biomes.temperate_forest.contains(biome) {
            (
                TreeKind::Birch,
                self.decorations.forest_log,
                self.decorations.forest_leaves,
            )
        } else {
            (
                TreeKind::Oak,
                self.decorations.oak_log,
                self.decorations.oak_leaves,
            )
        };
        log.zip(leaves)
            .map(|(log, leaves)| TreeBlocks { kind, log, leaves })
    }

    fn tree_leaf_is_present(
        &self,
        plan: &ColumnPlan,
        kind: TreeKind,
        trunk_top_y: i32,
        offset: TreeLeafOffset,
    ) -> bool {
        let TreeLeafOffset {
            relative_y,
            dx,
            dz,
            radius,
        } = offset;
        let y = trunk_top_y + relative_y;
        if radius == 0 {
            return dx == 0 && dz == 0;
        }
        let edge_x = dx.unsigned_abs() == radius as u8;
        let edge_z = dz.unsigned_abs() == radius as u8;
        if edge_x && edge_z {
            if radius >= 2 {
                return false;
            }
            let corner = match (dx.is_positive(), dz.is_positive()) {
                (false, false) => 0,
                (true, false) => 1,
                (true, true) => 2,
                (false, true) => 3,
            };
            let salt = match kind {
                TreeKind::Oak => 0x0A4,
                TreeKind::Birch => 0xB17C,
                TreeKind::Spruce => 0x5A9C,
                TreeKind::Jungle => 0xA6E1,
                TreeKind::Acacia => 0xACA1,
            };
            let rotation = feature_hash(self.seed, plan.wx, trunk_top_y, plan.wz, salt) as u8 & 3;
            return if relative_y > 0 {
                corner == rotation
            } else {
                corner != rotation
            };
        }
        if radius < 2 || !(edge_x || edge_z) {
            return true;
        }
        let salt = match kind {
            TreeKind::Oak => 0x0A4,
            TreeKind::Birch => 0xB17C,
            TreeKind::Spruce => 0x5A9C,
            TreeKind::Jungle => 0xA6E1,
            TreeKind::Acacia => 0xACA1,
        };
        !feature_hash(
            self.seed,
            plan.wx + i32::from(dx),
            y,
            plan.wz + i32::from(dz),
            salt,
        )
        .is_multiple_of(5)
    }

    fn place_cactus(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let Some(cactus) = self.decorations.cactus else {
            return false;
        };
        let height =
            1 + (feature_hash(self.seed, lx as i32, base_y, lz as i32, 0xCA_C7) % 3) as i32;
        let Some(top_exclusive) = checked_y_offset(base_y, height) else {
            return false;
        };
        for y in base_y..top_exclusive {
            if chunk.get_block(lx, y, lz) != Some(self.air) {
                return false;
            }
        }
        for y in base_y..top_exclusive {
            self.place_single(chunk, lx, y, lz, cactus, touched);
        }
        true
    }

    fn place_sugar_cane(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let Some(sugar_cane) = self.decorations.sugar_cane else {
            return false;
        };
        let Some(below_y) = checked_y_offset(base_y, -1) else {
            return false;
        };
        if chunk.get_block(lx, base_y, lz) != Some(self.air)
            || !self.has_adjacent_water(chunk, lx, below_y, lz)
        {
            return false;
        }
        self.place_single(chunk, lx, base_y, lz, sugar_cane, touched);
        if let Some(above_y) = checked_y_offset(base_y, 1)
            && chunk.get_block(lx, above_y, lz) == Some(self.air)
        {
            self.place_single(chunk, lx, above_y, lz, sugar_cane, touched);
        }
        true
    }

    fn has_adjacent_water(&self, chunk: &Chunk, lx: u8, y: i32, lz: u8) -> bool {
        [(1i8, 0i8), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .any(|(dx, dz)| {
                let x = lx as i8 + dx;
                let z = lz as i8 + dz;
                if !(0..16).contains(&x) || !(0..16).contains(&z) {
                    return false;
                }
                let x = x as u8;
                let z = z as u8;
                chunk.get_block(x, y, z) == Some(self.water)
                    || checked_y_offset(y, 1).and_then(|above_y| chunk.get_block(x, above_y, z))
                        == Some(self.water)
            })
    }

    fn place_kelp_column(
        &self,
        chunk: &mut Chunk,
        plan: &ColumnPlan,
        hash: u64,
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let lx = plan.lx;
        let lz = plan.lz;
        let Some(base_y) = checked_y_offset(plan.height, 1) else {
            return false;
        };
        let water_top = plan.top_non_air;
        let Some(kelp) = self.decorations.kelp else {
            return false;
        };
        let stem = self.decorations.kelp_plant.unwrap_or(kelp);
        let available = i64::from(water_top) - i64::from(base_y) + 1;
        if available < 2 {
            return false;
        }
        let height = i64::from(2 + (hash % 5) as i32).min(available);
        let Ok(top_y) = i32::try_from(i64::from(base_y) + height - 1) else {
            return false;
        };
        for y in base_y..=top_y {
            if chunk.get_block(lx, y, lz) != Some(self.water) {
                return false;
            }
        }
        for y in base_y..top_y {
            self.place_single(chunk, lx, y, lz, stem, touched);
        }
        self.place_single(chunk, lx, top_y, lz, kelp, touched);
        true
    }

    fn place_single(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        y: i32,
        lz: u8,
        state: BlockStateId,
        touched: &mut [Option<i32>; 256],
    ) {
        if chunk.set_block(lx, y, lz, state).is_some() {
            let touched = &mut touched[lz as usize * 16 + lx as usize];
            *touched = Some(touched.map_or(y, |top| top.max(y)));
        }
    }

    fn refresh_known_top_column(&self, chunk: &mut Chunk, lx: u8, lz: u8, top: i32) {
        let current = chunk
            .heightmaps
            .get("MOTION_BLOCKING")
            .map(|heightmap| {
                i64::from(heightmap.get(lx, lz)) + i64::from(self.geometry.min_y()) - 1
            })
            .unwrap_or(i64::from(self.geometry.min_y()));
        let top = i64::from(top).max(current);
        let Ok(top) = i32::try_from(top) else {
            return;
        };
        let Some(value) = heightmap_value_for_top(self.geometry, top) else {
            return;
        };
        if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
            mb.set(lx, lz, value);
        }
        if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
            ws.set(lx, lz, value);
        }
        chunk.highest_opaque.set(lx, lz, value);
    }

    fn apply_structure_cell(
        &self,
        chunk: &mut Chunk,
        grid_x: i32,
        grid_z: i32,
        touched: &mut [bool; 256],
    ) {
        let Some((template, center_x, center_z)) = self.structure_plan(grid_x, grid_z) else {
            return;
        };
        let center_height = self.surface_height(center_x, center_z);
        if self.structures.fixed_center().is_none() {
            if center_height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA {
                return;
            }
            let biome = self.biome_for(center_x, center_z, center_height);
            if !self.biomes.grassland.contains(&biome) {
                return;
            }
        }

        let size = template.size();
        let origin_x = center_x - size[0] / 2;
        let Some(origin_y) = checked_y_offset(center_height, 1) else {
            return;
        };
        let origin_z = center_z - size[2] / 2;
        paste_template(chunk, template, origin_x, origin_y, origin_z, touched);
        let mut inhabitants = chunk.settlement_inhabitants();
        inhabitants.extend(
            self.structures
                .inhabitants()
                .iter()
                .zip(template.villager_markers())
                .filter_map(|(inhabitant, marker)| {
                    let x = origin_x.checked_add(marker[0])?;
                    let y = origin_y.checked_add(marker[1])?;
                    let z = origin_z.checked_add(marker[2])?;
                    (x.div_euclid(16) == chunk.pos.x && z.div_euclid(16) == chunk.pos.z).then(
                        || {
                            let position = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                            SettlementInhabitantMarker {
                                claim: format!("{}@{center_x},{center_z}", inhabitant.id),
                                entity_type: inhabitant.entity_type.clone(),
                                position,
                                villager_kind: inhabitant.villager_kind.clone(),
                                profession: inhabitant.profession.clone(),
                                level: inhabitant.level,
                                home: Some(position),
                                job_site: (inhabitant.profession != "none").then_some(position),
                                meeting_point: Some([
                                    f64::from(center_x) + 0.5,
                                    f64::from(origin_y),
                                    f64::from(center_z) + 0.5,
                                ]),
                            }
                        },
                    )
                }),
        );
        if !inhabitants.is_empty() {
            chunk.set_settlement_inhabitants(&inhabitants);
        }

        let mut vacant_homes = chunk.settlement_vacant_homes();
        vacant_homes.extend(
            template
                .villager_markers()
                .iter()
                .enumerate()
                .skip(self.structures.inhabitants().len())
                .take(1)
                .filter_map(|(slot, marker)| {
                    let x = origin_x.checked_add(marker[0])?;
                    let y = origin_y.checked_add(marker[1])?;
                    let z = origin_z.checked_add(marker[2])?;
                    (x.div_euclid(16) == chunk.pos.x && z.div_euclid(16) == chunk.pos.z).then(
                        || SettlementVacantHomeMarker {
                            claim: format!("solaris:vacant-home-{slot}@{center_x},{center_z}"),
                            position: [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5],
                        },
                    )
                }),
        );
        if !vacant_homes.is_empty() {
            chunk.set_settlement_vacant_homes(&vacant_homes);
        }
    }

    fn structure_plan(&self, grid_x: i32, grid_z: i32) -> Option<(&StructureTemplate, i32, i32)> {
        let templates = self.structures.templates();
        if templates.is_empty() {
            return None;
        }
        if let Some((center_x, center_z)) = self.structures.fixed_center() {
            return (grid_x == 0 && grid_z == 0).then_some((&templates[0], center_x, center_z));
        }
        let spacing = self.structures.grid_chunks();
        let separation = self.structures.separation_chunks();
        let usable = (spacing - separation * 2).max(1);
        let h = feature_hash(self.seed, grid_x, 0, grid_z, self.structures.salt());
        let x_offset = separation + (h % usable as u64) as i32;
        let z_offset = separation + ((h >> 16) % usable as u64) as i32;
        let template = &templates[((h >> 32) as usize) % templates.len()];
        let center_chunk_x = i64::from(grid_x) * i64::from(spacing) + i64::from(x_offset);
        let center_chunk_z = i64::from(grid_z) * i64::from(spacing) + i64::from(z_offset);
        let center_x = i32::try_from(center_chunk_x * 16 + 8).ok()?;
        let center_z = i32::try_from(center_chunk_z * 16 + 8).ok()?;
        Some((template, center_x, center_z))
    }

    fn refresh_structure_column(&self, chunk: &mut Chunk, lx: u8, lz: u8) {
        let top = (self.geometry.min_y()..self.geometry.max_y())
            .rev()
            .find(|&y| {
                chunk
                    .get_block(lx, y, lz)
                    .is_some_and(|state| state != self.air)
            })
            .unwrap_or(self.geometry.min_y());
        let Some(value) = heightmap_value_for_top(self.geometry, top) else {
            return;
        };
        if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
            mb.set(lx, lz, value);
        }
        if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
            ws.set(lx, lz, value);
        }
        chunk.highest_opaque.set(lx, lz, value);
    }

    fn base_stone_for_y(&self, lx: u8, y: i32, lz: u8, pos: ChunkPos) -> BlockStateId {
        if y <= DEEPSLATE_SOLID_Y {
            return self.deepslate;
        }
        if y > DEEPSLATE_TOP_Y {
            return self.stone;
        }
        let wx = world_block_coordinate(pos.x, lx);
        let wz = world_block_coordinate(pos.z, lz);
        let deepslate_chance = (DEEPSLATE_TOP_Y - y + 1) as u64;
        if feature_hash(self.seed, wx, y, wz, 0xD33F).is_multiple_of(9 - deepslate_chance) {
            self.deepslate
        } else {
            self.stone
        }
    }

    fn is_cave_cell(&self, x: i32, y: i32, z: i32, surface_y: i32) -> bool {
        self.density_router().is_cave(x, y, z, surface_y)
    }

    #[cfg(test)]
    fn ore_for(
        &self,
        x: i32,
        y: i32,
        z: i32,
        base: BlockStateId,
        biome: &Identifier,
    ) -> BlockStateId {
        let h = feature_hash(self.seed, x, y, z, 0x0A_E0);
        for rule in self.ores.rules() {
            if rule.matches(h, y, biome) {
                return self.ore_variant(base, rule.normal, rule.deepslate);
            }
        }
        base
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

fn ore_offset_is_available(existing: &[[i8; 3]], candidate: [i8; 3]) -> bool {
    candidate
        .iter()
        .all(|coordinate| i32::from(*coordinate).abs() <= ORE_VEIN_RADIUS)
        && !existing.contains(&candidate)
}

fn connected_ore_offsets(
    offsets: &mut [[i8; 3]; MAX_ORE_VEIN_SIZE],
    vein_hash: u64,
    anchor_y: i32,
    vein_size: usize,
    mut can_place: impl FnMut([i8; 3], u64) -> bool,
) -> usize {
    let vein_size = vein_size.min(MAX_ORE_VEIN_SIZE);
    if vein_size == 0 {
        return 0;
    }

    let mut candidates = [[0_i8; 3]; MAX_ORE_VEIN_SIZE];
    let mut parents = [0_usize; MAX_ORE_VEIN_SIZE];
    for candidate_index in 1..vein_size {
        let mut next = None;
        for attempt in 0..ORE_GROWTH_ATTEMPTS {
            let hash = feature_hash(
                vein_hash as i64,
                candidate_index as i32,
                attempt as i32,
                anchor_y,
                0x0AE0_600D,
            );
            let parent_index = usize::try_from(hash % candidate_index as u64)
                .expect("bounded ore parent index fits usize");
            let parent = candidates[parent_index];
            let direction = ORE_DIRECTIONS[((hash >> 16) % 6) as usize];
            let candidate = [
                parent[0] + direction[0],
                parent[1] + direction[1],
                parent[2] + direction[2],
            ];
            if ore_offset_is_available(&candidates[..candidate_index], candidate) {
                next = Some((candidate, parent_index));
                break;
            }
        }
        let (candidate, parent_index) = next.unwrap_or_else(|| {
            first_available_ore_offset(&candidates[..candidate_index])
                .expect("bounded ore vein cube has room")
        });
        candidates[candidate_index] = candidate;
        parents[candidate_index] = parent_index;
    }

    let mut accepted = [false; MAX_ORE_VEIN_SIZE];
    accepted[0] = can_place(candidates[0], vein_hash);
    let mut accepted_count = usize::from(accepted[0]);
    for candidate_index in 1..vein_size {
        let cell_hash = vein_hash ^ candidate_index as u64;
        accepted[candidate_index] =
            accepted[parents[candidate_index]] && can_place(candidates[candidate_index], cell_hash);
        if accepted[candidate_index] {
            offsets[accepted_count] = candidates[candidate_index];
            accepted_count += 1;
        }
    }
    accepted_count
}

fn first_available_ore_offset(existing: &[[i8; 3]]) -> Option<([i8; 3], usize)> {
    for (parent_index, parent) in existing.iter().enumerate() {
        for direction in ORE_DIRECTIONS {
            let candidate = [
                parent[0] + direction[0],
                parent[1] + direction[1],
                parent[2] + direction[2],
            ];
            if ore_offset_is_available(existing, candidate) {
                return Some((candidate, parent_index));
            }
        }
    }
    None
}

fn paste_template(
    chunk: &mut Chunk,
    template: &StructureTemplate,
    origin_x: i32,
    origin_y: i32,
    origin_z: i32,
    touched: &mut [bool; 256],
) {
    let min_x = world_block_coordinate(chunk.pos.x, 0);
    let min_z = world_block_coordinate(chunk.pos.z, 0);
    let geometry = chunk.geometry();
    for block in template.blocks() {
        let wx = origin_x + block.pos[0];
        let Some(wy) = origin_y.checked_add(block.pos[1]) else {
            continue;
        };
        let wz = origin_z + block.pos[2];
        if !(geometry.min_y()..geometry.max_y()).contains(&wy)
            || wx < min_x
            || wx >= min_x + 16
            || wz < min_z
            || wz >= min_z + 16
        {
            continue;
        }
        let lx = (wx - min_x) as u8;
        let lz = (wz - min_z) as u8;
        if chunk.set_block(lx, wy, lz, block.state).is_some() {
            touched[lz as usize * 16 + lx as usize] = true;
        }
    }
    for template_chest in template.chests() {
        let x = origin_x + template_chest.pos[0];
        let Some(y) = origin_y.checked_add(template_chest.pos[1]) else {
            continue;
        };
        let z = origin_z + template_chest.pos[2];
        if !(geometry.min_y()..geometry.max_y()).contains(&y)
            || x < min_x
            || x >= min_x + 16
            || z < min_z
            || z >= min_z + 16
        {
            continue;
        }
        let Some(expected_state) = template
            .blocks()
            .iter()
            .find(|block| block.pos == template_chest.pos)
            .map(|block| block.state)
        else {
            continue;
        };
        let lx = (x - min_x) as u8;
        let lz = (z - min_z) as u8;
        if chunk.get_block(lx, y, lz) != Some(expected_state) {
            continue;
        }
        chunk
            .chests
            .insert(mc_world::BlockPos { x, y, z }, template_chest.chest.clone());
    }
}

impl ChunkGenerator for TerrainGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk =
            Chunk::empty_with_geometry(pos, self.air, self.biomes.default.clone(), self.geometry);
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());
        let columns = std::array::from_fn(|idx| {
            let lx = (idx % 16) as u8;
            let lz = (idx / 16) as u8;
            self.plan_column(pos, lx, lz)
        });

        for plan in &columns {
            self.fill_column(&mut chunk, plan);
            // Heightmap value: Y of the first air cell above the
            // Heightmaps store offsets from this dimension's minimum Y.
            let Some(world_surface) = heightmap_value_for_top(self.geometry, plan.top_non_air)
            else {
                continue;
            };
            let Some(motion_blocking) = heightmap_value_for_top(self.geometry, plan.height) else {
                continue;
            };
            if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                mb.set(plan.lx, plan.lz, motion_blocking);
            }
            if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                ws.set(plan.lx, plan.lz, world_surface);
            }
            chunk.highest_opaque.set(plan.lx, plan.lz, motion_blocking);
        }
        self.apply_caves(&mut chunk, &columns);
        self.apply_ores(&mut chunk, &columns);
        self.assign_biomes(&mut chunk, &columns);
        self.apply_structures(&mut chunk);
        self.apply_decorations(&mut chunk, &columns);
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        chunk
    }
}

#[cfg(test)]
#[path = "terrain/tests.rs"]
mod tests;
