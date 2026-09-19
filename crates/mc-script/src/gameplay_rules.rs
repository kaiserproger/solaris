//! The startup rule contract: what a plugin deployment contributes to a world's
//! worldgen and spawning.
//!
//! This module is independent of guest execution. The component host converts a
//! component's `rule-plan` answer into [`GameplayRules`] before opening the
//! world. Its representation is the compatibility boundary, not an artifact of
//! the guest runtime.
//!
//! The type is also the world-compatibility boundary: [`GameplayRules::contract_name`]
//! is persisted in the world contract and compared on every reopen, so two
//! deployments that resolve to the same rules must resolve to the same string.
//! That is why the fields, their order and their serialization attributes are
//! part of the contract and not an implementation detail.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The resolved startup rules of one plugin deployment.
///
/// A plan is a normalization, not a script: every category is optional, and a
/// plan that sets none is refused by [`GameplayRules::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct GameplayRules {
    #[serde(default)]
    pub spawning: Vec<BiomeSpawns>,
    #[serde(default)]
    pub trees: Vec<TreeRule>,
    pub clay: Option<ClayRule>,
    pub placement: Option<SpawnPlacement>,
}

/// The spawn groups one biome uses, replacing whatever the biome had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BiomeSpawns {
    pub biome: String,
    pub groups: BTreeMap<String, Vec<SpawnEntry>>,
}

impl BiomeSpawns {
    /// One biome's replacement spawn groups, keyed by group name.
    #[must_use]
    pub fn new(biome: impl Into<String>, groups: BTreeMap<String, Vec<SpawnEntry>>) -> Self {
        Self {
            biome: biome.into(),
            groups,
        }
    }
}

/// One entity's budget inside a spawn group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SpawnEntry {
    pub entity: String,
    pub min: u32,
    pub max: u32,
    pub weight: u32,
}

impl SpawnEntry {
    /// One entity's count range and selection weight in a spawn group.
    #[must_use]
    pub fn new(entity: impl Into<String>, min: u32, max: u32, weight: u32) -> Self {
        Self {
            entity: entity.into(),
            min,
            max,
            weight,
        }
    }
}

/// Tree placement for the biomes one declaration covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TreeRule {
    pub biomes: Vec<String>,
    /// Positive deterministic candidate divisor, not a distance.
    pub spacing: u64,
    pub density_threshold: f64,
}

impl TreeRule {
    /// One tree declaration covering `biomes` at `spacing` with
    /// `density_threshold`.
    #[must_use]
    pub fn new(biomes: Vec<String>, spacing: u64, density_threshold: f64) -> Self {
        Self {
            biomes,
            spacing,
            density_threshold,
        }
    }
}

/// Clay deposit dimensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ClayRule {
    pub rarity: u64,
    pub radius_min: u8,
    pub radius_max: u8,
    pub max_water_depth: u8,
}

impl ClayRule {
    /// One clay declaration: a candidate divisor and a bounded disk in a bounded
    /// water depth.
    #[must_use]
    pub fn new(rarity: u64, radius_min: u8, radius_max: u8, max_water_depth: u8) -> Self {
        Self {
            rarity,
            radius_min,
            radius_max,
            max_water_depth,
        }
    }
}

/// Candidate separation for village and terrain placements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SpawnPlacement {
    pub land_spacing: u8,
    pub water_attempts: u8,
    pub water_depth: u8,
}

impl SpawnPlacement {
    /// One placement declaration: land separation and the water search bounds.
    #[must_use]
    pub fn new(land_spacing: u8, water_attempts: u8, water_depth: u8) -> Self {
        Self {
            land_spacing,
            water_attempts,
            water_depth,
        }
    }
}

/// Why a resolved rule plan cannot be materialized.
///
/// One variant per production check in [`GameplayRules::validate`], so a caller
/// that has to report or fail closed on a refused plan can say which bound the
/// plan broke instead of quoting one opaque message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GameplayRulesError {
    /// The plan sets no category at all.
    NoCategory,
    /// More than 64 spawning or tree declarations.
    TooManyDeclarations,
    /// A spawning biome name is over-long or declared twice.
    InvalidSpawningBiome,
    /// A spawn group is not one of the four supported names, or holds more than
    /// 32 entries.
    UnsupportedSpawnGroup,
    /// A spawn entry is over-long, duplicated, or outside `1 <= min <= max <= 6`
    /// and `1..10000` weight.
    InvalidSpawnEntry,
    /// A tree declaration has no biomes, too many, a zero spacing, or a
    /// non-finite threshold outside `-1..=1`.
    InvalidTreeRule,
    /// A tree biome name is over-long, or an earlier declaration already named
    /// it.
    InvalidTreeBiome,
    /// A clay rule exceeds the bounded deposit dimensions.
    ClayOutOfBounds,
    /// A spawn placement exceeds the bounded search dimensions.
    PlacementOutOfBounds,
}

impl GameplayRulesError {
    /// The operator-facing message.
    ///
    /// One wording for the component contract: the same plan always produces
    /// the same refusal, independent of the package that declared it.
    #[must_use]
    pub fn message(self) -> &'static str {
        match self {
            Self::NoCategory => "rule plan must define spawning, trees, clay, or placement",
            Self::TooManyDeclarations => "rule plan exceeds 64 biome declarations",
            Self::InvalidSpawningBiome => "invalid or duplicate spawning biome",
            Self::UnsupportedSpawnGroup => "unsupported spawn group or more than 32 entries",
            Self::InvalidSpawnEntry => "invalid, duplicate, or over-budget spawn entry",
            Self::InvalidTreeRule => "invalid tree rule",
            Self::InvalidTreeBiome => "invalid or duplicate tree biome",
            Self::ClayOutOfBounds => "clay rule exceeds bounded deposit dimensions",
            Self::PlacementOutOfBounds => "spawn placement exceeds bounded search dimensions",
        }
    }
}

impl std::fmt::Display for GameplayRulesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for GameplayRulesError {}

impl GameplayRules {
    /// Assemble a plan from its categories.
    ///
    /// The fields are `#[non_exhaustive]` so that adding a category cannot break a
    /// consumer's pattern match, which means another crate - the component host
    /// converting a `rule-plan` - needs this constructor. It does not validate:
    /// [`GameplayRules::validate`] is the only validator, and a caller that
    /// assembled a plan constructs then validates it.
    #[must_use]
    pub fn new(
        spawning: Vec<BiomeSpawns>,
        trees: Vec<TreeRule>,
        clay: Option<ClayRule>,
        placement: Option<SpawnPlacement>,
    ) -> Self {
        Self {
            spawning,
            trees,
            clay,
            placement,
        }
    }

    /// The world contract's fingerprint of these rules.
    ///
    /// Sha256 over canonical TOML. The `component-rules` prefix identifies the
    /// component-only deployment contract; changing its hashing or serialized
    /// field set changes the persisted world contract and therefore rejects an
    /// incompatible reopen.
    #[must_use]
    pub fn contract_name(&self) -> String {
        let canonical = toml::to_string(self).expect("validated rule plan is serializable");
        format!("component-rules:{:x}", Sha256::digest(canonical.as_bytes()))
    }

    /// Whether these rules can be materialized.
    ///
    /// Every category, list, name and numeric bound a world-side owner relies on
    /// is checked here, so a plan that reaches materialization carries only
    /// values the native owners can bound. The component host runs this before
    /// materializing the deployment.
    pub fn validate(&self) -> Result<(), GameplayRulesError> {
        if self.spawning.is_empty()
            && self.trees.is_empty()
            && self.clay.is_none()
            && self.placement.is_none()
        {
            return Err(GameplayRulesError::NoCategory);
        }
        if self.spawning.len() > 64 || self.trees.len() > 64 {
            return Err(GameplayRulesError::TooManyDeclarations);
        }
        let mut spawn_biomes = BTreeSet::new();
        for rule in &self.spawning {
            if rule.biome.len() > 128 || !spawn_biomes.insert(&rule.biome) {
                return Err(GameplayRulesError::InvalidSpawningBiome);
            }
            for (group, entries) in &rule.groups {
                if !matches!(
                    group.as_str(),
                    "creature" | "monster" | "water_ambient" | "water_creature"
                ) || entries.len() > 32
                {
                    return Err(GameplayRulesError::UnsupportedSpawnGroup);
                }
                let mut entities = BTreeSet::new();
                for entry in entries {
                    if entry.entity.len() > 128
                        || !entities.insert(&entry.entity)
                        || entry.min == 0
                        || entry.min > entry.max
                        || entry.max > 6
                        || entry.weight == 0
                        || entry.weight > 10_000
                    {
                        return Err(GameplayRulesError::InvalidSpawnEntry);
                    }
                }
            }
        }
        let mut tree_biomes = BTreeSet::new();
        for rule in &self.trees {
            if rule.biomes.is_empty()
                || rule.biomes.len() > 64
                || rule.spacing == 0
                || !rule.density_threshold.is_finite()
                || !(-1.0..=1.0).contains(&rule.density_threshold)
            {
                return Err(GameplayRulesError::InvalidTreeRule);
            }
            for biome in &rule.biomes {
                if biome.len() > 128 || !tree_biomes.insert(biome) {
                    return Err(GameplayRulesError::InvalidTreeBiome);
                }
            }
        }
        if let Some(rule) = &self.clay
            && (rule.rarity == 0
                || rule.radius_min == 0
                || rule.radius_min > rule.radius_max
                || rule.radius_max > 3
                || !(1..=32).contains(&rule.max_water_depth))
        {
            return Err(GameplayRulesError::ClayOutOfBounds);
        }
        if let Some(rule) = &self.placement
            && (!(1..=4).contains(&rule.land_spacing)
                || !(1..=32).contains(&rule.water_attempts)
                || !(1..=16).contains(&rule.water_depth))
        {
            return Err(GameplayRulesError::PlacementOutOfBounds);
        }
        Ok(())
    }
}
