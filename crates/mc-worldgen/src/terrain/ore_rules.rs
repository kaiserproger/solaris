use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;
use mc_data::worldgen_ores::{HeightAnchor, OreFeature, OrePlacementCount, OreTarget};
use mc_world::chunk::{MAX_Y, MIN_Y};
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
        biomes: &BiomeRules,
        fallback: BlockStateId,
    ) -> Self {
        let state = |name: &str| resolve_block_or(registry, name, fallback);
        // Coarse family sizes for embedded operation; this is not Mojang's
        // placement algorithm and does not imply vanilla worldgen parity.
        Self::new(vec![
            OreRule {
                normal: state("minecraft:emerald_ore"),
                deepslate: state("minecraft:deepslate_emerald_ore"),
                y: YRange::new(EMERALD_MIN_Y, EMERALD_MAX_Y),
                spacing: OreSpacing::peaked(224, 130, 260),
                biomes: BiomeScope::only(biomes.mountain.clone()),
                size: 3,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:gold_ore"),
                deepslate: state("minecraft:deepslate_gold_ore"),
                y: YRange::new(GOLD_MIN_Y, 112),
                spacing: OreSpacing::Fixed(58),
                biomes: BiomeScope::only(biomes.hot_dry.clone()),
                size: 9,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:diamond_ore"),
                deepslate: state("minecraft:deepslate_diamond_ore"),
                y: YRange::new(DIAMOND_MIN_Y, DIAMOND_MAX_Y),
                spacing: OreSpacing::peaked(-56, 210, 380),
                biomes: BiomeScope::Any,
                size: 8,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:redstone_ore"),
                deepslate: state("minecraft:deepslate_redstone_ore"),
                y: YRange::new(REDSTONE_MIN_Y, REDSTONE_MAX_Y),
                spacing: OreSpacing::peaked(-48, 95, 115),
                biomes: BiomeScope::Any,
                size: 8,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:lapis_ore"),
                deepslate: state("minecraft:deepslate_lapis_ore"),
                y: YRange::new(LAPIS_MIN_Y, LAPIS_MAX_Y),
                spacing: OreSpacing::peaked(0, 150, 210),
                biomes: BiomeScope::Any,
                size: 7,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:gold_ore"),
                deepslate: state("minecraft:deepslate_gold_ore"),
                y: YRange::new(GOLD_MIN_Y, GOLD_MAX_Y),
                spacing: OreSpacing::peaked(-16, 105, 160),
                biomes: BiomeScope::Any,
                size: 9,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:iron_ore"),
                deepslate: state("minecraft:deepslate_iron_ore"),
                y: YRange::new(IRON_MIN_Y, IRON_MAX_Y),
                spacing: OreSpacing::peaked(16, 97, 140),
                biomes: BiomeScope::Any,
                size: 9,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:copper_ore"),
                deepslate: state("minecraft:deepslate_copper_ore"),
                y: YRange::new(COPPER_MIN_Y, COPPER_MAX_Y),
                spacing: OreSpacing::peaked(48, 89, 130),
                biomes: BiomeScope::Any,
                size: 10,
                discard_chance_on_air_exposure: 0.0,
            },
            OreRule {
                normal: state("minecraft:coal_ore"),
                deepslate: state("minecraft:deepslate_coal_ore"),
                y: YRange::new(COAL_MIN_Y, COAL_MAX_Y),
                spacing: OreSpacing::peaked(96, 83, 120),
                biomes: BiomeScope::Any,
                size: 17,
                discard_chance_on_air_exposure: 0.0,
            },
        ])
        .expect("embedded ore rules fit the admission budget")
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
        }
    }

    const fn minimum(self) -> u64 {
        match self {
            Self::Fixed(spacing) => spacing,
            Self::Peaked { min_spacing, .. } => min_spacing,
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

#[cfg(test)]
#[path = "ore_rules_tests.rs"]
mod tests;
