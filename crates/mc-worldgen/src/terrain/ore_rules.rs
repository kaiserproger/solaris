use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;
use mc_data::worldgen_ores::{HeightAnchor, OreFeature, OrePlacementCount, OreTarget};
use mc_world::chunk::ChunkGeometry;
use mc_world::{BlockRegistry, BlockStateId};

use super::{biome_rules::BiomeRules, resolve_block_or};

// Candidate cells preserve the old per-block density without scanning every
// world voxel. The hard caps fence malformed sidecars and keep work and stack
// storage bounded per generated chunk.
pub(super) const ORE_ANCHOR_CELL_EDGE: i32 = 4;
pub(super) const ORE_ANCHOR_CELL_VOLUME: u64 = 64;
pub(super) const MAX_ORE_ANCHORS_PER_CELL: usize = 16;
pub(super) const MAX_ORE_VEIN_SIZE: usize = 64;
/// Maximum number of sidecar ore features admitted into chunk generation.
pub const MAX_ORE_RULES: usize = 64;
/// Maximum estimated ore scan and vein-cell work for one generated chunk.
pub const MAX_ORE_WORK_UNITS_PER_CHUNK: u64 = 2_000_000;
const ORE_CHUNK_HALO_CELLS_PER_LAYER: u64 = 36;

#[derive(Clone, Copy)]
enum EmbeddedOreScope {
    Any,
    EmeraldBiomes,
    Badlands,
}

const EMERALD_ORE_BIOMES: &[&str] = &[
    "minecraft:cherry_grove",
    "minecraft:frozen_peaks",
    "minecraft:grove",
    "minecraft:jagged_peaks",
    "minecraft:meadow",
    "minecraft:snowy_slopes",
    "minecraft:stony_peaks",
    "minecraft:windswept_forest",
    "minecraft:windswept_gravelly_hills",
    "minecraft:windswept_hills",
];

const EXTRA_GOLD_BIOMES: &[&str] = &[
    "minecraft:badlands",
    "minecraft:eroded_badlands",
    "minecraft:wooded_badlands",
];

#[derive(Clone, Copy)]
enum EmbeddedOreDistribution {
    Uniform,
    Trapezoid,
}

#[derive(Clone, Copy)]
struct EmbeddedOrePass {
    placed_feature: &'static str,
    normal: &'static str,
    deepslate: &'static str,
    min: HeightAnchor,
    max: HeightAnchor,
    attempts_numerator: u32,
    attempts_denominator: u32,
    distribution: EmbeddedOreDistribution,
    size: u32,
    discard_chance_on_air_exposure: f64,
    scope: EmbeddedOreScope,
}

// Compact facts extracted from the vanilla 26.1.2 placed/configured features.
// Separate passes matter: merging them changes both height peaks and abundance.
const VANILLA_OVERWORLD_ORE_PASSES: &[EmbeddedOrePass] = &[
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_coal_upper",
        normal: "minecraft:coal_ore",
        deepslate: "minecraft:deepslate_coal_ore",
        min: HeightAnchor::Absolute(136),
        max: HeightAnchor::BelowTop(0),
        attempts_numerator: 30,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 17,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_coal_lower",
        normal: "minecraft:coal_ore",
        deepslate: "minecraft:deepslate_coal_ore",
        min: HeightAnchor::Absolute(0),
        max: HeightAnchor::Absolute(192),
        attempts_numerator: 20,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 17,
        discard_chance_on_air_exposure: 0.5,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_iron_upper",
        normal: "minecraft:iron_ore",
        deepslate: "minecraft:deepslate_iron_ore",
        min: HeightAnchor::Absolute(80),
        max: HeightAnchor::Absolute(384),
        attempts_numerator: 90,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 9,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_iron_middle",
        normal: "minecraft:iron_ore",
        deepslate: "minecraft:deepslate_iron_ore",
        min: HeightAnchor::Absolute(-24),
        max: HeightAnchor::Absolute(56),
        attempts_numerator: 10,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 9,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_iron_small",
        normal: "minecraft:iron_ore",
        deepslate: "minecraft:deepslate_iron_ore",
        min: HeightAnchor::AboveBottom(0),
        max: HeightAnchor::Absolute(72),
        attempts_numerator: 10,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 4,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_gold",
        normal: "minecraft:gold_ore",
        deepslate: "minecraft:deepslate_gold_ore",
        min: HeightAnchor::Absolute(-64),
        max: HeightAnchor::Absolute(32),
        attempts_numerator: 4,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 9,
        discard_chance_on_air_exposure: 0.5,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_gold_lower",
        normal: "minecraft:gold_ore",
        deepslate: "minecraft:deepslate_gold_ore",
        min: HeightAnchor::Absolute(-64),
        max: HeightAnchor::Absolute(-48),
        attempts_numerator: 1,
        attempts_denominator: 2,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 9,
        discard_chance_on_air_exposure: 0.5,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_redstone",
        normal: "minecraft:redstone_ore",
        deepslate: "minecraft:deepslate_redstone_ore",
        min: HeightAnchor::AboveBottom(0),
        max: HeightAnchor::Absolute(15),
        attempts_numerator: 4,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 8,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_redstone_lower",
        normal: "minecraft:redstone_ore",
        deepslate: "minecraft:deepslate_redstone_ore",
        min: HeightAnchor::AboveBottom(-32),
        max: HeightAnchor::AboveBottom(32),
        attempts_numerator: 8,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 8,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_diamond",
        normal: "minecraft:diamond_ore",
        deepslate: "minecraft:deepslate_diamond_ore",
        min: HeightAnchor::AboveBottom(-80),
        max: HeightAnchor::AboveBottom(80),
        attempts_numerator: 7,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 4,
        discard_chance_on_air_exposure: 0.5,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_diamond_medium",
        normal: "minecraft:diamond_ore",
        deepslate: "minecraft:deepslate_diamond_ore",
        min: HeightAnchor::Absolute(-64),
        max: HeightAnchor::Absolute(-4),
        attempts_numerator: 2,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 8,
        discard_chance_on_air_exposure: 0.5,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_diamond_large",
        normal: "minecraft:diamond_ore",
        deepslate: "minecraft:deepslate_diamond_ore",
        min: HeightAnchor::AboveBottom(-80),
        max: HeightAnchor::AboveBottom(80),
        attempts_numerator: 1,
        attempts_denominator: 9,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 12,
        discard_chance_on_air_exposure: 0.7,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_diamond_buried",
        normal: "minecraft:diamond_ore",
        deepslate: "minecraft:deepslate_diamond_ore",
        min: HeightAnchor::AboveBottom(-80),
        max: HeightAnchor::AboveBottom(80),
        attempts_numerator: 4,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 8,
        discard_chance_on_air_exposure: 1.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_lapis",
        normal: "minecraft:lapis_ore",
        deepslate: "minecraft:deepslate_lapis_ore",
        min: HeightAnchor::Absolute(-32),
        max: HeightAnchor::Absolute(32),
        attempts_numerator: 2,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 7,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_lapis_buried",
        normal: "minecraft:lapis_ore",
        deepslate: "minecraft:deepslate_lapis_ore",
        min: HeightAnchor::AboveBottom(0),
        max: HeightAnchor::Absolute(64),
        attempts_numerator: 4,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 7,
        discard_chance_on_air_exposure: 1.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_copper",
        normal: "minecraft:copper_ore",
        deepslate: "minecraft:deepslate_copper_ore",
        min: HeightAnchor::Absolute(-16),
        max: HeightAnchor::Absolute(112),
        attempts_numerator: 16,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 10,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Any,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_emerald",
        normal: "minecraft:emerald_ore",
        deepslate: "minecraft:deepslate_emerald_ore",
        min: HeightAnchor::Absolute(-16),
        max: HeightAnchor::Absolute(480),
        attempts_numerator: 100,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Trapezoid,
        size: 3,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::EmeraldBiomes,
    },
    EmbeddedOrePass {
        placed_feature: "minecraft:ore_gold_extra",
        normal: "minecraft:gold_ore",
        deepslate: "minecraft:deepslate_gold_ore",
        min: HeightAnchor::Absolute(32),
        max: HeightAnchor::Absolute(256),
        attempts_numerator: 50,
        attempts_denominator: 1,
        distribution: EmbeddedOreDistribution::Uniform,
        size: 9,
        discard_chance_on_air_exposure: 0.0,
        scope: EmbeddedOreScope::Badlands,
    },
];

#[derive(Debug, Clone)]
pub struct OreRules {
    rules: Vec<OreRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OreRulesError {
    #[error("ore rules contain {provided} entries; maximum is {max}")]
    TooManyRules { provided: usize, max: usize },
    #[error("ore rules require {required} chunk-work units; maximum is {max}")]
    ChunkWorkBudgetExceeded { required: u64, max: u64 },
}

impl OreRules {
    pub fn new(rules: Vec<OreRule>) -> Result<Self, OreRulesError> {
        if rules.len() > MAX_ORE_RULES {
            return Err(OreRulesError::TooManyRules {
                provided: rules.len(),
                max: MAX_ORE_RULES,
            });
        }
        let required = rules.iter().fold(0_u64, |total, rule| {
            total.saturating_add(ore_rule_chunk_work(rule))
        });
        if required > MAX_ORE_WORK_UNITS_PER_CHUNK {
            return Err(OreRulesError::ChunkWorkBudgetExceeded {
                required,
                max: MAX_ORE_WORK_UNITS_PER_CHUNK,
            });
        }
        Ok(Self { rules })
    }

    #[must_use]
    pub fn solaris_default(
        registry: &BlockRegistry,
        _biomes: &BiomeRules,
        fallback: BlockStateId,
    ) -> Self {
        let state = |name: &str| resolve_block_or(registry, name, fallback);
        let geometry = mc_world::chunk::OVERWORLD_GEOMETRY;
        let rules = VANILLA_OVERWORLD_ORE_PASSES
            .iter()
            .filter_map(|pass| {
                debug_assert!(pass.placed_feature.starts_with("minecraft:ore_"));
                let (y, raw_min, raw_max) = resolve_height_range(pass.min, pass.max, geometry)?;
                let spacing = spacing_for_attempts(
                    raw_min,
                    raw_max,
                    pass.attempts_numerator,
                    pass.attempts_denominator,
                    pass.size,
                    matches!(pass.distribution, EmbeddedOreDistribution::Trapezoid),
                );
                let scope = match pass.scope {
                    EmbeddedOreScope::Any => BiomeScope::Any,
                    EmbeddedOreScope::EmeraldBiomes => {
                        BiomeScope::only(embedded_biomes(EMERALD_ORE_BIOMES))
                    }
                    EmbeddedOreScope::Badlands => {
                        BiomeScope::only(embedded_biomes(EXTRA_GOLD_BIOMES))
                    }
                };
                Some(OreRule {
                    normal: state(pass.normal),
                    deepslate: state(pass.deepslate),
                    y,
                    spacing,
                    biomes: scope,
                    size: pass.size,
                    discard_chance_on_air_exposure: pass.discard_chance_on_air_exposure,
                })
            })
            .collect();
        Self::new(rules).expect("embedded ore rules fit the admission budget")
    }

    /// Converts sidecar features into bounded generation rules.
    ///
    /// The complete input is rejected when it exceeds an admission limit;
    /// rules are never silently truncated.
    pub fn from_features(
        registry: &BlockRegistry,
        biomes: &BiomeRules,
        features: &[OreFeature],
        biome_data: Option<&BiomeWorldgenData>,
        geometry: ChunkGeometry,
    ) -> Result<Option<Self>, OreRulesError> {
        if features.len() > MAX_ORE_RULES {
            return Err(OreRulesError::TooManyRules {
                provided: features.len(),
                max: MAX_ORE_RULES,
            });
        }
        let mut rules = Vec::with_capacity(features.len());
        for feature in features {
            let Some((normal, deepslate)) = ore_targets(registry, &feature.targets) else {
                continue;
            };
            let Some(height) = &feature.placement.height else {
                continue;
            };
            let Some((y, raw_min, raw_max)) =
                resolve_height_range(height.min, height.max, geometry)
            else {
                continue;
            };
            let spacing = ore_spacing(&feature.placement, raw_min, raw_max, feature.size);
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
                size: feature.size,
                discard_chance_on_air_exposure: feature.discard_chance_on_air_exposure,
            });
        }
        if rules.is_empty() {
            Ok(None)
        } else {
            Self::new(rules).map(Some)
        }
    }

    #[must_use]
    pub fn rules(&self) -> &[OreRule] {
        &self.rules
    }
}

fn ore_rule_chunk_work(rule: &OreRule) -> u64 {
    if rule.y.min > rule.y.max {
        return 0;
    }
    let min_cell_y = i64::from(rule.y.min).div_euclid(i64::from(ORE_ANCHOR_CELL_EDGE));
    let max_cell_y = i64::from(rule.y.max).div_euclid(i64::from(ORE_ANCHOR_CELL_EDGE));
    let y_cell_count = u64::try_from(max_cell_y - min_cell_y + 1).unwrap_or(u64::MAX);
    let vein_size = u64::from(rule.size.clamp(1, MAX_ORE_VEIN_SIZE as u32));
    let denominator = rule.spacing.minimum().max(1).saturating_mul(vein_size);
    let anchors_per_cell = ORE_ANCHOR_CELL_VOLUME
        .div_ceil(denominator)
        .min(MAX_ORE_ANCHORS_PER_CELL as u64);
    let work_per_cell = 1_u64.saturating_add(anchors_per_cell.saturating_mul(vein_size));
    y_cell_count
        .saturating_mul(ORE_CHUNK_HALO_CELLS_PER_LAYER)
        .saturating_mul(work_per_cell)
}

#[derive(Debug, Clone)]
pub struct OreRule {
    pub normal: BlockStateId,
    pub deepslate: BlockStateId,
    pub y: YRange,
    pub spacing: OreSpacing,
    pub biomes: BiomeScope,
    pub size: u32,
    pub discard_chance_on_air_exposure: f64,
}

impl OreRule {
    #[cfg(test)]
    pub(super) fn matches(&self, h: u64, y: i32, biome: &Identifier) -> bool {
        (self.y.min..=self.y.max).contains(&y)
            && self.biomes.matches(biome)
            && h.is_multiple_of(self.spacing.at_y(y, self.y))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct YRange {
    pub min: i32,
    pub max: i32,
}

impl YRange {
    #[must_use]
    pub const fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum OreSpacing {
    Fixed(u64),
    Peaked {
        peak_y: i32,
        min_spacing: u64,
        range: u64,
    },
    Trapezoid {
        raw_min: i64,
        raw_max: i64,
        average_spacing: u64,
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

    pub(super) fn at_y(&self, y: i32, range: YRange) -> u64 {
        match *self {
            Self::Fixed(spacing) => spacing,
            Self::Peaked {
                peak_y,
                min_spacing,
                range: spacing_range,
            } => peaked_spacing(y, range.min, range.max, peak_y, min_spacing, spacing_range),
            Self::Trapezoid {
                raw_min,
                raw_max,
                average_spacing,
            } => trapezoid_spacing(y, raw_min, raw_max, average_spacing),
        }
    }

    fn minimum(self) -> u64 {
        match self {
            Self::Fixed(spacing) => spacing,
            Self::Peaked { min_spacing, .. } => min_spacing,
            Self::Trapezoid {
                raw_min,
                raw_max,
                average_spacing,
            } => {
                let midpoint = raw_min.saturating_add(raw_max.saturating_sub(raw_min) / 2);
                let midpoint = i32::try_from(midpoint).unwrap_or_else(|_| {
                    if midpoint.is_negative() {
                        i32::MIN
                    } else {
                        i32::MAX
                    }
                });
                trapezoid_spacing(midpoint, raw_min, raw_max, average_spacing)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum BiomeScope {
    Any,
    Only(Vec<Identifier>),
}

impl BiomeScope {
    #[must_use]
    pub fn only(biomes: Vec<Identifier>) -> Self {
        Self::Only(biomes)
    }

    pub(super) fn matches(&self, biome: &Identifier) -> bool {
        match self {
            Self::Any => true,
            Self::Only(biomes) => biomes.contains(biome),
        }
    }
}

fn embedded_biomes(names: &[&str]) -> Vec<Identifier> {
    names
        .iter()
        .map(|name| Identifier::parse(*name).expect("embedded biome identifier"))
        .collect()
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

fn resolve_height_range(
    min: HeightAnchor,
    max: HeightAnchor,
    geometry: ChunkGeometry,
) -> Option<(YRange, i64, i64)> {
    let raw_min = height_anchor_y(min, geometry);
    let raw_max = height_anchor_y(max, geometry);
    if raw_min > raw_max {
        return None;
    }
    let clipped_min = raw_min.max(i64::from(geometry.min_y()));
    let clipped_max = raw_max.min(i64::from(geometry.max_y()) - 1);
    if clipped_min > clipped_max {
        return None;
    }
    Some((
        YRange::new(
            i32::try_from(clipped_min).ok()?,
            i32::try_from(clipped_max).ok()?,
        ),
        raw_min,
        raw_max,
    ))
}

fn height_anchor_y(anchor: HeightAnchor, geometry: ChunkGeometry) -> i64 {
    match anchor {
        HeightAnchor::Absolute(y) => i64::from(y),
        HeightAnchor::AboveBottom(offset) => i64::from(geometry.min_y()) + i64::from(offset),
        HeightAnchor::BelowTop(offset) => i64::from(geometry.max_y()) - 1 - i64::from(offset),
    }
}

fn ore_spacing(
    placement: &mc_data::worldgen_ores::OrePlacement,
    raw_min: i64,
    raw_max: i64,
    size: u32,
) -> OreSpacing {
    let (attempts_numerator, mut attempts_denominator): (u64, u64) = match placement.count {
        Some(OrePlacementCount::Constant(count)) => (u64::from(count), 1),
        Some(OrePlacementCount::Uniform { min, max }) => {
            (u64::from(min).saturating_add(u64::from(max)), 2)
        }
        None => (1, 1),
    };
    if let Some(chance) = placement.rarity_chance {
        attempts_denominator = attempts_denominator.saturating_mul(u64::from(chance.max(1)));
    }
    spacing_for_attempts(
        raw_min,
        raw_max,
        u32::try_from(attempts_numerator).unwrap_or(u32::MAX),
        u32::try_from(attempts_denominator).unwrap_or(u32::MAX),
        size,
        placement
            .height
            .as_ref()
            .is_some_and(|height| height.kind.as_str() == "minecraft:trapezoid"),
    )
}

fn spacing_for_attempts(
    raw_min: i64,
    raw_max: i64,
    attempts_numerator: u32,
    attempts_denominator: u32,
    size: u32,
    trapezoid: bool,
) -> OreSpacing {
    let raw_height = u64::try_from(raw_max.saturating_sub(raw_min).saturating_add(1))
        .unwrap_or(u64::MAX)
        .max(1);
    let numerator = raw_height
        .saturating_mul(256)
        .saturating_mul(u64::from(attempts_denominator.max(1)));
    let expected_blocks = u64::from(attempts_numerator)
        .saturating_mul(u64::from(size.max(1)))
        .max(1);
    let uniform_spacing = numerator.div_ceil(expected_blocks).max(5);
    if trapezoid {
        OreSpacing::Trapezoid {
            raw_min,
            raw_max,
            average_spacing: uniform_spacing,
        }
    } else {
        OreSpacing::Fixed(uniform_spacing)
    }
}

fn trapezoid_spacing(y: i32, raw_min: i64, raw_max: i64, average_spacing: u64) -> u64 {
    if raw_min > raw_max {
        return u64::MAX;
    }
    let y = i64::from(y).clamp(raw_min, raw_max);
    let levels = u128::try_from(raw_max.saturating_sub(raw_min).saturating_add(1))
        .unwrap_or(u128::MAX)
        .max(1);
    let index = u128::try_from(y.saturating_sub(raw_min)).unwrap_or(0);
    let weight = (index + 1).min(levels.saturating_sub(index)).max(1);
    let half = levels / 2;
    let weight_sum = if levels.is_multiple_of(2) {
        half.saturating_mul(half.saturating_add(1))
    } else {
        half.saturating_add(1).saturating_pow(2)
    };
    u64::try_from(
        u128::from(average_spacing)
            .saturating_mul(weight_sum)
            .div_ceil(levels.saturating_mul(weight)),
    )
    .unwrap_or(u64::MAX)
    .max(1)
}

fn peaked_spacing(
    y: i32,
    min_y: i32,
    max_y: i32,
    peak_y: i32,
    min_spacing: u64,
    range: u64,
) -> u64 {
    let distance_from_min = (i64::from(peak_y) - i64::from(min_y)).unsigned_abs();
    let distance_from_max = (i64::from(max_y) - i64::from(peak_y)).unsigned_abs();
    let max_distance = distance_from_min.max(distance_from_max).max(1) as f64;
    let distance = (i64::from(y) - i64::from(peak_y)).unsigned_abs() as f64 / max_distance;
    min_spacing.saturating_add((distance * range as f64).round() as u64)
}

#[cfg(test)]
#[path = "ore_rules_tests.rs"]
mod tests;
