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
use mc_data::biomes::BiomeWorldgenData;
use mc_data::worldgen_features::{FeatureCount, WorldgenFeatureFacts};
use mc_data::worldgen_ores::{HeightAnchor, OreFeature, OrePlacementCount, OreTarget};
use mc_world::chunk::{Chunk, ChunkPos, Heightmap, MAX_Y, MIN_Y};
use mc_world::{
    BIOME_DIM, BIOME_VOLUME, BiomeSection, BlockRegistry, BlockStateId, ChunkGenerator,
    PackedBitArray,
};

use crate::noise::fbm_2d;
use crate::structures::{StructureRules, StructureTemplate};

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
pub const SEA_LEVEL: i32 = 63;
const CONTINENT_FREQUENCY: f64 = 1.0 / 420.0;
const COAST_DETAIL_FREQUENCY: f64 = 1.0 / 96.0;
const FOREST_FREQUENCY: f64 = 1.0 / 160.0;
const OCEAN_THRESHOLD: f64 = -0.16;
const BEACH_HEIGHT_ABOVE_SEA: i32 = 2;
/// Number of dirt cells between grass cap and stone.
const DIRT_DEPTH: i32 = 3;
const CAVE_MIN_Y: i32 = MIN_Y + 8;
const CAVE_SURFACE_CLEARANCE: i32 = 24;
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
const GOLD_MIN_Y: i32 = -64;
const GOLD_MAX_Y: i32 = 32;
const REDSTONE_MIN_Y: i32 = -64;
const REDSTONE_MAX_Y: i32 = 15;
const DIAMOND_MIN_Y: i32 = -64;
const DIAMOND_MAX_Y: i32 = 16;
const LAPIS_MIN_Y: i32 = -64;
const LAPIS_MAX_Y: i32 = 64;
const EMERALD_MIN_Y: i32 = -16;
const EMERALD_MAX_Y: i32 = 320;
const OVERWORLD_BIOME_IDS: &[&str] = &[
    "minecraft:mushroom_fields",
    "minecraft:deep_frozen_ocean",
    "minecraft:frozen_ocean",
    "minecraft:deep_cold_ocean",
    "minecraft:cold_ocean",
    "minecraft:deep_ocean",
    "minecraft:ocean",
    "minecraft:deep_lukewarm_ocean",
    "minecraft:lukewarm_ocean",
    "minecraft:warm_ocean",
    "minecraft:stony_shore",
    "minecraft:swamp",
    "minecraft:mangrove_swamp",
    "minecraft:snowy_slopes",
    "minecraft:snowy_plains",
    "minecraft:snowy_beach",
    "minecraft:windswept_gravelly_hills",
    "minecraft:grove",
    "minecraft:windswept_hills",
    "minecraft:snowy_taiga",
    "minecraft:windswept_forest",
    "minecraft:taiga",
    "minecraft:plains",
    "minecraft:meadow",
    "minecraft:beach",
    "minecraft:forest",
    "minecraft:old_growth_spruce_taiga",
    "minecraft:flower_forest",
    "minecraft:birch_forest",
    "minecraft:dark_forest",
    "minecraft:pale_garden",
    "minecraft:savanna_plateau",
    "minecraft:savanna",
    "minecraft:jungle",
    "minecraft:badlands",
    "minecraft:desert",
    "minecraft:wooded_badlands",
    "minecraft:jagged_peaks",
    "minecraft:stony_peaks",
    "minecraft:frozen_river",
    "minecraft:river",
    "minecraft:ice_spikes",
    "minecraft:old_growth_pine_taiga",
    "minecraft:sunflower_plains",
    "minecraft:old_growth_birch_forest",
    "minecraft:sparse_jungle",
    "minecraft:bamboo_jungle",
    "minecraft:eroded_badlands",
    "minecraft:windswept_savanna",
    "minecraft:cherry_grove",
    "minecraft:frozen_peaks",
    "minecraft:dripstone_caves",
    "minecraft:lush_caves",
    "minecraft:deep_dark",
];

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
    sand: BlockStateId,
    deepslate: BlockStateId,
    water: BlockStateId,
    biomes: BiomeRules,
    ores: OreRules,
    structures: StructureRules,
    decorations: DecorationBlocks,
    // Kept so the generator's lifetime is bounded by something
    // sensible if the storage drops the only other reference.
    #[allow(dead_code)]
    registry: Arc<BlockRegistry>,
}

#[derive(Clone)]
struct DecorationBlocks {
    oak_log: Option<BlockStateId>,
    oak_leaves: Option<BlockStateId>,
    short_grass: Option<BlockStateId>,
    dandelion: Option<BlockStateId>,
    poppy: Option<BlockStateId>,
    flower_patch: Vec<BlockStateId>,
    grass_patch_spacing: u64,
    pumpkin: Option<BlockStateId>,
    sugar_cane: Option<BlockStateId>,
    cactus: Option<BlockStateId>,
}

impl DecorationBlocks {
    fn new(registry: &BlockRegistry) -> Self {
        Self {
            oak_log: optional_block(registry, "minecraft:oak_log"),
            oak_leaves: optional_block(registry, "minecraft:oak_leaves"),
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
                _ => {}
            }
        }
        blocks
    }
}

#[derive(Clone)]
pub struct OreRules {
    rules: Vec<OreRule>,
}

impl OreRules {
    #[must_use]
    pub fn new(rules: Vec<OreRule>) -> Self {
        Self { rules }
    }

    #[must_use]
    pub fn solaris_default(
        registry: &BlockRegistry,
        biomes: &BiomeRules,
        fallback: BlockStateId,
    ) -> Self {
        let state = |name: &str| resolve_block_or(registry, name, fallback);
        Self::new(vec![
            OreRule {
                normal: state("minecraft:emerald_ore"),
                deepslate: state("minecraft:deepslate_emerald_ore"),
                y: YRange::new(EMERALD_MIN_Y, EMERALD_MAX_Y),
                spacing: OreSpacing::peaked(224, 130, 260),
                biomes: BiomeScope::only(biomes.mountain.clone()),
            },
            OreRule {
                normal: state("minecraft:gold_ore"),
                deepslate: state("minecraft:deepslate_gold_ore"),
                y: YRange::new(GOLD_MIN_Y, 112),
                spacing: OreSpacing::Fixed(58),
                biomes: BiomeScope::only(biomes.hot_dry.clone()),
            },
            OreRule {
                normal: state("minecraft:diamond_ore"),
                deepslate: state("minecraft:deepslate_diamond_ore"),
                y: YRange::new(DIAMOND_MIN_Y, DIAMOND_MAX_Y),
                spacing: OreSpacing::peaked(-56, 210, 380),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:redstone_ore"),
                deepslate: state("minecraft:deepslate_redstone_ore"),
                y: YRange::new(REDSTONE_MIN_Y, REDSTONE_MAX_Y),
                spacing: OreSpacing::peaked(-48, 95, 115),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:lapis_ore"),
                deepslate: state("minecraft:deepslate_lapis_ore"),
                y: YRange::new(LAPIS_MIN_Y, LAPIS_MAX_Y),
                spacing: OreSpacing::peaked(0, 150, 210),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:gold_ore"),
                deepslate: state("minecraft:deepslate_gold_ore"),
                y: YRange::new(GOLD_MIN_Y, GOLD_MAX_Y),
                spacing: OreSpacing::peaked(-16, 105, 160),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:iron_ore"),
                deepslate: state("minecraft:deepslate_iron_ore"),
                y: YRange::new(IRON_MIN_Y, IRON_MAX_Y),
                spacing: OreSpacing::peaked(16, 97, 140),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:copper_ore"),
                deepslate: state("minecraft:deepslate_copper_ore"),
                y: YRange::new(COPPER_MIN_Y, COPPER_MAX_Y),
                spacing: OreSpacing::peaked(48, 89, 130),
                biomes: BiomeScope::Any,
            },
            OreRule {
                normal: state("minecraft:coal_ore"),
                deepslate: state("minecraft:deepslate_coal_ore"),
                y: YRange::new(COAL_MIN_Y, COAL_MAX_Y),
                spacing: OreSpacing::peaked(96, 83, 120),
                biomes: BiomeScope::Any,
            },
        ])
    }

    #[must_use]
    pub fn from_features(
        registry: &BlockRegistry,
        biomes: &BiomeRules,
        features: &[OreFeature],
        biome_data: Option<&BiomeWorldgenData>,
    ) -> Option<Self> {
        let mut rules = Vec::new();
        for feature in features {
            let Some((normal, deepslate)) = ore_targets(registry, &feature.targets) else {
                continue;
            };
            let Some(height) = &feature.placement.height else {
                continue;
            };
            let min = height_anchor_y(height.min).clamp(MIN_Y, MAX_Y - 1);
            let max = height_anchor_y(height.max).clamp(MIN_Y, MAX_Y - 1);
            if min > max {
                continue;
            }
            let y = YRange::new(min, max);
            let spacing = ore_spacing(&feature.placement, y);
            let feature_biomes = biome_data
                .map(|data| data.biomes_for_feature(&feature.placed_feature))
                .unwrap_or_default();
            let biome_scope =
                if feature_biomes.is_empty() || feature_biomes.len() >= biomes.all.len() {
                    BiomeScope::Any
                } else {
                    BiomeScope::only(feature_biomes)
                };
            rules.push(OreRule {
                normal,
                deepslate,
                y,
                spacing,
                biomes: biome_scope,
            });
        }
        (!rules.is_empty()).then(|| Self::new(rules))
    }

    #[must_use]
    pub fn rules(&self) -> &[OreRule] {
        &self.rules
    }
}

#[derive(Clone)]
pub struct OreRule {
    pub normal: BlockStateId,
    pub deepslate: BlockStateId,
    pub y: YRange,
    pub spacing: OreSpacing,
    pub biomes: BiomeScope,
}

impl OreRule {
    fn matches(&self, h: u64, y: i32, biome: &Identifier) -> bool {
        self.y.contains(y)
            && self.biomes.matches(biome)
            && h.is_multiple_of(self.spacing.at_y(y, self.y))
    }
}

#[derive(Clone, Copy)]
pub struct YRange {
    pub min: i32,
    pub max: i32,
}

impl YRange {
    #[must_use]
    pub const fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }

    fn contains(&self, y: i32) -> bool {
        (self.min..=self.max).contains(&y)
    }
}

#[derive(Clone, Copy)]
pub enum OreSpacing {
    Fixed(u64),
    Peaked {
        peak_y: i32,
        min_spacing: u64,
        range: u64,
    },
}

impl OreSpacing {
    #[must_use]
    pub const fn peaked(peak_y: i32, min_spacing: u64, range: u64) -> Self {
        Self::Peaked {
            peak_y,
            min_spacing,
            range,
        }
    }

    fn at_y(&self, y: i32, range: YRange) -> u64 {
        match *self {
            Self::Fixed(spacing) => spacing,
            Self::Peaked {
                peak_y,
                min_spacing,
                range: spacing_range,
            } => peaked_spacing(y, range.min, range.max, peak_y, min_spacing, spacing_range),
        }
    }
}

#[derive(Clone)]
pub enum BiomeScope {
    Any,
    Only(Vec<Identifier>),
}

impl BiomeScope {
    #[must_use]
    pub fn only(biomes: Vec<Identifier>) -> Self {
        Self::Only(biomes)
    }

    fn matches(&self, biome: &Identifier) -> bool {
        match self {
            Self::Any => true,
            Self::Only(biomes) => biomes.contains(biome),
        }
    }
}

fn resolve_block(registry: &BlockRegistry, name: &str) -> BlockStateId {
    let id = Identifier::parse(name).expect("static identifier");
    registry
        .block(&id)
        .map(|b| b.default)
        .unwrap_or_else(|| panic!("registry missing required block {name}"))
}

fn resolve_block_or(registry: &BlockRegistry, name: &str, fallback: BlockStateId) -> BlockStateId {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default).unwrap_or(fallback)
}

fn optional_block(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    let id = Identifier::parse(name).expect("static identifier");
    registry.block(&id).map(|b| b.default)
}

fn first_resolved_block(registry: &BlockRegistry, ids: &[Identifier]) -> Option<BlockStateId> {
    ids.iter()
        .find_map(|id| registry.block(id).map(|block| block.default))
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

#[derive(Clone)]
pub struct BiomeRules {
    default: Identifier,
    all: Vec<Identifier>,
    deep_ocean: Vec<Identifier>,
    ocean: Vec<Identifier>,
    beach: Vec<Identifier>,
    river: Vec<Identifier>,
    swamp: Vec<Identifier>,
    cold: Vec<Identifier>,
    temperate_forest: Vec<Identifier>,
    grassland: Vec<Identifier>,
    hot_dry: Vec<Identifier>,
    mountain: Vec<Identifier>,
    jungle: Vec<Identifier>,
    cave: Vec<Identifier>,
}

impl BiomeRules {
    #[must_use]
    pub fn vanilla_overworld() -> Self {
        let ids = |names: &[&str]| -> Vec<Identifier> {
            names
                .iter()
                .map(|name| Identifier::parse(*name).expect("static biome identifier"))
                .collect()
        };
        Self {
            default: Identifier::parse("minecraft:plains").expect("static biome identifier"),
            all: ids(OVERWORLD_BIOME_IDS),
            deep_ocean: ids(&[
                "minecraft:deep_frozen_ocean",
                "minecraft:deep_cold_ocean",
                "minecraft:deep_ocean",
                "minecraft:deep_lukewarm_ocean",
            ]),
            ocean: ids(&[
                "minecraft:frozen_ocean",
                "minecraft:cold_ocean",
                "minecraft:ocean",
                "minecraft:lukewarm_ocean",
                "minecraft:warm_ocean",
            ]),
            beach: ids(&[
                "minecraft:beach",
                "minecraft:snowy_beach",
                "minecraft:stony_shore",
            ]),
            river: ids(&["minecraft:river", "minecraft:frozen_river"]),
            swamp: ids(&["minecraft:swamp", "minecraft:mangrove_swamp"]),
            cold: ids(&[
                "minecraft:snowy_plains",
                "minecraft:snowy_taiga",
                "minecraft:taiga",
                "minecraft:old_growth_pine_taiga",
                "minecraft:old_growth_spruce_taiga",
                "minecraft:ice_spikes",
                "minecraft:grove",
            ]),
            temperate_forest: ids(&[
                "minecraft:forest",
                "minecraft:flower_forest",
                "minecraft:birch_forest",
                "minecraft:old_growth_birch_forest",
                "minecraft:dark_forest",
                "minecraft:pale_garden",
                "minecraft:windswept_forest",
                "minecraft:cherry_grove",
            ]),
            grassland: ids(&[
                "minecraft:plains",
                "minecraft:sunflower_plains",
                "minecraft:meadow",
                "minecraft:mushroom_fields",
            ]),
            hot_dry: ids(&[
                "minecraft:desert",
                "minecraft:savanna",
                "minecraft:savanna_plateau",
                "minecraft:windswept_savanna",
                "minecraft:badlands",
                "minecraft:wooded_badlands",
                "minecraft:eroded_badlands",
            ]),
            mountain: ids(&[
                "minecraft:jagged_peaks",
                "minecraft:frozen_peaks",
                "minecraft:stony_peaks",
                "minecraft:snowy_slopes",
                "minecraft:windswept_hills",
                "minecraft:windswept_gravelly_hills",
            ]),
            jungle: ids(&[
                "minecraft:jungle",
                "minecraft:sparse_jungle",
                "minecraft:bamboo_jungle",
            ]),
            cave: ids(&[
                "minecraft:dripstone_caves",
                "minecraft:lush_caves",
                "minecraft:deep_dark",
            ]),
        }
    }

    #[must_use]
    pub fn from_worldgen_data(data: &BiomeWorldgenData) -> Option<Self> {
        let fallback = Self::vanilla_overworld();
        let tag = |name: &str| -> Vec<Identifier> {
            data.tag(&Identifier::parse(format!("minecraft:{name}")).expect("static biome tag"))
                .to_vec()
        };
        let by_name = |names: &[&str]| -> Vec<Identifier> {
            names
                .iter()
                .filter_map(|name| Identifier::parse(format!("minecraft:{name}")).ok())
                .filter(|id| data.biomes().any(|biome| biome == id))
                .collect()
        };
        let all = tag("is_overworld");
        if all.is_empty() {
            return None;
        }
        let deep_ocean = tag("is_deep_ocean");
        let ocean = without(tag("is_ocean"), &deep_ocean);
        Some(Self {
            default: Identifier::parse("minecraft:plains").expect("static biome identifier"),
            all,
            deep_ocean: or_fallback(deep_ocean, &fallback.deep_ocean),
            ocean: or_fallback(ocean, &fallback.ocean),
            beach: or_fallback(
                union([tag("is_beach"), by_name(&["stony_shore"])]),
                &fallback.beach,
            ),
            river: or_fallback(tag("is_river"), &fallback.river),
            swamp: or_fallback(by_name(&["swamp", "mangrove_swamp"]), &fallback.swamp),
            cold: or_fallback(
                union([
                    tag("is_taiga"),
                    by_name(&["snowy_plains", "ice_spikes", "grove"]),
                ]),
                &fallback.cold,
            ),
            temperate_forest: or_fallback(
                union([tag("is_forest"), by_name(&["cherry_grove", "pale_garden"])]),
                &fallback.temperate_forest,
            ),
            grassland: or_fallback(
                by_name(&["plains", "sunflower_plains", "meadow", "mushroom_fields"]),
                &fallback.grassland,
            ),
            hot_dry: or_fallback(
                union([tag("is_badlands"), tag("is_savanna"), by_name(&["desert"])]),
                &fallback.hot_dry,
            ),
            mountain: or_fallback(
                union([tag("is_mountain"), tag("is_hill")]),
                &fallback.mountain,
            ),
            jungle: or_fallback(tag("is_jungle"), &fallback.jungle),
            cave: or_fallback(
                by_name(&["dripstone_caves", "lush_caves", "deep_dark"]),
                &fallback.cave,
            ),
        })
    }

    #[must_use]
    pub fn overworld_ids(&self) -> &[Identifier] {
        &self.all
    }

    fn pick(&self, bucket: &[Identifier], x: i32, z: i32, salt: u64) -> Identifier {
        if bucket.is_empty() {
            return self.default.clone();
        }
        let idx = feature_hash(0, x, 0, z, salt) as usize % bucket.len();
        bucket[idx].clone()
    }

    fn is_ocean(&self, biome: &Identifier) -> bool {
        self.ocean.contains(biome) || self.deep_ocean.contains(biome)
    }

    fn is_beach_or_shore(&self, biome: &Identifier) -> bool {
        self.beach.contains(biome)
    }

    fn is_river(&self, biome: &Identifier) -> bool {
        self.river.contains(biome)
    }

    fn is_surface_water(&self, biome: &Identifier) -> bool {
        self.is_ocean(biome) || self.is_river(biome)
    }
}

fn or_fallback(values: Vec<Identifier>, fallback: &[Identifier]) -> Vec<Identifier> {
    if values.is_empty() {
        fallback.to_vec()
    } else {
        values
    }
}

fn union<const N: usize>(lists: [Vec<Identifier>; N]) -> Vec<Identifier> {
    let mut out = Vec::new();
    for id in lists.into_iter().flatten() {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn without(mut values: Vec<Identifier>, excluded: &[Identifier]) -> Vec<Identifier> {
    values.retain(|id| !excluded.contains(id));
    values
}

fn ore_targets(
    registry: &BlockRegistry,
    targets: &[OreTarget],
) -> Option<(BlockStateId, BlockStateId)> {
    let mut normal = None;
    let mut deepslate = None;
    for target in targets {
        let state = registry.block(&target.state)?.default;
        match target.replaceable_tag.as_ref().map(Identifier::as_str) {
            Some("minecraft:stone_ore_replaceables") => normal = Some(state),
            Some("minecraft:deepslate_ore_replaceables") => deepslate = Some(state),
            _ => {}
        }
    }
    Some((normal?, deepslate?))
}

fn height_anchor_y(anchor: HeightAnchor) -> i32 {
    match anchor {
        HeightAnchor::Absolute(y) => y,
        HeightAnchor::AboveBottom(offset) => MIN_Y + offset,
        HeightAnchor::BelowTop(offset) => MAX_Y - offset,
    }
}

fn ore_spacing(placement: &mc_data::worldgen_ores::OrePlacement, y: YRange) -> OreSpacing {
    let count = match placement.count {
        Some(OrePlacementCount::Constant(count)) => count.max(1),
        Some(OrePlacementCount::Uniform { min, max }) => ((min + max) / 2).max(1),
        None => 1,
    } as u64;
    let density = count.saturating_mul(7).max(1);
    let base = 512u64.saturating_div(density).max(5);
    if placement
        .height
        .as_ref()
        .is_some_and(|height| height.kind.as_str() == "minecraft:trapezoid")
    {
        OreSpacing::peaked((y.min + y.max) / 2, base, base * 3)
    } else {
        OreSpacing::Fixed(base)
    }
}

impl TerrainGenerator {
    /// Build a generator from a seed plus a block registry. Panics
    /// if any of the four required blocks (air, bedrock, stone,
    /// dirt, grass_block) is missing from the registry — they are
    /// vanilla-mandatory for any 26.1.2 world and resolving them
    /// once at construction time keeps `generate` hot-path-free.
    #[must_use]
    pub fn new(seed: i64, registry: Arc<BlockRegistry>) -> Self {
        Self::with_biome_rules(seed, registry, BiomeRules::vanilla_overworld())
    }

    #[must_use]
    pub fn with_biome_rules(seed: i64, registry: Arc<BlockRegistry>, biomes: BiomeRules) -> Self {
        let stone = resolve_block(registry.as_ref(), "minecraft:stone");
        let ores = OreRules::solaris_default(registry.as_ref(), &biomes, stone);
        Self::with_rules(seed, registry, biomes, ores)
    }

    #[must_use]
    pub fn with_rules(
        seed: i64,
        registry: Arc<BlockRegistry>,
        biomes: BiomeRules,
        ores: OreRules,
    ) -> Self {
        let air = resolve_block(registry.as_ref(), "minecraft:air");
        let stone = resolve_block(registry.as_ref(), "minecraft:stone");
        Self {
            seed,
            air,
            bedrock: resolve_block(registry.as_ref(), "minecraft:bedrock"),
            stone,
            dirt: resolve_block(registry.as_ref(), "minecraft:dirt"),
            grass_block: resolve_block(registry.as_ref(), "minecraft:grass_block"),
            sand: resolve_block_or(registry.as_ref(), "minecraft:sand", stone),
            deepslate: resolve_block_or(registry.as_ref(), "minecraft:deepslate", stone),
            water: resolve_block_or(registry.as_ref(), "minecraft:water", air),
            biomes,
            ores,
            structures: StructureRules::none(),
            decorations: DecorationBlocks::new(registry.as_ref()),
            registry,
        }
    }

    #[must_use]
    pub fn with_structures(mut self, structures: StructureRules) -> Self {
        self.structures = structures;
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
        let hills = fbm_2d(
            world_x as f64 * NOISE_FREQUENCY,
            world_z as f64 * NOISE_FREQUENCY,
            self.seed,
            NOISE_OCTAVES,
            NOISE_PERSISTENCE,
        );
        let continental = self.continentalness(world_x, world_z);
        let river = self.river_signal(world_x, world_z);
        let raw = if continental < OCEAN_THRESHOLD {
            let depth = (OCEAN_THRESHOLD - continental) * 58.0;
            SEA_LEVEL as f64 - 8.0 - depth + hills * 4.0
        } else {
            let uplift = continental.max(0.0) * 20.0;
            let river_cut = if river.abs() < 0.035 {
                18.0 * (1.0 - river.abs() / 0.035)
            } else {
                0.0
            };
            BASE_HEIGHT + uplift + hills * HEIGHT_AMPLITUDE + self.ridges(world_x, world_z) * 18.0
                - river_cut
        };
        // Guard against extreme outputs even though fbm_2d is bounded.
        raw.round().clamp(MIN_Y as f64 + 2.0, 250.0) as i32
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
        if river.abs() < 0.035 && continental > -0.08 {
            return self
                .biomes
                .pick(&self.biomes.river, world_x, world_z, 0x5249_5645);
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
            world_x as f64 / 360.0,
            world_z as f64 / 360.0,
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

    fn river_signal(&self, world_x: i32, world_z: i32) -> f64 {
        fbm_2d(
            world_x as f64 / 150.0,
            world_z as f64 / 150.0,
            self.seed ^ 0x5249_5645_5200,
            2,
            0.5,
        )
    }

    fn fill_column(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        lz: u8,
        height: i32,
        biome: &Identifier,
    ) -> i32 {
        let _ = chunk.set_block(lx, MIN_Y, lz, self.bedrock);
        let dirt_start = (height - DIRT_DEPTH).max(MIN_Y + 1);
        for y in (MIN_Y + 1)..dirt_start {
            let _ = chunk.set_block(lx, y, lz, self.base_stone_for_y(lx, y, lz, chunk.pos));
        }
        let surface = if self.biomes.is_surface_water(biome) || self.biomes.is_beach_or_shore(biome)
        {
            self.sand
        } else {
            self.grass_block
        };
        let fill = if surface == self.sand {
            self.sand
        } else {
            self.dirt
        };
        for y in dirt_start..height {
            let _ = chunk.set_block(lx, y, lz, fill);
        }
        let _ = chunk.set_block(lx, height, lz, surface);
        let mut top_non_air = height;
        if height < SEA_LEVEL || self.biomes.is_river(biome) {
            for y in (height + 1)..=SEA_LEVEL {
                let _ = chunk.set_block(lx, y, lz, self.water);
            }
            top_non_air = SEA_LEVEL;
        }
        // Air above stays as-is from Chunk::empty.
        let _ = self.air;
        top_non_air
    }

    fn assign_biomes(&self, chunk: &mut Chunk, column_heights: &[i32; 256]) {
        let pos = chunk.pos;
        for (section_idx, section) in chunk.biomes.iter_mut().enumerate() {
            let mut palette: Vec<Identifier> = Vec::new();
            let mut indices = PackedBitArray::zeroed(6, BIOME_VOLUME);
            for cy in 0..BIOME_DIM {
                for cz in 0..BIOME_DIM {
                    for cx in 0..BIOME_DIM {
                        let lx = cx * 4 + 2;
                        let lz = cz * 4 + 2;
                        let wx = pos.x * 16 + lx as i32;
                        let wz = pos.z * 16 + lz as i32;
                        let y = MIN_Y + section_idx as i32 * 16 + cy as i32 * 4 + 2;
                        let biome = self.biome_for_cell(wx, y, wz, column_heights[lz * 16 + lx]);
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

    fn apply_features(&self, chunk: &mut Chunk, lx: u8, lz: u8, height: i32) {
        let wx = chunk.pos.x * 16 + lx as i32;
        let wz = chunk.pos.z * 16 + lz as i32;
        let cave_max_y = (height - CAVE_SURFACE_CLEARANCE).max(CAVE_MIN_Y);
        for y in (MIN_Y + 1)..height {
            if y >= CAVE_MIN_Y && y <= cave_max_y && self.is_cave_cell(wx, y, wz) {
                let _ = chunk.set_block(lx, y, lz, self.air);
                continue;
            }

            if matches!(chunk.get_block(lx, y, lz), Some(state) if state == self.stone || state == self.deepslate)
            {
                let biome = self.biome_for(wx, wz, height);
                let ore = self.ore_for(
                    wx,
                    y,
                    wz,
                    chunk.get_block(lx, y, lz).unwrap_or(self.stone),
                    &biome,
                );
                if ore != self.stone && ore != self.deepslate {
                    let _ = chunk.set_block(lx, y, lz, ore);
                }
            }
        }
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

    fn apply_decorations(&self, chunk: &mut Chunk, column_heights: &[i32; 256]) {
        let mut touched = [false; 256];
        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let idx = lz as usize * 16 + lx as usize;
                let height = column_heights[idx];
                if height <= MIN_Y || height + 8 >= MAX_Y {
                    continue;
                }
                let wx = chunk.pos.x * 16 + lx as i32;
                let wz = chunk.pos.z * 16 + lz as i32;
                let biome = self.biome_for(wx, wz, height);
                let surface = chunk.get_block(lx, height, lz).unwrap_or(self.air);
                let h = feature_hash(self.seed, wx, height, wz, 0xDEC0_0001);

                if (self.biomes.temperate_forest.contains(&biome)
                    || self.biomes.jungle.contains(&biome))
                    && h.is_multiple_of(83)
                    && self.place_tree(chunk, lx, height + 1, lz, &mut touched)
                {
                    continue;
                }
                if self.biomes.hot_dry.contains(&biome)
                    && surface == self.sand
                    && h.is_multiple_of(47)
                    && self.place_cactus(chunk, lx, height + 1, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.beach.contains(&biome) || self.biomes.river.contains(&biome))
                    && h.is_multiple_of(29)
                    && self.place_sugar_cane(chunk, lx, height + 1, lz, &mut touched)
                {
                    continue;
                }
                if (self.biomes.grassland.contains(&biome)
                    || self.biomes.temperate_forest.contains(&biome))
                    && surface == self.grass_block
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

    fn place_tree(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [bool; 256],
    ) -> bool {
        let (Some(log), Some(leaves)) = (self.decorations.oak_log, self.decorations.oak_leaves)
        else {
            return false;
        };
        if !(2..=13).contains(&lx) || !(2..=13).contains(&lz) || base_y + 5 >= MAX_Y {
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

    fn place_cactus(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        base_y: i32,
        lz: u8,
        touched: &mut [bool; 256],
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
        touched: &mut [bool; 256],
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
                (0..16).contains(&x)
                    && (0..16).contains(&z)
                    && (chunk.get_block(x as u8, y, z as u8) == Some(self.water)
                        || chunk.get_block(x as u8, y + 1, z as u8) == Some(self.water))
            })
    }

    fn place_single(
        &self,
        chunk: &mut Chunk,
        lx: u8,
        y: i32,
        lz: u8,
        state: BlockStateId,
        touched: &mut [bool; 256],
    ) {
        if chunk.set_block(lx, y, lz, state).is_some() {
            touched[lz as usize * 16 + lx as usize] = true;
        }
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
        if center_height <= SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA {
            return;
        }
        let biome = self.biome_for(center_x, center_z, center_height);
        if !self.biomes.grassland.contains(&biome) {
            return;
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
        let top = (MIN_Y..MAX_Y)
            .rev()
            .find(|&y| {
                chunk
                    .get_block(lx, y, lz)
                    .is_some_and(|state| state != self.air)
            })
            .unwrap_or(MIN_Y);
        let value = (top + 1 - MIN_Y) as u32;
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
        n > CAVE_THRESHOLD
    }

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
}

impl ChunkGenerator for TerrainGenerator {
    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::empty(pos, self.air, self.biomes.default.clone());
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), Heightmap::zeroed());
        let mut column_heights = [MIN_Y; 256];

        for lz in 0..16u8 {
            for lx in 0..16u8 {
                let wx = pos.x * 16 + lx as i32;
                let wz = pos.z * 16 + lz as i32;
                let height = self.surface_height(wx, wz);
                let biome = self.biome_for(wx, wz, height);
                let top_non_air = self.fill_column(&mut chunk, lx, lz, height, &biome);
                column_heights[lz as usize * 16 + lx as usize] = height;
                self.apply_features(&mut chunk, lx, lz, height);
                // Heightmap value: Y of the first air cell above the
                // top non-air block, expressed as `(top + 1) - MIN_Y`.
                let world_surface = (top_non_air + 1 - MIN_Y) as u32;
                let motion_blocking = (height + 1 - MIN_Y) as u32;
                if let Some(mb) = chunk.heightmaps.get_mut("MOTION_BLOCKING") {
                    mb.set(lx, lz, motion_blocking);
                }
                if let Some(ws) = chunk.heightmaps.get_mut("WORLD_SURFACE") {
                    ws.set(lx, lz, world_surface);
                }
                chunk.highest_opaque.set(lx, lz, motion_blocking);
            }
        }
        self.assign_biomes(&mut chunk, &column_heights);
        self.apply_decorations(&mut chunk, &column_heights);
        self.apply_structures(&mut chunk);
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
                id: Identifier::parse("minecraft:sand").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 14,
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
            BlockReport {
                id: Identifier::parse("minecraft:gold_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 15,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:redstone_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 16,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:diamond_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 17,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:lapis_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 18,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:emerald_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 19,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_gold_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 20,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_redstone_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 21,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_diamond_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 22,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_lapis_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 23,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:deepslate_emerald_ore").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 24,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:stone_bricks").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 25,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:oak_log").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 26,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:oak_leaves").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 27,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:short_grass").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 28,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:dandelion").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 29,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:poppy").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 30,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:pumpkin").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 31,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:sugar_cane").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 32,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
            BlockReport {
                id: Identifier::parse("minecraft:cactus").unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: 33,
                    default: true,
                    properties: BTreeMap::new(),
                }],
            },
        ];
        Arc::new(BlockRegistry::from_report(&report).unwrap())
    }

    #[test]
    fn generated_column_has_bedrock_and_biome_surface() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let chunk = g.generate(ChunkPos { x: 0, z: 0 });
        let air = BlockStateId(0);
        let bedrock = BlockStateId(1);
        let grass = BlockStateId(4);
        let water = BlockStateId(5);
        let sand = BlockStateId(14);

        // Bedrock at MIN_Y.
        assert_eq!(chunk.get_block(8, MIN_Y, 8), Some(bedrock));
        // Find the terrain surface. Biome selection decides whether it
        // is grassland or a water/coast sand surface.
        let height = g.surface_height(8, 8);
        assert!(
            matches!(chunk.get_block(8, height, 8), Some(state) if state == grass || state == sand)
        );
        if height < SEA_LEVEL {
            assert_eq!(chunk.get_block(8, SEA_LEVEL, 8), Some(water));
            assert_eq!(chunk.get_block(8, SEA_LEVEL + 1, 8), Some(air));
        } else {
            assert_eq!(chunk.get_block(8, height + 1, 8), Some(air));
        }

        // Heightmap value matches the height field.
        let hm = chunk.heightmaps.get("MOTION_BLOCKING").unwrap();
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
    fn different_seeds_change_generated_chunks() {
        let a = TerrainGenerator::new(0, tiny_registry());
        let b = TerrainGenerator::new(1, tiny_registry());
        let positions = [
            ChunkPos { x: 0, z: 0 },
            ChunkPos { x: 5, z: -3 },
            ChunkPos { x: -12, z: 8 },
        ];

        for pos in positions {
            let chunk_a = a.generate(pos);
            let chunk_b = b.generate(pos);
            for y in MIN_Y..=96 {
                for x in 0..16u8 {
                    for z in 0..16u8 {
                        if chunk_a.get_block(x, y, z) != chunk_b.get_block(x, y, z) {
                            return;
                        }
                    }
                }
            }
        }

        panic!("different world seeds should alter at least one sampled generated chunk");
    }

    #[test]
    fn persisted_chunk_edit_wins_after_seed_change() {
        let registry = tiny_registry();
        let generator_a = Arc::new(TerrainGenerator::new(0, Arc::clone(&registry)));
        let generator_b = Arc::new(TerrainGenerator::new(1, Arc::clone(&registry)));
        let cpos = ChunkPos { x: 5, z: -3 };
        let world_x = cpos.x * 16 + 8;
        let world_z = cpos.z * 16 + 8;
        let edit_pos = mc_world::chunk::BlockPos {
            x: world_x,
            y: generator_a.surface_height(world_x, world_z) + 1,
            z: world_z,
        };
        let marker = BlockStateId(25);
        let root = unique_temp_world_dir();
        std::fs::create_dir_all(root.join("region")).unwrap();

        let mut storage = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
            .unwrap()
            .with_generator(Arc::clone(&generator_a) as Arc<dyn ChunkGenerator>);
        assert_ne!(
            storage.get_block(edit_pos).unwrap(),
            Some(marker),
            "generated fallback should not already contain the edit marker"
        );
        storage.set_block_at(edit_pos, marker).unwrap().unwrap();
        assert!(storage.flush_dirty().unwrap() >= 1);
        drop(storage);

        let mut reopened = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
            .unwrap()
            .with_generator(Arc::clone(&generator_b) as Arc<dyn ChunkGenerator>);
        assert_eq!(reopened.get_block(edit_pos).unwrap(), Some(marker));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn far_chunks_still_have_terrain() {
        let g = TerrainGenerator::new(1234, tiny_registry());
        let chunk = g.generate(ChunkPos {
            x: 1_000,
            z: -1_000,
        });
        let grass = BlockStateId(4);
        let sand = BlockStateId(14);
        let height = g.surface_height(1_000 * 16 + 8, -1_000 * 16 + 8);
        assert!(
            matches!(chunk.get_block(8, height, 8), Some(state) if state == grass || state == sand)
        );
        assert_eq!(chunk.status, "minecraft:full");
    }

    #[test]
    fn default_seed_origin_remains_land_spawn() {
        let g = TerrainGenerator::new(0, tiny_registry());
        let height = g.surface_height(0, 0);
        let biome = g.biome_for(0, 0, height);

        assert!(height > SEA_LEVEL, "spawn origin should not be underwater");
        assert!(!g.biomes.is_ocean(&biome));
    }

    #[test]
    fn continental_mask_produces_water_coasts_and_land_biomes() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let mut saw_ocean = false;
        let mut saw_beach = false;
        let mut saw_forest_or_jungle = false;
        let mut saw_grass_or_dry = false;
        let mut saw_mountain = false;
        let mut ocean_column = None;

        for wx in (-512..=512).step_by(16) {
            for wz in (-512..=512).step_by(16) {
                let height = g.surface_height(wx, wz);
                let biome = g.biome_for(wx, wz, height);
                if g.biomes.is_ocean(&biome) {
                    saw_ocean = true;
                    ocean_column.get_or_insert((wx, wz, height));
                } else if g.biomes.is_beach_or_shore(&biome) {
                    saw_beach = true;
                } else if g.biomes.temperate_forest.contains(&biome)
                    || g.biomes.jungle.contains(&biome)
                {
                    saw_forest_or_jungle = true;
                } else if g.biomes.grassland.contains(&biome) || g.biomes.hot_dry.contains(&biome) {
                    saw_grass_or_dry = true;
                } else if g.biomes.mountain.contains(&biome) {
                    saw_mountain = true;
                }
            }
        }

        assert!(saw_ocean, "expected ocean cells in the sampled area");
        assert!(saw_beach, "expected beach cells around coastlines");
        assert!(
            saw_forest_or_jungle,
            "expected forest/jungle cells in the sampled area"
        );
        assert!(
            saw_grass_or_dry,
            "expected grassland/dry cells in the sampled area"
        );
        assert!(saw_mountain, "expected mountain cells in the sampled area");

        let (wx, wz, height) = ocean_column.unwrap();
        let chunk = g.generate(ChunkPos {
            x: wx.div_euclid(16),
            z: wz.div_euclid(16),
        });
        let lx = wx.rem_euclid(16) as u8;
        let lz = wz.rem_euclid(16) as u8;
        assert!(height < SEA_LEVEL);
        assert_eq!(chunk.get_block(lx, SEA_LEVEL, lz), Some(BlockStateId(5)));
        assert_eq!(
            chunk.get_block(lx, SEA_LEVEL + 1, lz),
            Some(BlockStateId(0))
        );
    }

    #[test]
    fn surface_decorations_are_visible_and_refresh_heightmaps() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let decorations = [
            BlockStateId(26),
            BlockStateId(27),
            BlockStateId(28),
            BlockStateId(29),
            BlockStateId(30),
            BlockStateId(31),
            BlockStateId(32),
            BlockStateId(33),
        ];
        let mut saw_decoration = false;

        for cx in -2..=2 {
            for cz in -2..=2 {
                let chunk = g.generate(ChunkPos { x: cx, z: cz });
                let again = g.generate(ChunkPos { x: cx, z: cz });
                for lx in 0..16u8 {
                    for lz in 0..16u8 {
                        let wx = cx * 16 + lx as i32;
                        let wz = cz * 16 + lz as i32;
                        let height = g.surface_height(wx, wz);
                        for y in (height + 1)..=(height + 8).min(MAX_Y - 1) {
                            assert_eq!(chunk.get_block(lx, y, lz), again.get_block(lx, y, lz));
                            if chunk
                                .get_block(lx, y, lz)
                                .is_some_and(|state| decorations.contains(&state))
                            {
                                saw_decoration = true;
                                assert!(chunk.highest_opaque_y(lx, lz).is_some_and(|top| top >= y));
                                assert!(
                                    chunk.heightmaps["WORLD_SURFACE"].get(lx, lz)
                                        >= (y + 1 - MIN_Y) as u32
                                );
                            }
                        }
                    }
                }
            }
        }

        assert!(
            saw_decoration,
            "sampled chunks should contain surface decorations"
        );
    }

    #[test]
    fn feature_facts_drive_grass_decoration_block() {
        let registry = tiny_registry();
        let poppy = Identifier::parse("minecraft:poppy").unwrap();
        let features = vec![WorldgenFeatureFacts {
            placed_feature: Identifier::parse("minecraft:patch_grass_plain").unwrap(),
            configured_feature: Identifier::parse("minecraft:test_patch_grass").unwrap(),
            configured_type: Identifier::parse("minecraft:simple_block").unwrap(),
            placement: mc_data::worldgen_features::FeaturePlacementFacts {
                count: Some(FeatureCount::Constant(32)),
                has_biome_filter: true,
                ..Default::default()
            },
            block_states: vec![poppy],
            tags: vec![],
        }];
        let g = TerrainGenerator::new(42, registry).with_feature_facts(&features);
        let grass = BlockStateId(4);
        let data_fed_plant = BlockStateId(30);

        for cx in -8..=8 {
            for cz in -8..=8 {
                let chunk = g.generate(ChunkPos { x: cx, z: cz });
                for lx in 0..16u8 {
                    for lz in 0..16u8 {
                        let wx = cx * 16 + lx as i32;
                        let wz = cz * 16 + lz as i32;
                        let height = g.surface_height(wx, wz);
                        let biome = g.biome_for(wx, wz, height);
                        if !(g.biomes.grassland.contains(&biome)
                            || g.biomes.temperate_forest.contains(&biome))
                            || chunk.get_block(lx, height, lz) != Some(grass)
                        {
                            continue;
                        }
                        let h = feature_hash(g.seed, wx, height, wz, 0xDEC0_0001);
                        if h.is_multiple_of(8)
                            && !h.is_multiple_of(97)
                            && !h.is_multiple_of(37)
                            && !h.is_multiple_of(41)
                        {
                            assert_eq!(chunk.get_block(lx, height + 1, lz), Some(data_fed_plant));
                            return;
                        }
                    }
                }
            }
        }

        panic!("sampled chunks should contain a grass decoration from feature facts");
    }

    #[test]
    fn generated_overlays_survive_flush_and_reopen() {
        let registry = tiny_registry();
        let marker = BlockStateId(25);
        let template = StructureTemplate::new(
            [1, 1, 1],
            vec![crate::structures::TemplateBlock {
                pos: [0, 0, 0],
                state: marker,
            }],
        );
        let structures =
            StructureRules::plains_village_markers(vec![template]).with_spacing_for_tests(1, 0);
        let generator =
            Arc::new(TerrainGenerator::new(42, Arc::clone(&registry)).with_structures(structures));
        let mut structure_target = None;
        'cells: for gx in -64..=64 {
            for gz in -64..=64 {
                let Some((_template, center_x, center_z)) = generator.structure_plan(gx, gz) else {
                    continue;
                };
                let height = generator.surface_height(center_x, center_z);
                let biome = generator.biome_for(center_x, center_z, height);
                if height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA
                    && generator.biomes.grassland.contains(&biome)
                {
                    structure_target = Some((center_x, height + 1, center_z));
                    break 'cells;
                }
            }
        }
        let (structure_x, structure_y, structure_z) = structure_target.expect("structure target");

        let root = unique_temp_world_dir();
        std::fs::create_dir_all(root.join("region")).unwrap();
        let mut storage = mc_world::WorldStorage::open(&root, Arc::clone(&registry))
            .unwrap()
            .with_generator(Arc::clone(&generator) as Arc<dyn ChunkGenerator>);
        let structure_pos = ChunkPos {
            x: structure_x.div_euclid(16),
            z: structure_z.div_euclid(16),
        };
        let structure_lx = structure_x.rem_euclid(16) as u8;
        let structure_lz = structure_z.rem_euclid(16) as u8;
        let chunk = storage
            .get_chunk(structure_pos)
            .unwrap()
            .expect("generated structure chunk");
        assert_eq!(
            chunk.get_block(structure_lx, structure_y, structure_lz),
            Some(marker)
        );

        let (decor_pos, decor_lx, decor_y, decor_lz, decor_state) =
            find_decoration_in_storage(&mut storage, &generator);
        assert!(storage.dirty_count() >= 1);
        assert!(storage.flush_dirty().unwrap() >= 1);
        drop(storage);

        let mut fresh = mc_world::WorldStorage::open(&root, Arc::clone(&registry)).unwrap();
        let structure_chunk = fresh
            .get_chunk(structure_pos)
            .unwrap()
            .expect("reopened structure chunk");
        assert_eq!(
            structure_chunk.get_block(structure_lx, structure_y, structure_lz),
            Some(marker)
        );
        assert!(
            structure_chunk
                .highest_opaque_y(structure_lx, structure_lz)
                .is_some_and(|top| top >= structure_y)
        );

        let decoration_chunk = fresh
            .get_chunk(decor_pos)
            .unwrap()
            .expect("reopened decoration chunk");
        assert_eq!(
            decoration_chunk.get_block(decor_lx, decor_y, decor_lz),
            Some(decor_state)
        );
        assert!(
            decoration_chunk
                .highest_opaque_y(decor_lx, decor_lz)
                .is_some_and(|top| top >= decor_y)
        );

        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_temp_world_dir() -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("solaris-worldgen-{suffix}"))
    }

    fn find_decoration_in_storage(
        storage: &mut mc_world::WorldStorage,
        generator: &TerrainGenerator,
    ) -> (ChunkPos, u8, i32, u8, BlockStateId) {
        let decorations = [
            BlockStateId(26),
            BlockStateId(27),
            BlockStateId(28),
            BlockStateId(29),
            BlockStateId(30),
            BlockStateId(31),
            BlockStateId(32),
            BlockStateId(33),
        ];
        for cx in -2..=2 {
            for cz in -2..=2 {
                let pos = ChunkPos { x: cx, z: cz };
                let chunk = storage.get_chunk(pos).unwrap().expect("generated chunk");
                for lx in 0..16u8 {
                    for lz in 0..16u8 {
                        let wx = cx * 16 + lx as i32;
                        let wz = cz * 16 + lz as i32;
                        let height = generator.surface_height(wx, wz);
                        for y in (height + 1)..=(height + 8).min(MAX_Y - 1) {
                            if let Some(state) = chunk.get_block(lx, y, lz)
                                && decorations.contains(&state)
                            {
                                return (pos, lx, y, lz, state);
                            }
                        }
                    }
                }
            }
        }
        panic!("sampled chunks should contain a decoration");
    }

    #[test]
    fn every_overworld_biome_is_reachable_by_selector() {
        use std::collections::BTreeSet;

        let g = TerrainGenerator::new(42, tiny_registry());
        let expected: BTreeSet<_> = g.biomes.all.iter().map(Identifier::as_str).collect();
        let mut seen = BTreeSet::new();

        for seed_offset in 0..4 {
            let g = TerrainGenerator::new(42 + seed_offset, tiny_registry());
            for wx in (-4096..=4096).step_by(32) {
                for wz in (-4096..=4096).step_by(32) {
                    let height = g.surface_height(wx, wz);
                    let biome = g.biome_for(wx, wz, height);
                    seen.insert(biome.as_str().to_string());
                    for y in (-48..=48).step_by(16) {
                        let biome = g.biome_for_cell(wx, y, wz, height);
                        seen.insert(biome.as_str().to_string());
                    }
                }
            }
        }

        for biome in expected {
            assert!(seen.contains(biome), "selector never emitted {biome}");
        }
    }

    #[test]
    fn structure_rules_paste_intersecting_template_blocks() {
        let marker = BlockStateId(25);
        let template = StructureTemplate::new(
            [1, 1, 1],
            vec![crate::structures::TemplateBlock {
                pos: [0, 0, 0],
                state: marker,
            }],
        );
        let structures =
            StructureRules::single_plains_village_marker(template).with_spacing_for_tests(1, 0);
        let g = TerrainGenerator::new(42, tiny_registry()).with_structures(structures);

        let mut target = None;
        'cells: for gx in -64..=64 {
            for gz in -64..=64 {
                let Some((_template, center_x, center_z)) = g.structure_plan(gx, gz) else {
                    continue;
                };
                let height = g.surface_height(center_x, center_z);
                let biome = g.biome_for(center_x, center_z, height);
                if height > SEA_LEVEL + BEACH_HEIGHT_ABOVE_SEA
                    && g.biomes.grassland.contains(&biome)
                {
                    target = Some((center_x, center_z, height + 1));
                    break 'cells;
                }
            }
        }
        let (wx, wz, y) = target.expect("dense structure grid should find a land grassland cell");
        let chunk = g.generate(ChunkPos {
            x: wx.div_euclid(16),
            z: wz.div_euclid(16),
        });
        let lx = wx.rem_euclid(16) as u8;
        let lz = wz.rem_euclid(16) as u8;

        assert_eq!(chunk.get_block(lx, y, lz), Some(marker));
        assert_eq!(
            chunk.heightmaps["WORLD_SURFACE"].get(lx, lz),
            (y + 1 - MIN_Y) as u32
        );
    }

    fn find_ore_cell(
        g: &TerrainGenerator,
        y: i32,
        biome: &Identifier,
        expected: BlockStateId,
    ) -> (i32, i32) {
        for x in -256..=256 {
            for z in -256..=256 {
                if g.ore_for(x, y, z, g.stone, biome) == expected {
                    return (x, z);
                }
            }
        }
        panic!("could not find ore {expected:?} at y={y} for biome {biome}");
    }

    fn ore_feature(
        feature: &str,
        normal: &str,
        deepslate: &str,
        min_y: i32,
        max_y: i32,
        count: u32,
    ) -> OreFeature {
        mc_data::worldgen_ores::OreFeature {
            placed_feature: Identifier::parse(feature).unwrap(),
            configured_feature: Identifier::parse(feature).unwrap(),
            placement: mc_data::worldgen_ores::OrePlacement {
                count: Some(OrePlacementCount::Constant(count)),
                rarity_chance: None,
                height: Some(mc_data::worldgen_ores::HeightRange {
                    kind: Identifier::parse("minecraft:uniform").unwrap(),
                    min: HeightAnchor::Absolute(min_y),
                    max: HeightAnchor::Absolute(max_y),
                }),
            },
            size: 4,
            discard_chance_on_air_exposure: 0.0,
            targets: vec![
                OreTarget {
                    state: Identifier::parse(normal).unwrap(),
                    replaceable_tag: Some(
                        Identifier::parse("minecraft:stone_ore_replaceables").unwrap(),
                    ),
                },
                OreTarget {
                    state: Identifier::parse(deepslate).unwrap(),
                    replaceable_tag: Some(
                        Identifier::parse("minecraft:deepslate_ore_replaceables").unwrap(),
                    ),
                },
            ],
        }
    }

    #[test]
    fn default_ore_rules_keep_priority_order() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let rules = g.ores.rules();

        assert_eq!(rules.len(), 9);
        assert_eq!(rules[0].normal, BlockStateId(19));
        assert!(matches!(&rules[0].biomes, BiomeScope::Only(_)));
        assert_eq!(rules[1].normal, BlockStateId(15));
        assert!(matches!(&rules[1].spacing, OreSpacing::Fixed(58)));
        assert_eq!(rules[2].normal, BlockStateId(17));
        assert_eq!(rules[8].normal, BlockStateId(8));
    }

    #[test]
    fn expanded_ore_families_are_reachable_and_biome_scoped() {
        let g = TerrainGenerator::new(42, tiny_registry());
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
        let hot_dry = Identifier::parse("minecraft:badlands").unwrap();

        for (y, biome, stone_ore, deepslate_ore) in [
            (-16, &plains, BlockStateId(15), BlockStateId(20)),
            (15, &plains, BlockStateId(16), BlockStateId(21)),
            (-56, &plains, BlockStateId(17), BlockStateId(22)),
            (64, &plains, BlockStateId(18), BlockStateId(23)),
            (224, &mountain, BlockStateId(19), BlockStateId(24)),
            (80, &hot_dry, BlockStateId(15), BlockStateId(20)),
        ] {
            let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
            assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
        }

        let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
        assert_eq!(
            g.ore_for(emerald_x, 224, emerald_z, g.stone, &plains),
            g.stone,
            "emerald should stay mountain-scoped"
        );

        let (gold_x, gold_z) = find_ore_cell(&g, 80, &hot_dry, BlockStateId(15));
        assert_ne!(
            g.ore_for(gold_x, 80, gold_z, g.stone, &plains),
            BlockStateId(15),
            "high gold boost should stay hot-dry scoped"
        );
    }

    #[test]
    fn data_fed_ore_rules_reach_generation() {
        let registry = tiny_registry();
        let biomes = BiomeRules::vanilla_overworld();
        let mountain = Identifier::parse("minecraft:jagged_peaks").unwrap();
        let hot_dry = Identifier::parse("minecraft:badlands").unwrap();
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let biome_data = mc_data::biomes::BiomeWorldgenData::from_parts(
            BTreeMap::from([
                (
                    mountain.clone(),
                    vec![Identifier::parse("minecraft:ore_emerald").unwrap()],
                ),
                (
                    hot_dry.clone(),
                    vec![Identifier::parse("minecraft:ore_gold_extra").unwrap()],
                ),
                (
                    plains.clone(),
                    vec![Identifier::parse("minecraft:ore_diamond").unwrap()],
                ),
            ]),
            BTreeMap::new(),
        );
        let features = vec![
            ore_feature(
                "minecraft:ore_emerald",
                "minecraft:emerald_ore",
                "minecraft:deepslate_emerald_ore",
                200,
                240,
                64,
            ),
            ore_feature(
                "minecraft:ore_gold_extra",
                "minecraft:gold_ore",
                "minecraft:deepslate_gold_ore",
                72,
                96,
                64,
            ),
            ore_feature(
                "minecraft:ore_diamond",
                "minecraft:diamond_ore",
                "minecraft:deepslate_diamond_ore",
                -64,
                -48,
                64,
            ),
        ];
        let ores =
            OreRules::from_features(registry.as_ref(), &biomes, &features, Some(&biome_data))
                .expect("sidecar ore features should become rules");
        let g = TerrainGenerator::with_rules(42, registry, biomes, ores);

        for (y, biome, stone_ore, deepslate_ore) in [
            (224, &mountain, BlockStateId(19), BlockStateId(24)),
            (80, &hot_dry, BlockStateId(15), BlockStateId(20)),
            (-56, &plains, BlockStateId(17), BlockStateId(22)),
        ] {
            let (x, z) = find_ore_cell(&g, y, biome, stone_ore);
            assert_eq!(g.ore_for(x, y, z, g.deepslate, biome), deepslate_ore);
        }

        let (emerald_x, emerald_z) = find_ore_cell(&g, 224, &mountain, BlockStateId(19));
        assert_eq!(
            g.ore_for(emerald_x, 224, emerald_z, g.stone, &plains),
            g.stone
        );
    }

    #[test]
    fn biome_rules_can_use_sidecar_tags() {
        let data = mc_data::biomes::BiomeWorldgenData::from_parts(
            BTreeMap::from([
                (Identifier::parse("minecraft:plains").unwrap(), Vec::new()),
                (Identifier::parse("minecraft:forest").unwrap(), Vec::new()),
                (Identifier::parse("minecraft:badlands").unwrap(), Vec::new()),
                (Identifier::parse("minecraft:ocean").unwrap(), Vec::new()),
                (
                    Identifier::parse("minecraft:deep_ocean").unwrap(),
                    Vec::new(),
                ),
            ]),
            BTreeMap::from([
                (
                    Identifier::parse("minecraft:is_overworld").unwrap(),
                    vec![
                        Identifier::parse("minecraft:plains").unwrap(),
                        Identifier::parse("minecraft:forest").unwrap(),
                        Identifier::parse("minecraft:badlands").unwrap(),
                        Identifier::parse("minecraft:ocean").unwrap(),
                        Identifier::parse("minecraft:deep_ocean").unwrap(),
                    ],
                ),
                (
                    Identifier::parse("minecraft:is_forest").unwrap(),
                    vec![Identifier::parse("minecraft:forest").unwrap()],
                ),
                (
                    Identifier::parse("minecraft:is_badlands").unwrap(),
                    vec![Identifier::parse("minecraft:badlands").unwrap()],
                ),
                (
                    Identifier::parse("minecraft:is_ocean").unwrap(),
                    vec![
                        Identifier::parse("minecraft:ocean").unwrap(),
                        Identifier::parse("minecraft:deep_ocean").unwrap(),
                    ],
                ),
                (
                    Identifier::parse("minecraft:is_deep_ocean").unwrap(),
                    vec![Identifier::parse("minecraft:deep_ocean").unwrap()],
                ),
            ]),
        );

        let rules = BiomeRules::from_worldgen_data(&data).expect("is_overworld tag is present");

        assert!(
            rules
                .overworld_ids()
                .contains(&Identifier::parse("minecraft:plains").unwrap())
        );
        assert!(
            rules
                .temperate_forest
                .contains(&Identifier::parse("minecraft:forest").unwrap())
        );
        assert!(
            rules
                .hot_dry
                .contains(&Identifier::parse("minecraft:badlands").unwrap())
        );
        assert!(
            rules
                .ocean
                .contains(&Identifier::parse("minecraft:ocean").unwrap())
        );
        assert!(
            rules
                .deep_ocean
                .contains(&Identifier::parse("minecraft:deep_ocean").unwrap())
        );
    }

    #[test]
    fn feature_layer_adds_caves_and_ores_without_cave_fluids() {
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
        for chunk in chunks {
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = chunk.pos.x * 16 + lx as i32;
                    let wz = chunk.pos.z * 16 + lz as i32;
                    let top = g.surface_height(wx, wz);
                    for y in (MIN_Y + 1)..top - CAVE_SURFACE_CLEARANCE {
                        match chunk.get_block(lx, y, lz) {
                            Some(BlockStateId(0)) => saw_cave_air = true,
                            Some(BlockStateId(7)) => saw_deepslate = true,
                            Some(BlockStateId(8..=13 | 15..=24)) => saw_ore = true,
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
    }
}
