//! Baseline terrain generator (M7).
//!
//! Produces a fully-formed [`Chunk`] from `(ChunkPos, seed)` using
//! Solaris's own hash-noise — no vanilla algorithm involved (per
//! ADR 0001 / PROJECT_SPEC §8.1). M20 starts an earth-like generator
//! shape with large land/ocean masks, beaches, forests, caves, ores,
//! fluid placement primitives, and optional data-backed structure markers.
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
use mc_data::worldgen_features::{FeatureCount, WorldgenFeatureFacts};
use mc_world::chunk::{Chunk, ChunkGeometry, ChunkPos, Heightmap, OVERWORLD_GEOMETRY};
use mc_world::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockRegistry, BlockStateId, ChunkGenerator,
    PackedBitArray,
};

use crate::noise::fbm_2d;
use crate::structures::{StructureRules, StructureTemplate};

mod biome_rules;
mod ore_rules;

pub use biome_rules::BiomeRules;
pub use ore_rules::{
    BiomeScope, MAX_ORE_RULES, MAX_ORE_WORK_UNITS_PER_CHUNK, OreRule, OreRules, OreRulesError,
    OreSpacing, YRange,
};
use ore_rules::{
    MAX_ORE_ANCHORS_PER_CELL, MAX_ORE_VEIN_SIZE, ORE_ANCHOR_CELL_EDGE, ORE_ANCHOR_CELL_VOLUME,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TerrainGeneratorError {
    #[error("block registry missing required terrain block {name}")]
    MissingRequiredBlock { name: &'static str },
}

/// Default terrain centre. Chosen so the player spawns on top of
/// the surface without needing to fall.
const BASE_HEIGHT: f64 = 70.0;
/// Peak-to-trough amplitude of the height field (in blocks above /
/// below `BASE_HEIGHT`).
const HEIGHT_AMPLITUDE: f64 = 12.0;
/// Lattice spacing of the noise. Smaller = lumpier; this gives
/// broad hills instead of one-chunk bumps.
const NOISE_FREQUENCY: f64 = 1.0 / 40.0;
/// Octaves of fbm noise. Three is enough to round off the smooth
/// blobs of single-octave value-noise into something hill-shaped.
const NOISE_OCTAVES: u32 = 3;
const NOISE_PERSISTENCE: f64 = 0.5;
pub const SEA_LEVEL: i32 = 63;
pub const METERS_PER_DEGREE: f64 = 111_319.491_666_666_67;
pub const MAX_MERCATOR_LATITUDE: f64 = 85.051_128_78;
const CONTINENT_FREQUENCY: f64 = 1.0 / 420.0;
const COAST_DETAIL_FREQUENCY: f64 = 1.0 / 96.0;
const FOREST_FREQUENCY: f64 = 1.0 / 520.0;
const TEMPERATURE_SCALE: f64 = 900.0;
const RIVER_SIGNAL_SCALE: f64 = 360.0;
const RIVER_TERRAIN_CORE_WIDTH: f64 = 0.04;
const RIVER_TERRAIN_WIDTH: f64 = 0.16;
const RIVER_BIOME_WIDTH: f64 = 0.09;
const OCEAN_THRESHOLD: f64 = -0.16;
const BEACH_HEIGHT_ABOVE_SEA: i32 = 2;
const COAST_BLEND_WIDTH: f64 = 0.28;
const TELLUS_CONTINENT_SCALE: f64 = 22_000.0;
const TELLUS_COAST_SCALE: f64 = 6_500.0;
const TELLUS_HILL_SCALE: f64 = 2_400.0;
const TELLUS_CLIMATE_SCALE: f64 = 18_000.0;
const TELLUS_MOISTURE_SCALE: f64 = 16_000.0;
const TELLUS_MOUNTAIN_MASK_SCALE: f64 = 24_000.0;
const TELLUS_MOUNTAIN_DETAIL_SCALE: f64 = 3_200.0;
/// Number of dirt cells between grass cap and stone.
const DIRT_DEPTH: i32 = 3;
const ORE_VEIN_RADIUS: i32 = 4;
const ORE_GROWTH_ATTEMPTS: usize = 12;
const ORE_DIRECTIONS: [[i8; 3]; 6] = [
    [-1, 0, 0],
    [1, 0, 0],
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
];
const CAVE_SURFACE_CLEARANCE: i32 = 24;
const CAVE_MOUTH_GRID: i32 = 128;
const CAVE_MOUTH_RADIUS: i32 = CAVE_SURFACE_CLEARANCE;
const CAVE_MOUTH_SPAWN_SAFE_RADIUS: i32 = 24;
const CAVE_FREQUENCY: f64 = 1.0 / 34.0;
const CAVE_THRESHOLD: f64 = 0.24;
const CAVE_BRANCH_FREQUENCY: f64 = 1.0 / 58.0;
const CAVE_BRANCH_THRESHOLD: f64 = 0.32;
const DEEPSLATE_TOP_Y: i32 = 0;
const DEEPSLATE_SOLID_Y: i32 = -8;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MercatorProjection {
    world_scale_meters_per_block: f64,
}

impl MercatorProjection {
    #[must_use]
    pub fn new(world_scale_meters_per_block: f64) -> Self {
        Self {
            world_scale_meters_per_block: world_scale_meters_per_block.max(0.001),
        }
    }

    #[must_use]
    pub fn from_settings(settings: TellusWorldgenSettings) -> Self {
        Self::new(settings.world_scale_meters_per_block)
    }

    #[must_use]
    pub fn blocks_per_degree(&self) -> f64 {
        METERS_PER_DEGREE / self.world_scale_meters_per_block
    }

    #[must_use]
    pub fn lat_lon_to_block(&self, latitude_degrees: f64, longitude_degrees: f64) -> (f64, f64) {
        let lat = latitude_degrees.clamp(-MAX_MERCATOR_LATITUDE, MAX_MERCATOR_LATITUDE);
        let lon = longitude_degrees.clamp(-180.0, 180.0);
        let x = lon * self.blocks_per_degree();
        let lat_rad = lat.to_radians();
        let mercator_degrees = (lat_rad.tan() + lat_rad.cos().recip()).ln().to_degrees();
        let z = -mercator_degrees * self.blocks_per_degree();
        (x, z)
    }

    #[must_use]
    pub fn block_to_lat_lon(&self, x: f64, z: f64) -> (f64, f64) {
        let longitude = (x / self.blocks_per_degree()).clamp(-180.0, 180.0);
        let mercator_radians = (-z / self.blocks_per_degree()).to_radians();
        let latitude = mercator_radians.sinh().atan().to_degrees();
        (latitude, longitude)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TellusClimate {
    pub latitude_degrees: f64,
    pub temperature: f64,
    pub moisture: f64,
}

pub const GENERATION_STAGE_ORDER: &[&str] = &[
    "base_terrain_and_surfaces",
    "caves_and_ores",
    "biome_assignment",
    "surface_decorations",
    "structures",
    "heightmap_refresh",
    "persistence_or_streaming",
];

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
    iron_ore: BlockStateId,
    water: BlockStateId,
    biomes: BiomeRules,
    ores: OreRules,
    structures: StructureRules,
    decorations: DecorationBlocks,
    worldgen_mode: WorldgenMode,
    // Kept so the generator's lifetime is bounded by something
    // sensible if the storage drops the only other reference.
    #[allow(dead_code)]
    registry: Arc<BlockRegistry>,
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
    hash: u64,
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
    short_grass: Option<BlockStateId>,
    dandelion: Option<BlockStateId>,
    poppy: Option<BlockStateId>,
    flower_patch: Vec<BlockStateId>,
    grass_patch_spacing: u64,
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
            short_grass: optional_block(registry, "minecraft:short_grass"),
            dandelion: optional_block(registry, "minecraft:dandelion"),
            poppy: optional_block(registry, "minecraft:poppy"),
            flower_patch: ["minecraft:dandelion", "minecraft:poppy"]
                .into_iter()
                .filter_map(|name| optional_block(registry, name))
                .collect(),
            grass_patch_spacing: 11,
            pumpkin: optional_block(registry, "minecraft:pumpkin"),
            sugar_cane: optional_block(registry, "minecraft:sugar_cane"),
            cactus: optional_block(registry, "minecraft:cactus"),
            seagrass: optional_block(registry, "minecraft:seagrass"),
            kelp_plant: optional_block(registry, "minecraft:kelp_plant"),
            kelp: optional_block(registry, "minecraft:kelp"),
        }
    }

    fn from_feature_facts(registry: &BlockRegistry, features: &[WorldgenFeatureFacts]) -> Self {
        let mut blocks = Self::new(registry);
        for feature in features {
            match feature.placed_feature.as_str() {
                "minecraft:patch_grass_plain" => {
                    if let Some(state) = first_resolved_block(registry, &feature.block_states) {
                        blocks.short_grass = Some(state);
                    }
                    if let Some(spacing) = feature.placement.count.map(|count| {
                        decoration_spacing_from_count(count, blocks.grass_patch_spacing)
                    }) {
                        blocks.grass_patch_spacing = spacing;
                    }
                }
                "minecraft:flower_plain" => {
                    let states = resolve_blocks(registry, &feature.block_states);
                    if !states.is_empty() {
                        blocks.flower_patch = states;
                        blocks.dandelion = blocks.flower_patch.first().copied();
                        blocks.poppy = blocks.flower_patch.get(1).copied().or(blocks.dandelion);
                    }
                }
                "minecraft:patch_cactus" => {
                    if let Some(state) = first_resolved_block(registry, &feature.block_states) {
                        blocks.cactus = Some(state);
                    }
                }
                "minecraft:patch_sugar_cane" | "minecraft:sugar_cane" => {
                    if let Some(state) = first_resolved_block(registry, &feature.block_states) {
                        blocks.sugar_cane = Some(state);
                    }
                }
                "minecraft:seagrass_simple" | "minecraft:seagrass_normal" => {
                    if let Some(state) = first_resolved_block(registry, &feature.block_states) {
                        blocks.seagrass = Some(state);
                    }
                }
                "minecraft:kelp_cold" | "minecraft:kelp_warm" => {
                    if let Some(state) = first_resolved_block(registry, &feature.block_states) {
                        blocks.kelp = Some(state);
                    }
                    blocks.kelp_plant = blocks
                        .kelp_plant
                        .or_else(|| optional_block(registry, "minecraft:kelp_plant"));
                }
                _ => {}
            }
            let placed = feature.placed_feature.as_str();
            if placed.contains("trees") || placed.contains("tree") {
                let log = first_resolved_block_with_suffix(registry, &feature.block_states, "_log");
                let leaves = first_resolved_generated_leaves(registry, &feature.block_states);
                if placed.contains("jungle") {
                    blocks.jungle_log = log.or(blocks.jungle_log);
                    blocks.jungle_leaves = leaves.or(blocks.jungle_leaves);
                } else if placed.contains("taiga") || placed.contains("spruce") {
                    blocks.cold_log = log.or(blocks.cold_log);
                    blocks.cold_leaves = leaves.or(blocks.cold_leaves);
                } else if placed.contains("forest") || placed.contains("birch") {
                    blocks.forest_log = log.or(blocks.forest_log);
                    blocks.forest_leaves = leaves.or(blocks.forest_leaves);
                } else {
                    blocks.oak_log = log.or(blocks.oak_log);
                    blocks.oak_leaves = leaves.or(blocks.oak_leaves);
                }
            }
        }
        blocks
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

fn first_resolved_block(registry: &BlockRegistry, ids: &[Identifier]) -> Option<BlockStateId> {
    ids.iter()
        .find_map(|id| registry.block(id).map(|block| block.default))
}

fn first_resolved_block_with_suffix(
    registry: &BlockRegistry,
    ids: &[Identifier],
    suffix: &str,
) -> Option<BlockStateId> {
    ids.iter()
        .filter(|id| id.path().ends_with(suffix))
        .find_map(|id| registry.block(id).map(|block| block.default))
}

fn first_resolved_generated_leaves(
    registry: &BlockRegistry,
    ids: &[Identifier],
) -> Option<BlockStateId> {
    ids.iter()
        .filter(|id| id.path().ends_with("_leaves"))
        .find_map(|id| generated_leaf_state(registry, id))
}

fn resolve_blocks(registry: &BlockRegistry, ids: &[Identifier]) -> Vec<BlockStateId> {
    ids.iter()
        .filter_map(|id| registry.block(id).map(|block| block.default))
        .collect()
}

fn decoration_spacing_from_count(count: FeatureCount, fallback: u64) -> u64 {
    let count = match count {
        FeatureCount::Constant(count) => count,
        FeatureCount::Uniform { min, max } => min.saturating_add(max) / 2,
    };
    if count == 0 {
        fallback
    } else {
        (256 / count).clamp(3, 29) as u64
    }
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
        Self::with_biome_rules(seed, registry, BiomeRules::vanilla_overworld())
    }

    /// Build a generator with explicit biome rules.
    ///
    /// # Panics
    ///
    /// Panics if the registry is missing required vanilla terrain blocks. Use
    /// [`TerrainGenerator::try_with_biome_rules`] for fallible startup validation.
    #[must_use]
    pub fn with_biome_rules(seed: i64, registry: Arc<BlockRegistry>, biomes: BiomeRules) -> Self {
        Self::try_with_biome_rules(seed, registry, biomes).unwrap_or_else(|err| panic!("{err}"))
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
            iron_ore: try_resolve_block(registry.as_ref(), "minecraft:iron_ore")?,
            water: resolve_block_or(registry.as_ref(), "minecraft:water", air),
            biomes,
            ores,
            structures: StructureRules::none(),
            decorations: DecorationBlocks::new(registry.as_ref()),
            worldgen_mode: WorldgenMode::VanillaLike,
            registry,
        })
    }

    #[must_use]
    pub fn with_structures(mut self, structures: StructureRules) -> Self {
        self.structures = structures;
        self
    }

    #[must_use]
    pub fn with_geometry(mut self, geometry: ChunkGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    #[must_use]
    pub fn with_feature_facts(mut self, features: &[WorldgenFeatureFacts]) -> Self {
        self.decorations = DecorationBlocks::from_feature_facts(self.registry.as_ref(), features);
        self
    }

    /// Sample the terrain height for an absolute world `(x, z)`.
    /// Public so tests + spawn-position picking can use the same
    /// function the generator does.
    #[must_use]
    pub fn surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        match self.worldgen_mode {
            WorldgenMode::VanillaLike => self.vanilla_surface_height(world_x, world_z),
            WorldgenMode::TellusLike(settings) => {
                self.tellus_surface_height(world_x, world_z, settings)
            }
        }
    }

    fn clamp_surface_height(&self, raw: f64) -> i32 {
        let min = self.geometry.min_y() + 2;
        let max = (self.geometry.max_y() - 2).min(250).max(min);
        raw.round().clamp(min as f64, max as f64) as i32
    }

    fn vanilla_surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        let hills = fbm_2d(
            world_x as f64 * NOISE_FREQUENCY,
            world_z as f64 * NOISE_FREQUENCY,
            self.seed,
            NOISE_OCTAVES,
            NOISE_PERSISTENCE,
        );
        let continental = self.continentalness(world_x, world_z);
        let river = self.river_signal(world_x, world_z);
        let depth = (OCEAN_THRESHOLD - COAST_BLEND_WIDTH - continental).max(0.0) * 40.0;
        let ocean = SEA_LEVEL as f64 - 5.0 - depth + hills * 4.0;
        let uplift = continental.max(0.0) * 20.0;
        let river_blend = river_valley_blend(river);
        let upland =
            BASE_HEIGHT + uplift + hills * HEIGHT_AMPLITUDE + self.ridges(world_x, world_z) * 18.0;
        let river_floor = SEA_LEVEL as f64 - 4.0 + hills.abs() * 2.0;
        let land = upland * (1.0 - river_blend) + river_floor * river_blend;
        let coast_t = ((continental - (OCEAN_THRESHOLD - COAST_BLEND_WIDTH))
            / (COAST_BLEND_WIDTH * 2.0))
            .clamp(0.0, 1.0);
        let smooth = smoothstep01(coast_t);
        let raw = ocean * (1.0 - smooth) + land * smooth;
        // Guard against extreme outputs even though fbm_2d is bounded.
        self.clamp_surface_height(raw)
    }

    fn tellus_surface_height(
        &self,
        world_x: i32,
        world_z: i32,
        settings: TellusWorldgenSettings,
    ) -> i32 {
        let projection = MercatorProjection::from_settings(settings);
        let (latitude, _) = projection.block_to_lat_lon(world_x as f64, world_z as f64);
        let equator_weight = (1.0 - latitude.abs() / MAX_MERCATOR_LATITUDE).clamp(0.0, 1.0);
        let land_mask = self.tellus_land_mask(world_x, world_z, settings);
        let ridges = self.ridges(world_x / 8, world_z / 8);
        let hills = fbm_2d(
            world_x as f64 / TELLUS_HILL_SCALE,
            world_z as f64 / TELLUS_HILL_SCALE,
            self.seed ^ 0x454C_4556,
            4,
            0.52,
        );
        let mountain = self.tellus_mountain_factor(world_x, world_z);
        let shore = ((land_mask + 0.14) / 0.42).clamp(0.0, 1.0);
        let shore = shore * shore * (3.0 - 2.0 * shore);
        let equatorial_uplift = (equator_weight - 0.5).max(0.0) * 4.0;
        let terrestrial = settings.sea_level as f64
            + 5.0
            + equatorial_uplift
            + (land_mask.max(0.0) * 38.0
                + ridges * 14.0
                + hills * 8.0
                + mountain * (270.0 + ridges * 180.0))
                * settings.terrestrial_height_scale.max(0.0);
        let oceanic = settings.sea_level as f64
            - 7.0
            - ((-land_mask).max(0.0) * 42.0 + hills.abs() * 4.0)
                * settings.oceanic_height_scale.max(0.0);
        let raw = oceanic * (1.0 - shore) + terrestrial * shore;
        self.clamp_surface_height(raw)
    }

    fn tellus_land_mask(
        &self,
        world_x: i32,
        world_z: i32,
        settings: TellusWorldgenSettings,
    ) -> f64 {
        let projection = MercatorProjection::from_settings(settings);
        let (latitude, _) = projection.block_to_lat_lon(world_x as f64, world_z as f64);
        let equator_weight = (1.0 - latitude.abs() / MAX_MERCATOR_LATITUDE).clamp(0.0, 1.0);
        let continent = fbm_2d(
            world_x as f64 / TELLUS_CONTINENT_SCALE,
            world_z as f64 / TELLUS_CONTINENT_SCALE,
            self.seed ^ 0x5445_4C4C_5553,
            5,
            0.55,
        );
        let coast = fbm_2d(
            world_x as f64 / TELLUS_COAST_SCALE,
            world_z as f64 / TELLUS_COAST_SCALE,
            self.seed ^ 0x434F_4153_5453,
            3,
            0.5,
        );
        continent + coast * 0.22 + (equator_weight - 0.5) * 0.08
    }

    #[must_use]
    pub fn tellus_climate(&self, world_x: i32, height: i32, world_z: i32) -> TellusClimate {
        let settings = match self.worldgen_mode {
            WorldgenMode::VanillaLike => TellusWorldgenSettings::default(),
            WorldgenMode::TellusLike(settings) => settings,
        };
        let projection = MercatorProjection::from_settings(settings);
        let (latitude_degrees, _) = projection.block_to_lat_lon(world_x as f64, world_z as f64);
        let latitude_cooling = latitude_degrees.abs() / MAX_MERCATOR_LATITUDE;
        let altitude_cooling = ((height - settings.sea_level).max(0) as f64 / 128.0).min(1.0);
        let weather = fbm_2d(
            world_x as f64 / TELLUS_CLIMATE_SCALE,
            world_z as f64 / TELLUS_CLIMATE_SCALE,
            self.seed ^ 0x434C_494D_4154,
            3,
            0.55,
        );
        TellusClimate {
            latitude_degrees,
            temperature: ((1.0 - latitude_cooling * 1.8 - altitude_cooling * 0.85)
                * settings.climate_strength)
                + weather * 0.12,
            moisture: fbm_2d(
                world_x as f64 / TELLUS_MOISTURE_SCALE,
                world_z as f64 / TELLUS_MOISTURE_SCALE,
                self.seed ^ 0x4D4F_4953_5455,
                3,
                0.55,
            ),
        }
    }

    fn continentalness(&self, world_x: i32, world_z: i32) -> f64 {
        let broad = fbm_2d(
            world_x as f64 * CONTINENT_FREQUENCY,
            world_z as f64 * CONTINENT_FREQUENCY,
            self.seed ^ 0x0043_4F41_5354,
            4,
            0.52,
        );
        let coast = fbm_2d(
            world_x as f64 * COAST_DETAIL_FREQUENCY,
            world_z as f64 * COAST_DETAIL_FREQUENCY,
            self.seed ^ 0x0053_484F_5245,
            2,
            0.5,
        );
        broad + coast * 0.28
    }

    fn biome_for(&self, world_x: i32, world_z: i32, height: i32) -> Identifier {
        match self.worldgen_mode {
            WorldgenMode::VanillaLike => self.vanilla_biome_for(world_x, world_z, height),
            WorldgenMode::TellusLike(settings) => {
                self.tellus_biome_for(world_x, world_z, height, settings)
            }
        }
    }

    fn vanilla_biome_for(&self, world_x: i32, world_z: i32, height: i32) -> Identifier {
        let continental = self.continentalness(world_x, world_z);
        let temperature = self.temperature(world_x, world_z);
        let moisture = self.moisture(world_x, world_z);
        let ridges = self.ridges(world_x, world_z);
        let river = self.river_signal(world_x, world_z);

        if height < SEA_LEVEL - 14 {
            return self
                .biomes
                .pick(&self.biomes.deep_ocean, world_x, world_z, 0x4445_4550);
        }
        if river.abs() < RIVER_BIOME_WIDTH && continental > -0.08 {
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
        if height > 118 || ridges > 0.55 {
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
    ) -> Identifier {
        let sea_level = settings.sea_level;
        let land_mask = self.tellus_land_mask(world_x, world_z, settings);
        let climate = self.tellus_climate(world_x, height, world_z);
        let mountain = self.tellus_mountain_factor(world_x, world_z);
        let river = self.river_signal(world_x / 2, world_z / 2);

        if settings.water_enabled {
            if height < sea_level - 18 {
                return self
                    .biomes
                    .pick(&self.biomes.deep_ocean, world_x, world_z, 0x5444_4545);
            }
            if height < sea_level - 1 {
                return self
                    .biomes
                    .pick(&self.biomes.ocean, world_x, world_z, 0x544F_434E);
            }
        }
        if land_mask.abs() < 0.08 || height <= sea_level + BEACH_HEIGHT_ABOVE_SEA {
            return self
                .biomes
                .pick(&self.biomes.beach, world_x, world_z, 0x5442_4541);
        }
        if settings.water_enabled && river.abs() < RIVER_BIOME_WIDTH * 0.65 && land_mask > -0.02 {
            return self
                .biomes
                .pick(&self.biomes.river, world_x, world_z, 0x5452_4956);
        }
        if height > sea_level + 86 || mountain > 0.22 {
            return self
                .biomes
                .pick(&self.biomes.mountain, world_x, world_z, 0x544D_4F55);
        }
        if height < 18 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, 0x5443_4156);
        }
        if climate.moisture > 0.62 && height <= sea_level + 8 {
            return self
                .biomes
                .pick(&self.biomes.swamp, world_x, world_z, 0x5453_5741);
        }
        if climate.temperature < -0.25 {
            return self
                .biomes
                .pick(&self.biomes.cold, world_x, world_z, 0x5443_4F4C);
        }
        if climate.temperature > 0.38 && climate.moisture < -0.08 {
            return self
                .biomes
                .pick(&self.biomes.hot_dry, world_x, world_z, 0x5448_4F54);
        }
        if climate.temperature > 0.22 && climate.moisture > 0.2 {
            return self
                .biomes
                .pick(&self.biomes.jungle, world_x, world_z, 0x544A_554E);
        }
        if climate.moisture > 0.04 {
            self.biomes
                .pick(&self.biomes.temperate_forest, world_x, world_z, 0x5446_4F52)
        } else {
            self.biomes
                .pick(&self.biomes.grassland, world_x, world_z, 0x5447_5241)
        }
    }

    fn biome_for_cell(
        &self,
        world_x: i32,
        y: i32,
        world_z: i32,
        surface_height: i32,
    ) -> Identifier {
        if y < surface_height - 24 && y < 32 {
            return self
                .biomes
                .pick(&self.biomes.cave, world_x, world_z, 0x554E_4447);
        }
        self.biome_for(world_x, world_z, surface_height)
    }

    fn moisture(&self, world_x: i32, world_z: i32) -> f64 {
        fbm_2d(
            world_x as f64 * FOREST_FREQUENCY,
            world_z as f64 * FOREST_FREQUENCY,
            self.seed ^ 0x464F_5245_5354,
            3,
            0.55,
        )
    }

    fn temperature(&self, world_x: i32, world_z: i32) -> f64 {
        fbm_2d(
            world_x as f64 / TEMPERATURE_SCALE,
            world_z as f64 / TEMPERATURE_SCALE,
            self.seed ^ 0x5445_4D50,
            3,
            0.55,
        )
    }

    fn ridges(&self, world_x: i32, world_z: i32) -> f64 {
        fbm_2d(
            world_x as f64 / 180.0,
            world_z as f64 / 180.0,
            self.seed ^ 0x5249_4447,
            4,
            0.5,
        )
        .abs()
    }

    fn tellus_mountain_factor(&self, world_x: i32, world_z: i32) -> f64 {
        let mask = fbm_2d(
            world_x as f64 / TELLUS_MOUNTAIN_MASK_SCALE,
            world_z as f64 / TELLUS_MOUNTAIN_MASK_SCALE,
            self.seed ^ 0x544D_4F55_4E54,
            4,
            0.56,
        );
        let mask = ((mask - 0.32) / 0.30).clamp(0.0, 1.0);
        let mask = mask * mask * (3.0 - 2.0 * mask);
        let ridge = self.ridges(world_x / 10, world_z / 10).powf(1.35);
        let detail = fbm_2d(
            world_x as f64 / TELLUS_MOUNTAIN_DETAIL_SCALE,
            world_z as f64 / TELLUS_MOUNTAIN_DETAIL_SCALE,
            self.seed ^ 0x544D_4153_5349,
            3,
            0.5,
        )
        .max(0.0);
        (mask * (ridge * 0.78 + detail * 0.22)).clamp(0.0, 1.0)
    }

    fn river_signal(&self, world_x: i32, world_z: i32) -> f64 {
        fbm_2d(
            world_x as f64 / RIVER_SIGNAL_SCALE,
            world_z as f64 / RIVER_SIGNAL_SCALE,
            self.seed ^ 0x5249_5645_5200,
            2,
            0.5,
        )
    }

    fn plan_column(&self, pos: ChunkPos, lx: u8, lz: u8) -> ColumnPlan {
        let wx = pos.x * 16 + lx as i32;
        let wz = pos.z * 16 + lz as i32;
        let height = self.surface_height(wx, wz);
        let biome = self.biome_for(wx, wz, height);
        let (mut surface, fill) = self.surface_materials(&biome);
        if self.is_spawn_iron_outcrop(wx, height, wz) {
            surface = self.iron_ore;
        } else if self.is_spawn_stone_outcrop(wx, height, wz) {
            surface = self.stone;
        }
        let (sea_level, water_enabled) = match self.worldgen_mode {
            WorldgenMode::VanillaLike => (SEA_LEVEL, true),
            WorldgenMode::TellusLike(settings) => (settings.sea_level, settings.water_enabled),
        };
        let top_non_air = if water_enabled && (height < sea_level || self.biomes.is_river(&biome)) {
            sea_level.clamp(height, self.geometry.max_y() - 1)
        } else {
            height
        };
        ColumnPlan {
            lx,
            lz,
            wx,
            wz,
            height,
            top_non_air,
            dirt_start: (height - DIRT_DEPTH).max(self.geometry.min_y() + 1),
            biome,
            surface,
            fill,
            hash: feature_hash(self.seed, wx, height, wz, 0xDEC0_0001),
        }
    }

    fn is_spawn_stone_outcrop(&self, wx: i32, height: i32, wz: i32) -> bool {
        height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA
            && (8..=11).contains(&wx)
            && (4..=8).contains(&wz)
    }

    fn is_spawn_iron_outcrop(&self, wx: i32, height: i32, wz: i32) -> bool {
        height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA
            && (12..=13).contains(&wx)
            && (4..=8).contains(&wz)
    }

    fn fill_column(&self, chunk: &mut Chunk, plan: &ColumnPlan) {
        let min_y = self.geometry.min_y();
        let _ = chunk.set_block(plan.lx, min_y, plan.lz, self.bedrock);
        for y in (min_y + 1)..plan.dirt_start {
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
            for y in (plan.height + 1)..=plan.top_non_air {
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
                        let y = self.geometry.min_y() + section_idx as i32 * 16 + cy as i32 * 4 + 2;
                        let biome = self.biome_for_cell(column.wx, y, column.wz, column.height);
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

    fn apply_features(&self, chunk: &mut Chunk, plan: &ColumnPlan) {
        if plan.dirt_start <= self.geometry.min_y() + 1 {
            return;
        }
        let cave_mouth_depth = self.surface_cave_mouth_depth_for_plan(plan);
        self.apply_caves(chunk, plan, cave_mouth_depth);
        self.apply_surface_cave_mouth(chunk, plan, cave_mouth_depth);
    }

    fn apply_caves(&self, chunk: &mut Chunk, plan: &ColumnPlan, cave_mouth_depth: i32) {
        let surface_clearance = CAVE_SURFACE_CLEARANCE - cave_mouth_depth;
        let cave_max_y = (plan.height - surface_clearance).min(plan.dirt_start - 1);
        let cave_min_y = self.geometry.min_y() + 8;
        if cave_max_y < cave_min_y {
            return;
        }
        let mut y = cave_min_y;
        while y <= cave_max_y {
            if self.is_cave_cell(plan.wx, y, plan.wz) {
                let end = (y + 1).min(cave_max_y);
                for carve_y in y..=end {
                    let _ = chunk.set_block(plan.lx, carve_y, plan.lz, self.air);
                }
            }
            y += 2;
        }
    }

    fn apply_surface_cave_mouth(
        &self,
        chunk: &mut Chunk,
        plan: &ColumnPlan,
        cave_mouth_depth: i32,
    ) {
        if cave_mouth_depth == 0 {
            return;
        }
        let floor = (plan.height - cave_mouth_depth).max(self.geometry.min_y() + 8);
        for y in (floor + 1)..=plan.height {
            let _ = chunk.set_block(plan.lx, y, plan.lz, self.air);
        }
    }

    fn surface_cave_mouth_depth_for_plan(&self, plan: &ColumnPlan) -> i32 {
        if plan.top_non_air != plan.height || self.biomes.is_surface_water(&plan.biome) {
            0
        } else {
            self.surface_cave_mouth_depth(plan.wx, plan.wz)
        }
    }

    fn apply_ores(&self, chunk: &mut Chunk) {
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
            let min_y = rule.y.min.max(self.geometry.min_y() + 1);
            let max_y = rule.y.max.min(self.geometry.max_y() - 1);
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
                let sample_y = cell_y * ORE_ANCHOR_CELL_EDGE + ORE_ANCHOR_CELL_EDGE / 2;
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
                            let surface_y = self.surface_height(anchor_x, anchor_z);
                            let biome = self.biome_for(anchor_x, anchor_z, surface_y);
                            if !rule.biomes.matches(&biome) {
                                continue;
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
            |offset, cell_hash| self.ore_cell_can_generate(rule, anchor, offset, cell_hash),
        );
        for &offset in &offsets[..offset_count] {
            self.place_ore_cell(chunk, rule, anchor, offset);
        }
    }

    fn ore_cell_can_generate(
        &self,
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
        let Some(base) = self.generated_cell_before_ores(world_x, world_y, world_z) else {
            return false;
        };
        if base != self.stone && base != self.deepslate {
            return false;
        }
        !self.should_discard_exposed_ore(world_x, world_y, world_z, rule, cell_hash)
    }

    fn place_ore_cell(&self, chunk: &mut Chunk, rule: &OreRule, anchor: [i32; 3], offset: [i8; 3]) {
        let world_x = i64::from(anchor[0]) + i64::from(offset[0]);
        let world_y = anchor[1] + i32::from(offset[1]);
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
        world_x: i32,
        y: i32,
        world_z: i32,
        rule: &OreRule,
        hash: u64,
    ) -> bool {
        let chance = rule.discard_chance_on_air_exposure;
        if !chance.is_finite()
            || chance <= 0.0
            || !self.generated_cell_touches_air(world_x, y, world_z)
        {
            return false;
        }
        if chance >= 1.0 {
            return true;
        }
        let sample = (hash >> 11) as f64 * (1.0 / ((1_u64 << 53) as f64));
        sample < chance
    }

    fn generated_cell_touches_air(&self, world_x: i32, y: i32, world_z: i32) -> bool {
        ORE_DIRECTIONS.iter().any(|direction| {
            let neighbour = world_x
                .checked_add(i32::from(direction[0]))
                .zip(y.checked_add(i32::from(direction[1])))
                .zip(world_z.checked_add(i32::from(direction[2])));
            let Some(((nx, ny), nz)) = neighbour else {
                return true;
            };
            self.generated_cell_before_ores(nx, ny, nz) == Some(self.air)
        })
    }

    fn generated_cell_before_ores(
        &self,
        world_x: i32,
        y: i32,
        world_z: i32,
    ) -> Option<BlockStateId> {
        if y < self.geometry.min_y() || y >= self.geometry.max_y() {
            return None;
        }
        let pos = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        };
        let plan = self.plan_column(
            pos,
            world_x.rem_euclid(16) as u8,
            world_z.rem_euclid(16) as u8,
        );
        if y > plan.top_non_air {
            return Some(self.air);
        }
        if y > plan.height {
            return Some(self.water);
        }
        if y == self.geometry.min_y() {
            return Some(self.bedrock);
        }

        let cave_mouth_depth = self.surface_cave_mouth_depth_for_plan(&plan);
        if cave_mouth_depth != 0 {
            let floor = (plan.height - cave_mouth_depth).max(self.geometry.min_y() + 8);
            if y > floor {
                return Some(self.air);
            }
        }
        let cave_min_y = self.geometry.min_y() + 8;
        let surface_clearance = CAVE_SURFACE_CLEARANCE - cave_mouth_depth;
        let cave_max_y = (plan.height - surface_clearance).min(plan.dirt_start - 1);
        if (cave_min_y..=cave_max_y).contains(&y) {
            let sample_y = cave_min_y + (y - cave_min_y).div_euclid(2) * 2;
            if self.is_cave_cell(world_x, sample_y, world_z) {
                return Some(self.air);
            }
        }
        if y < plan.dirt_start {
            return Some(self.base_stone_for_y(
                world_x.rem_euclid(16) as u8,
                y,
                world_z.rem_euclid(16) as u8,
                pos,
            ));
        }
        if y < plan.height {
            return Some(plan.fill);
        }
        Some(plan.surface)
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
                if height <= self.geometry.min_y() || height + 8 >= self.geometry.max_y() {
                    continue;
                }
                let biome = &plan.biome;
                let surface = plan.surface;
                let h = plan.hash;

                if self.is_spawn_stone_outcrop(plan.wx, height, plan.wz) {
                    continue;
                }
                if (self.biomes.temperate_forest.contains(biome)
                    || self.biomes.cold.contains(biome)
                    || self.biomes.jungle.contains(biome))
                    && h.is_multiple_of(83)
                    && self.place_tree(
                        chunk,
                        lx,
                        height + 1,
                        lz,
                        self.tree_blocks_for_biome(biome),
                        &mut touched,
                    )
                {
                    continue;
                }
                if self.biomes.hot_dry.contains(biome)
                    && surface == self.sand
                    && h.is_multiple_of(47)
                    && self.place_cactus(chunk, lx, height + 1, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.beach.contains(biome) || self.biomes.river.contains(biome))
                    && h.is_multiple_of(29)
                    && self.place_sugar_cane(chunk, lx, height + 1, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.grassland.contains(biome)
                    || self.biomes.temperate_forest.contains(biome)
                    || self.biomes.jungle.contains(biome))
                    && (surface == self.grass_block || surface == self.podzol)
                {
                    let plant = if h.is_multiple_of(97) {
                        self.decorations.pumpkin
                    } else if h.is_multiple_of(37) {
                        self.decorations.dandelion
                    } else if h.is_multiple_of(41) {
                        self.decorations.poppy
                    } else if h.is_multiple_of(self.decorations.grass_patch_spacing) {
                        self.decorations.short_grass
                    } else {
                        None
                    };
                    if let Some(plant) = plant {
                        self.place_single(chunk, lx, height + 1, lz, plant, &mut touched);
                    }
                }
                if self.biomes.is_ocean(biome) && plan.top_non_air > height + 2 {
                    if h.is_multiple_of(31) && self.place_kelp_column(chunk, plan, h, &mut touched)
                    {
                        continue;
                    }
                    if h.is_multiple_of(17)
                        && let Some(seagrass) = self.decorations.seagrass
                        && chunk.get_block(lx, height + 1, lz) == Some(self.water)
                    {
                        self.place_single(chunk, lx, height + 1, lz, seagrass, &mut touched);
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

    fn place_tree(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        blocks: (Option<BlockStateId>, Option<BlockStateId>),
        touched: &mut [Option<i32>; 256],
    ) -> bool {
        let (Some(log), Some(leaves)) = blocks else {
            return false;
        };
        if !(2..=13).contains(&lx) || !(2..=13).contains(&lz) || base_y + 5 >= self.geometry.max_y()
        {
            return false;
        }
        for y in base_y..=(base_y + 5) {
            if chunk.get_block(lx, y, lz) != Some(self.air) {
                return false;
            }
        }
        for y in base_y..=(base_y + 3) {
            self.place_single(chunk, lx, y, lz, log, touched);
        }
        for dz in -2i8..=2 {
            for dx in -2i8..=2 {
                let distance = dx.unsigned_abs() + dz.unsigned_abs();
                if distance > 3 {
                    continue;
                }
                let x = lx.wrapping_add_signed(dx);
                let z = lz.wrapping_add_signed(dz);
                for y in (base_y + 3)..=(base_y + 5) {
                    if chunk.get_block(x, y, z) == Some(self.air) {
                        self.place_single(chunk, x, y, z, leaves, touched);
                    }
                }
            }
        }
        true
    }

    fn tree_blocks_for_biome(
        &self,
        biome: &Identifier,
    ) -> (Option<BlockStateId>, Option<BlockStateId>) {
        if self.biomes.jungle.contains(biome) {
            (self.decorations.jungle_log, self.decorations.jungle_leaves)
        } else if self.biomes.cold.contains(biome) || biome.path().contains("taiga") {
            (self.decorations.cold_log, self.decorations.cold_leaves)
        } else if self.biomes.temperate_forest.contains(biome) {
            (self.decorations.forest_log, self.decorations.forest_leaves)
        } else {
            (self.decorations.oak_log, self.decorations.oak_leaves)
        }
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
        for y in base_y..(base_y + height) {
            if chunk.get_block(lx, y, lz) != Some(self.air) {
                return false;
            }
        }
        for y in base_y..(base_y + height) {
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
        if chunk.get_block(lx, base_y, lz) != Some(self.air)
            || !self.has_adjacent_water(chunk, lx, base_y - 1, lz)
        {
            return false;
        }
        self.place_single(chunk, lx, base_y, lz, sugar_cane, touched);
        if chunk.get_block(lx, base_y + 1, lz) == Some(self.air) {
            self.place_single(chunk, lx, base_y + 1, lz, sugar_cane, touched);
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
                    || chunk.get_block(x, y + 1, z) == Some(self.water)
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
        let base_y = plan.height + 1;
        let water_top = plan.top_non_air;
        let Some(kelp) = self.decorations.kelp else {
            return false;
        };
        let stem = self.decorations.kelp_plant.unwrap_or(kelp);
        let available = water_top - base_y + 1;
        if available < 2 {
            return false;
        }
        let height = (2 + (hash % 5) as i32).min(available);
        let top_y = base_y + height - 1;
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
            .map(|heightmap| heightmap.get(lx, lz) as i32 + self.geometry.min_y() - 1)
            .unwrap_or(self.geometry.min_y());
        let value = (top.max(current) + 1 - self.geometry.min_y()) as u32;
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
        let origin_y = center_height + 1;
        let origin_z = center_z - size[2] / 2;
        paste_template(chunk, template, origin_x, origin_y, origin_z, touched);
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
        let center_chunk_x = grid_x * spacing + x_offset;
        let center_chunk_z = grid_z * spacing + z_offset;
        Some((template, center_chunk_x * 16 + 8, center_chunk_z * 16 + 8))
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
        let value = (top + 1 - self.geometry.min_y()) as u32;
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
        let branch = fbm_2d(
            (x as f64 + y as f64 * 0.41) * CAVE_BRANCH_FREQUENCY,
            z as f64 * CAVE_BRANCH_FREQUENCY,
            self.seed ^ 0x4252_414E_4348,
            2,
            0.5,
        );
        let room = feature_hash(
            self.seed,
            x.div_euclid(4),
            y.div_euclid(3),
            z.div_euclid(4),
            0xC4A7,
        )
        .is_multiple_of(211);
        n > CAVE_THRESHOLD || branch > CAVE_BRANCH_THRESHOLD || room
    }

    fn surface_cave_mouth_depth(&self, x: i32, z: i32) -> i32 {
        let grid_x = x.div_euclid(CAVE_MOUTH_GRID);
        let grid_z = z.div_euclid(CAVE_MOUTH_GRID);
        let hash = feature_hash(self.seed, grid_x, 0, grid_z, 0xC4A7_E001);
        let offset_span = CAVE_MOUTH_GRID - CAVE_MOUTH_RADIUS * 2;
        let center_x = grid_x * CAVE_MOUTH_GRID
            + CAVE_MOUTH_RADIUS
            + i32::try_from(hash % offset_span as u64).expect("cave mouth x offset fits i32");
        let center_z = grid_z * CAVE_MOUTH_GRID
            + CAVE_MOUTH_RADIUS
            + i32::try_from((hash >> 16) % offset_span as u64)
                .expect("cave mouth z offset fits i32");
        let spawn_clearance = CAVE_MOUTH_SPAWN_SAFE_RADIUS + CAVE_MOUTH_RADIUS;
        let center_distance_squared =
            i64::from(center_x) * i64::from(center_x) + i64::from(center_z) * i64::from(center_z);
        if center_distance_squared <= i64::from(spawn_clearance) * i64::from(spawn_clearance) {
            return 0;
        }
        let dx = i64::from(x - center_x);
        let dz = i64::from(z - center_z);
        let distance = ((dx * dx + dz * dz) as f64).sqrt().ceil() as i32;
        (CAVE_MOUTH_RADIUS - distance).max(0)
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

fn smoothstep01(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn river_valley_blend(signal: f64) -> f64 {
    let distance = signal.abs();
    if distance <= RIVER_TERRAIN_CORE_WIDTH {
        1.0
    } else if distance < RIVER_TERRAIN_WIDTH {
        let bank =
            (RIVER_TERRAIN_WIDTH - distance) / (RIVER_TERRAIN_WIDTH - RIVER_TERRAIN_CORE_WIDTH);
        smoothstep01(bank)
    } else {
        0.0
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
    let min_x = chunk.pos.x * 16;
    let min_z = chunk.pos.z * 16;
    for block in template.blocks() {
        let wx = origin_x + block.pos[0];
        let wy = origin_y + block.pos[1];
        let wz = origin_z + block.pos[2];
        if wx < min_x || wx >= min_x + 16 || wz < min_z || wz >= min_z + 16 {
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
        let y = origin_y + template_chest.pos[1];
        let z = origin_z + template_chest.pos[2];
        if x < min_x || x >= min_x + 16 || z < min_z || z >= min_z + 16 {
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
            self.apply_features(&mut chunk, plan);
            // Heightmap value: Y of the first air cell above the
            // Heightmaps store offsets from this dimension's minimum Y.
            let world_surface = (plan.top_non_air + 1 - self.geometry.min_y()) as u32;
            let motion_blocking = (plan.height + 1 - self.geometry.min_y()) as u32;
            if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                mb.set(plan.lx, plan.lz, motion_blocking);
            }
            if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                ws.set(plan.lx, plan.lz, world_surface);
            }
            chunk.highest_opaque.set(plan.lx, plan.lz, motion_blocking);
        }
        self.apply_ores(&mut chunk);
        self.assign_biomes(&mut chunk, &columns);
        self.apply_decorations(&mut chunk, &columns);
        self.apply_structures(&mut chunk);
        chunk.status = "minecraft:full".into();
        chunk.mark_dirty();
        chunk
    }
}

#[cfg(test)]
#[path = "terrain/tests.rs"]
mod tests;
