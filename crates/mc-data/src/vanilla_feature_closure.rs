//! Vanilla placed-feature closure reader.
//!
//! This resolves vanilla worldgen JSON **by reference**, starting from entries
//! the caller names (a placed feature id, or the `feature_pool_element` entries
//! of a structure template pool) and following `placed_feature` →
//! `configured_feature` → state providers, placement modifiers, int providers
//! and block predicates down to their leaves. It loads only that closure:
//! entries that are not transitively reachable are never read, so the parts of
//! the vanilla registries Solaris does not implement can never fail a load.
//!
//! Anything the closure *does* reach that Solaris does not implement fails
//! closed ([`ClosureError::UnsupportedType`]), naming the type id and the entry
//! that referenced it. The reader executes nothing; the typed specs it returns
//! are compiled and placed by `mc-worldgen`.
//!
//! ## Checkpoint A1 partial layer
//!
//! The implemented surface is exactly the reachable village decor closure
//! covered so far:
//!
//! - configured features `minecraft:simple_block`, `minecraft:block_pile`,
//!   `minecraft:block_column`, `minecraft:tree` (checkpoint A2: the four
//!   reachable trunk/foliage placer pairs, an empty decorator list, and the
//!   `two_layers_feature_size` minimum size),
//! - state providers `simple`, `weighted`, `rotated`, `noise_threshold`,
//!   `rule_based`.
//! - placement modifiers `count`, `random_offset`, `block_predicate_filter`.
//! - block predicates `would_survive`, `matching_block_tag`, `matching_blocks`,
//!   `all_of`, `not`.
//! - int providers `constant`, `uniform`, `trapezoid`, `biased_to_bottom`,
//!   `weighted_list`.
//!
//! Nothing outside that surface is accepted, and nothing here runs on an
//! ordinary server start: this module is exercised by tests and will be used by
//! checkpoint B when village assembly is wired.

use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;

use crate::{Identifier, ResourcePath, ResourcePathError, read_json_resource};

/// How a reference is named in error messages, e.g. "configured feature".
type EntryKind = &'static str;

const PLACED_FEATURE: EntryKind = "placed_feature";
const CONFIGURED_FEATURE: EntryKind = "configured_feature";
const TEMPLATE_POOL: EntryKind = "template_pool";
const STATE_PROVIDER: EntryKind = "state provider";
const PLACEMENT_MODIFIER: EntryKind = "placement modifier";
const BLOCK_PREDICATE: EntryKind = "block predicate";
const INT_PROVIDER: EntryKind = "int provider";
const TRUNK_PLACER: EntryKind = "trunk placer";
const FOLIAGE_PLACER: EntryKind = "foliage placer";
const FEATURE_SIZE: EntryKind = "feature size";
const TREE_DECORATOR: EntryKind = "tree decorator";
const ROOT_PLACER: EntryKind = "root placer";
const POOL_ELEMENT: EntryKind = "template pool element";

#[derive(Debug, Error)]
pub enum ClosureError {
    #[error("feature closure io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("feature closure parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} in {path}")]
    InvalidIdentifier { path: PathBuf, value: String },
    #[error(transparent)]
    ResourcePath(#[from] ResourcePathError),
    #[error("missing {kind} {id} referenced by {referrer} at {path}")]
    MissingEntry {
        kind: EntryKind,
        id: Identifier,
        referrer: Identifier,
        path: PathBuf,
    },
    #[error("non-minecraft {kind} {id} referenced by {referrer} is outside the vanilla closure")]
    UnsupportedNamespace {
        kind: EntryKind,
        id: Identifier,
        referrer: Identifier,
    },
    #[error("unsupported {kind} type {type_id} referenced by {referrer}")]
    UnsupportedType {
        kind: EntryKind,
        type_id: Identifier,
        referrer: Identifier,
    },
    #[error("{kind} {entry} is missing field {field}")]
    MissingField {
        kind: EntryKind,
        entry: Identifier,
        field: &'static str,
    },
    #[error("{kind} {entry} has invalid value for {field}: {value}")]
    InvalidField {
        kind: EntryKind,
        entry: Identifier,
        field: &'static str,
        value: String,
    },
}

/// A block state as written in worldgen JSON: block id plus explicit property
/// values. Resolution to a registry state id happens in `mc-worldgen`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockStateSpec {
    pub block: Identifier,
    pub properties: Vec<(String, String)>,
}

/// A placed feature: its configured feature plus the placement pipeline, in
/// application order.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedFeatureSpec {
    pub id: Identifier,
    pub feature: ConfiguredFeatureSpec,
    pub placement: Vec<PlacementModifierSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredFeatureSpec {
    pub id: Identifier,
    pub kind: ConfiguredFeatureKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfiguredFeatureKind {
    SimpleBlock {
        to_place: StateProviderSpec,
        schedule_tick: bool,
    },
    BlockPile {
        state_provider: StateProviderSpec,
    },
    BlockColumn {
        layers: Vec<BlockColumnLayer>,
        direction: ColumnDirection,
        allowed_placement: BlockPredicateSpec,
        prioritize_tip: bool,
    },
    Tree(Box<TreeSpec>),
}

/// `minecraft:tree` configuration, restricted to what the village closure
/// reaches.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeSpec {
    pub trunk_provider: StateProviderSpec,
    pub trunk_placer: TrunkPlacerSpec,
    pub foliage_provider: StateProviderSpec,
    pub foliage_placer: FoliagePlacerSpec,
    pub minimum_size: TwoLayersFeatureSize,
    /// `Codec.BOOL.fieldOf("ignore_vines").orElse(false)`.
    pub ignore_vines: bool,
    /// `below_trunk_provider`, defaulting to vanilla's
    /// `PLACE_BELOW_OVERWORLD_TRUNKS`.
    pub below_trunk_provider: StateProviderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrunkPlacerSpec {
    Straight(TrunkPlacerHeights),
    Forking(TrunkPlacerHeights),
}

/// `base_height`/`height_rand_a`/`height_rand_b`, the shared trunk placer parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrunkPlacerHeights {
    pub base_height: i32,
    pub height_rand_a: i32,
    pub height_rand_b: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FoliagePlacerSpec {
    Blob {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        height: i32,
    },
    Spruce {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        trunk_height: IntProviderSpec,
    },
    Pine {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
        height: IntProviderSpec,
    },
    Acacia {
        radius: IntProviderSpec,
        offset: IntProviderSpec,
    },
}

/// `minecraft:two_layers_feature_size` with its codec defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwoLayersFeatureSize {
    pub limit: i32,
    pub lower_size: i32,
    pub upper_size: i32,
    pub min_clipped_height: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockColumnLayer {
    pub height: IntProviderSpec,
    pub provider: StateProviderSpec,
}

/// `minecraft:block_column` direction. Only `up` is reachable from the village
/// decor closure today; the whole direction enum is modelled so the field
/// resolves from its own domain instead of failing on an unrelated value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnDirection {
    Up,
    Down,
    North,
    South,
    West,
    East,
}

impl ColumnDirection {
    #[must_use]
    pub fn offset(self) -> (i32, i32, i32) {
        match self {
            Self::Up => (0, 1, 0),
            Self::Down => (0, -1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "up" => Self::Up,
            "down" => Self::Down,
            "north" => Self::North,
            "south" => Self::South,
            "west" => Self::West,
            "east" => Self::East,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateProviderSpec {
    Simple {
        state: BlockStateSpec,
    },
    Weighted {
        entries: Vec<WeightedState>,
    },
    /// `minecraft:rotated_block_provider` keeps only the block: its codec maps
    /// the written state through `Block`, so written properties are dropped and
    /// the axis is drawn per placement.
    Rotated {
        block: Identifier,
    },
    NoiseThreshold(NoiseThresholdSpec),
    RuleBased {
        fallback: Option<Box<StateProviderSpec>>,
        rules: Vec<RuleBasedRule>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedState {
    pub state: BlockStateSpec,
    pub weight: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NoiseThresholdSpec {
    pub seed: i64,
    pub noise: NoiseParametersSpec,
    pub scale: f32,
    pub threshold: f32,
    pub high_chance: f32,
    pub default_state: BlockStateSpec,
    pub low_states: Vec<BlockStateSpec>,
    pub high_states: Vec<BlockStateSpec>,
}

/// `minecraft:noise` parameters as referenced by `noise_threshold_provider`.
#[derive(Debug, Clone, PartialEq)]
pub struct NoiseParametersSpec {
    pub first_octave: i32,
    pub amplitudes: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuleBasedRule {
    pub predicate: BlockPredicateSpec,
    pub then: StateProviderSpec,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlacementModifierSpec {
    Count(IntProviderSpec),
    RandomOffset {
        xz_spread: IntProviderSpec,
        y_spread: IntProviderSpec,
    },
    BlockPredicateFilter(BlockPredicateSpec),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IntProviderSpec {
    Constant(i32),
    Uniform {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    Trapezoid {
        min_inclusive: i32,
        max_inclusive: i32,
        plateau: i32,
    },
    BiasedToBottom {
        min_inclusive: i32,
        max_inclusive: i32,
    },
    WeightedList(Vec<WeightedInt>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeightedInt {
    pub provider: IntProviderSpec,
    pub weight: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPredicateSpec {
    WouldSurvive {
        offset: (i32, i32, i32),
        state: BlockStateSpec,
    },
    MatchingBlockTag {
        offset: (i32, i32, i32),
        tag: Identifier,
    },
    MatchingBlocks {
        offset: (i32, i32, i32),
        blocks: Vec<Identifier>,
    },
    AllOf {
        predicates: Vec<BlockPredicateSpec>,
    },
    Not {
        predicate: Box<BlockPredicateSpec>,
    },
}

/// One `feature_pool_element` of a structure template pool, resolved to its
/// placed feature.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolFeatureEntry {
    pub pool: Identifier,
    pub placed_feature: Identifier,
    pub weight: u32,
    pub spec: PlacedFeatureSpec,
}

/// Resolve-by-reference reader over a vanilla `worldgen` directory, e.g.
/// `<vanilla_data_dir>/data/minecraft/worldgen`.
pub struct FeatureClosure {
    worldgen_dir: PathBuf,
}

impl FeatureClosure {
    #[must_use]
    pub fn new(worldgen_dir: impl Into<PathBuf>) -> Self {
        Self {
            worldgen_dir: worldgen_dir.into(),
        }
    }

    /// Resolve one placed feature and everything it references.
    pub fn load_placed_feature(&self, id: &Identifier) -> Result<PlacedFeatureSpec, ClosureError> {
        self.load_placed_feature_from(id, id)
    }

    fn load_placed_feature_from(
        &self,
        id: &Identifier,
        referrer: &Identifier,
    ) -> Result<PlacedFeatureSpec, ClosureError> {
        let value = self.read_entry(PLACED_FEATURE, "placed_feature", id, referrer)?;
        let configured_id = parse_id(
            self.path(PLACED_FEATURE, id),
            field_str(&value, "feature", PLACED_FEATURE, id)?,
        )?;
        let configured_value =
            self.read_entry(CONFIGURED_FEATURE, "configured_feature", &configured_id, id)?;
        let feature = self.parse_configured_feature(&configured_id, &configured_value)?;
        let placement = match value.get("placement") {
            None => Vec::new(),
            Some(Value::Array(entries)) => entries
                .iter()
                .map(|entry| self.parse_placement_modifier(id, entry))
                .collect::<Result<Vec<_>, _>>()?,
            Some(other) => {
                return Err(ClosureError::InvalidField {
                    kind: PLACED_FEATURE,
                    entry: id.clone(),
                    field: "placement",
                    value: other.to_string(),
                });
            }
        };
        Ok(PlacedFeatureSpec {
            id: id.clone(),
            feature,
            placement,
        })
    }

    /// Resolve every `feature_pool_element` of a structure template pool.
    ///
    /// Elements that are not placed-feature elements (empty/legacy/single pool
    /// elements) belong to jigsaw assembly rather than to the feature closure
    /// and are skipped. A missing pool or feature file, an unreadable element or
    /// an unsupported reachable type fails closed.
    pub fn load_pool_features(
        &self,
        pool: &Identifier,
    ) -> Result<Vec<PoolFeatureEntry>, ClosureError> {
        let value = self.read_entry(TEMPLATE_POOL, "template_pool", pool, pool)?;
        let elements = field_array(&value, "elements", TEMPLATE_POOL, pool)?;

        let mut entries = Vec::new();
        for entry in elements {
            let element = field_of(entry, "element", TEMPLATE_POOL, pool)?;
            // Vanilla's pool codec requires `weight` in 1..=150 for every element.
            let weight = u32::try_from(i32_field(entry, "weight", TEMPLATE_POOL, pool)?)
                .ok()
                .filter(|weight| (1..=150).contains(weight))
                .ok_or_else(|| ClosureError::InvalidField {
                    kind: TEMPLATE_POOL,
                    entry: pool.clone(),
                    field: "weight",
                    value: entry
                        .get("weight")
                        .map(Value::to_string)
                        .unwrap_or_else(|| String::from("<missing>")),
                })?;
            match field_str(element, "element_type", TEMPLATE_POOL, pool)? {
                "minecraft:feature_pool_element" => {}
                // Jigsaw elements are assembly's business, not the feature
                // closure's.
                "minecraft:empty_pool_element"
                | "minecraft:single_pool_element"
                | "minecraft:legacy_single_pool_element" => continue,
                // A list element can wrap feature elements, so skipping it
                // would silently drop closure entries.
                other => {
                    return Err(ClosureError::UnsupportedType {
                        kind: POOL_ELEMENT,
                        type_id: parse_id(self.path(TEMPLATE_POOL, pool), other)?,
                        referrer: pool.clone(),
                    });
                }
            }
            let placed_feature = parse_id(
                self.path(TEMPLATE_POOL, pool),
                field_str(element, "feature", TEMPLATE_POOL, pool)?,
            )?;
            let spec = self.load_placed_feature_from(&placed_feature, pool)?;
            entries.push(PoolFeatureEntry {
                pool: pool.clone(),
                placed_feature,
                weight,
                spec,
            });
        }
        Ok(entries)
    }

    fn path(&self, dir: &str, id: &Identifier) -> PathBuf {
        self.worldgen_dir.join(dir).join(id.path())
    }

    fn read_entry(
        &self,
        kind: EntryKind,
        dir: &str,
        id: &Identifier,
        referrer: &Identifier,
    ) -> Result<Value, ClosureError> {
        if id.namespace() != "minecraft" {
            return Err(ClosureError::UnsupportedNamespace {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
            });
        }
        let root = self.worldgen_dir.join(dir);
        let mut resource = ResourcePath::from_identifier_path(id)?;
        resource.set_extension("json")?;
        let lexical = resource.lexical_under(&root);
        let io_error = |path, source| ClosureError::Io { path, source };
        let parse_error = |path, source| ClosureError::Parse { path, source };
        let Some(opened) = resource.open_existing_under(&root)? else {
            return Err(ClosureError::MissingEntry {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
                path: lexical,
            });
        };
        read_json_resource(opened, &io_error, &parse_error)
    }

    fn parse_configured_feature(
        &self,
        id: &Identifier,
        value: &Value,
    ) -> Result<ConfiguredFeatureSpec, ClosureError> {
        let type_id = parse_id(
            self.path(CONFIGURED_FEATURE, id),
            field_str(value, "type", CONFIGURED_FEATURE, id)?,
        )?;
        let config = field_of(value, "config", CONFIGURED_FEATURE, id)?;
        let kind = match type_id.path() {
            "simple_block" => ConfiguredFeatureKind::SimpleBlock {
                to_place: self.parse_state_provider(
                    id,
                    field_of(config, "to_place", CONFIGURED_FEATURE, id)?,
                )?,
                schedule_tick: config
                    .get("schedule_tick")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            "block_pile" => ConfiguredFeatureKind::BlockPile {
                state_provider: self.parse_state_provider(
                    id,
                    field_of(config, "state_provider", CONFIGURED_FEATURE, id)?,
                )?,
            },
            "block_column" => {
                let layers = field_array(config, "layers", CONFIGURED_FEATURE, id)?
                    .iter()
                    .map(|layer| {
                        Ok(BlockColumnLayer {
                            height: self.parse_int_provider(
                                id,
                                field_of(layer, "height", CONFIGURED_FEATURE, id)?,
                            )?,
                            provider: self.parse_state_provider(
                                id,
                                field_of(layer, "provider", CONFIGURED_FEATURE, id)?,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, ClosureError>>()?;
                let direction_name = field_str(config, "direction", CONFIGURED_FEATURE, id)?;
                let direction = ColumnDirection::parse(direction_name).ok_or_else(|| {
                    ClosureError::InvalidField {
                        kind: CONFIGURED_FEATURE,
                        entry: id.clone(),
                        field: "direction",
                        value: direction_name.to_owned(),
                    }
                })?;
                ConfiguredFeatureKind::BlockColumn {
                    layers,
                    direction,
                    allowed_placement: self.parse_block_predicate(
                        id,
                        field_of(config, "allowed_placement", CONFIGURED_FEATURE, id)?,
                    )?,
                    prioritize_tip: field_of(config, "prioritize_tip", CONFIGURED_FEATURE, id)?
                        .as_bool()
                        .ok_or_else(|| ClosureError::InvalidField {
                            kind: CONFIGURED_FEATURE,
                            entry: id.clone(),
                            field: "prioritize_tip",
                            value: "not a boolean".to_owned(),
                        })?,
                }
            }
            "tree" => ConfiguredFeatureKind::Tree(Box::new(self.parse_tree(id, config)?)),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: CONFIGURED_FEATURE,
                    type_id,
                    referrer: id.clone(),
                });
            }
        };
        Ok(ConfiguredFeatureSpec {
            id: id.clone(),
            kind,
        })
    }

    /// `minecraft:tree`. The village closure reaches four trunk/foliage placer
    /// pairs, an empty decorator list and a `two_layers_feature_size` minimum
    /// size; every other placer, size, decorator or root placer fails closed.
    fn parse_tree(&self, id: &Identifier, config: &Value) -> Result<TreeSpec, ClosureError> {
        if let Some(root_placer) = config.get("root_placer") {
            return Err(ClosureError::UnsupportedType {
                kind: ROOT_PLACER,
                type_id: parse_id(
                    self.path(ROOT_PLACER, id),
                    field_str(root_placer, "type", ROOT_PLACER, id)?,
                )?,
                referrer: id.clone(),
            });
        }
        let decorators = field_array(config, "decorators", CONFIGURED_FEATURE, id)?;
        if let Some(decorator) = decorators.first() {
            return Err(ClosureError::UnsupportedType {
                kind: TREE_DECORATOR,
                type_id: parse_id(
                    self.path(TREE_DECORATOR, id),
                    field_str(decorator, "type", TREE_DECORATOR, id)?,
                )?,
                referrer: id.clone(),
            });
        }

        let trunk_placer = field_of(config, "trunk_placer", CONFIGURED_FEATURE, id)?;
        let trunk_type = parse_id(
            self.path(TRUNK_PLACER, id),
            field_str(trunk_placer, "type", TRUNK_PLACER, id)?,
        )?;
        let heights = TrunkPlacerHeights {
            base_height: i32_field(trunk_placer, "base_height", TRUNK_PLACER, id)?,
            height_rand_a: i32_field(trunk_placer, "height_rand_a", TRUNK_PLACER, id)?,
            height_rand_b: i32_field(trunk_placer, "height_rand_b", TRUNK_PLACER, id)?,
        };
        let trunk_placer = match trunk_type.path() {
            "straight_trunk_placer" => TrunkPlacerSpec::Straight(heights),
            "forking_trunk_placer" => TrunkPlacerSpec::Forking(heights),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: TRUNK_PLACER,
                    type_id: trunk_type,
                    referrer: id.clone(),
                });
            }
        };

        let foliage_placer = field_of(config, "foliage_placer", CONFIGURED_FEATURE, id)?;
        let foliage_type = parse_id(
            self.path(FOLIAGE_PLACER, id),
            field_str(foliage_placer, "type", FOLIAGE_PLACER, id)?,
        )?;
        let radius =
            self.parse_int_provider(id, field_of(foliage_placer, "radius", FOLIAGE_PLACER, id)?)?;
        let offset =
            self.parse_int_provider(id, field_of(foliage_placer, "offset", FOLIAGE_PLACER, id)?)?;
        let foliage_placer = match foliage_type.path() {
            "blob_foliage_placer" => FoliagePlacerSpec::Blob {
                radius,
                offset,
                height: i32_field(foliage_placer, "height", FOLIAGE_PLACER, id)?,
            },
            "spruce_foliage_placer" => FoliagePlacerSpec::Spruce {
                radius,
                offset,
                trunk_height: self.parse_int_provider(
                    id,
                    field_of(foliage_placer, "trunk_height", FOLIAGE_PLACER, id)?,
                )?,
            },
            "pine_foliage_placer" => FoliagePlacerSpec::Pine {
                radius,
                offset,
                height: self.parse_int_provider(
                    id,
                    field_of(foliage_placer, "height", FOLIAGE_PLACER, id)?,
                )?,
            },
            "acacia_foliage_placer" => FoliagePlacerSpec::Acacia { radius, offset },
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: FOLIAGE_PLACER,
                    type_id: foliage_type,
                    referrer: id.clone(),
                });
            }
        };

        let minimum_size = field_of(config, "minimum_size", CONFIGURED_FEATURE, id)?;
        let size_type = parse_id(
            self.path(FEATURE_SIZE, id),
            field_str(minimum_size, "type", FEATURE_SIZE, id)?,
        )?;
        if size_type.path() != "two_layers_feature_size" {
            return Err(ClosureError::UnsupportedType {
                kind: FEATURE_SIZE,
                type_id: size_type,
                referrer: id.clone(),
            });
        }
        let optional_i32 = |field: &'static str| -> Result<Option<i32>, ClosureError> {
            match minimum_size.get(field) {
                None => Ok(None),
                Some(_) => Ok(Some(i32_field(minimum_size, field, FEATURE_SIZE, id)?)),
            }
        };

        Ok(TreeSpec {
            trunk_provider: self.parse_state_provider(
                id,
                field_of(config, "trunk_provider", CONFIGURED_FEATURE, id)?,
            )?,
            trunk_placer,
            foliage_provider: self.parse_state_provider(
                id,
                field_of(config, "foliage_provider", CONFIGURED_FEATURE, id)?,
            )?,
            foliage_placer,
            minimum_size: TwoLayersFeatureSize {
                limit: optional_i32("limit")?.unwrap_or(1),
                lower_size: optional_i32("lower_size")?.unwrap_or(0),
                upper_size: optional_i32("upper_size")?.unwrap_or(1),
                min_clipped_height: optional_i32("min_clipped_height")?,
            },
            ignore_vines: config
                .get("ignore_vines")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            below_trunk_provider: match config.get("below_trunk_provider") {
                Some(provider) => self.parse_state_provider(id, provider)?,
                None => below_overworld_trunks_provider(),
            },
        })
    }

    fn parse_state_provider(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<StateProviderSpec, ClosureError> {
        let type_id = parse_id(
            self.path(STATE_PROVIDER, referrer),
            field_str(value, "type", STATE_PROVIDER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "simple_state_provider" => StateProviderSpec::Simple {
                state: self.parse_block_state(
                    referrer,
                    field_of(value, "state", STATE_PROVIDER, referrer)?,
                )?,
            },
            "weighted_state_provider" => StateProviderSpec::Weighted {
                entries: {
                    let entries = field_array(value, "entries", STATE_PROVIDER, referrer)?;
                    let mut weighted = Vec::with_capacity(entries.len());
                    for entry in entries {
                        weighted.push(WeightedState {
                            state: self.parse_block_state(
                                referrer,
                                field_of(entry, "data", STATE_PROVIDER, referrer)?,
                            )?,
                            weight: i32_field(entry, "weight", STATE_PROVIDER, referrer)?,
                        });
                    }
                    weighted
                },
            },
            "rotated_block_provider" => {
                let state = self.parse_block_state(
                    referrer,
                    field_of(value, "state", STATE_PROVIDER, referrer)?,
                )?;
                StateProviderSpec::Rotated { block: state.block }
            }
            "noise_threshold_provider" => StateProviderSpec::NoiseThreshold(NoiseThresholdSpec {
                seed: i64_field(value, "seed", STATE_PROVIDER, referrer)?,
                noise: parse_noise_parameters(
                    field_of(value, "noise", STATE_PROVIDER, referrer)?,
                    referrer,
                )?,
                scale: f32_field(value, "scale", STATE_PROVIDER, referrer)?,
                threshold: f32_field(value, "threshold", STATE_PROVIDER, referrer)?,
                high_chance: f32_field(value, "high_chance", STATE_PROVIDER, referrer)?,
                default_state: self.parse_block_state(
                    referrer,
                    field_of(value, "default_state", STATE_PROVIDER, referrer)?,
                )?,
                low_states: self.parse_block_state_list(referrer, value, "low_states")?,
                high_states: self.parse_block_state_list(referrer, value, "high_states")?,
            }),
            "rule_based_state_provider" => {
                let fallback = value
                    .get("fallback")
                    .map(|fallback| self.parse_state_provider(referrer, fallback))
                    .transpose()?
                    .map(Box::new);
                let rules = field_array(value, "rules", STATE_PROVIDER, referrer)?
                    .iter()
                    .map(|rule| {
                        Ok(RuleBasedRule {
                            predicate: self.parse_block_predicate(
                                referrer,
                                field_of(rule, "if_true", STATE_PROVIDER, referrer)?,
                            )?,
                            then: self.parse_state_provider(
                                referrer,
                                field_of(rule, "then", STATE_PROVIDER, referrer)?,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, ClosureError>>()?;
                StateProviderSpec::RuleBased { fallback, rules }
            }
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: STATE_PROVIDER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_placement_modifier(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<PlacementModifierSpec, ClosureError> {
        let type_id = parse_id(
            self.path(PLACEMENT_MODIFIER, referrer),
            field_str(value, "type", PLACEMENT_MODIFIER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "count" => PlacementModifierSpec::Count(self.parse_int_provider(
                referrer,
                field_of(value, "count", PLACEMENT_MODIFIER, referrer)?,
            )?),
            "random_offset" => PlacementModifierSpec::RandomOffset {
                xz_spread: self.parse_int_provider(
                    referrer,
                    field_of(value, "xz_spread", PLACEMENT_MODIFIER, referrer)?,
                )?,
                y_spread: self.parse_int_provider(
                    referrer,
                    field_of(value, "y_spread", PLACEMENT_MODIFIER, referrer)?,
                )?,
            },
            "block_predicate_filter" => {
                PlacementModifierSpec::BlockPredicateFilter(self.parse_block_predicate(
                    referrer,
                    field_of(value, "predicate", PLACEMENT_MODIFIER, referrer)?,
                )?)
            }
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: PLACEMENT_MODIFIER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_int_provider(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<IntProviderSpec, ClosureError> {
        if let Some(constant) = value.as_i64() {
            return Ok(IntProviderSpec::Constant(i32::try_from(constant).map_err(
                |_| ClosureError::InvalidField {
                    kind: INT_PROVIDER,
                    entry: referrer.clone(),
                    field: "value",
                    value: constant.to_string(),
                },
            )?));
        }
        let type_id = parse_id(
            self.path(INT_PROVIDER, referrer),
            field_str(value, "type", INT_PROVIDER, referrer)?,
        )?;
        Ok(match type_id.path() {
            "constant" => {
                IntProviderSpec::Constant(i32_field(value, "value", INT_PROVIDER, referrer)?)
            }
            "uniform" => {
                let min_inclusive = i32_field(value, "min_inclusive", INT_PROVIDER, referrer)?;
                let max_inclusive = i32_field(value, "max_inclusive", INT_PROVIDER, referrer)?;
                if max_inclusive < min_inclusive {
                    return Err(ClosureError::InvalidField {
                        kind: INT_PROVIDER,
                        entry: referrer.clone(),
                        field: "max_inclusive",
                        value: format!("{max_inclusive} < {min_inclusive}"),
                    });
                }
                IntProviderSpec::Uniform {
                    min_inclusive,
                    max_inclusive,
                }
            }
            "trapezoid" => IntProviderSpec::Trapezoid {
                min_inclusive: i32_field(value, "min", INT_PROVIDER, referrer)?,
                max_inclusive: i32_field(value, "max", INT_PROVIDER, referrer)?,
                plateau: i32_field(value, "plateau", INT_PROVIDER, referrer)?,
            },
            "biased_to_bottom" => IntProviderSpec::BiasedToBottom {
                min_inclusive: i32_field(value, "min_inclusive", INT_PROVIDER, referrer)?,
                max_inclusive: i32_field(value, "max_inclusive", INT_PROVIDER, referrer)?,
            },
            "weighted_list" => IntProviderSpec::WeightedList({
                let distribution = field_array(value, "distribution", INT_PROVIDER, referrer)?;
                let mut entries = Vec::with_capacity(distribution.len());
                for entry in distribution {
                    entries.push(WeightedInt {
                        provider: self.parse_int_provider(
                            referrer,
                            field_of(entry, "data", INT_PROVIDER, referrer)?,
                        )?,
                        weight: i32_field(entry, "weight", INT_PROVIDER, referrer)?,
                    });
                }
                entries
            }),
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: INT_PROVIDER,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_block_predicate(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<BlockPredicateSpec, ClosureError> {
        let type_id = parse_id(
            self.path(BLOCK_PREDICATE, referrer),
            field_str(value, "type", BLOCK_PREDICATE, referrer)?,
        )?;
        let offset = parse_offset(value, BLOCK_PREDICATE, referrer)?;
        Ok(match type_id.path() {
            "would_survive" => BlockPredicateSpec::WouldSurvive {
                offset,
                state: self.parse_block_state(
                    referrer,
                    field_of(value, "state", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "matching_block_tag" => BlockPredicateSpec::MatchingBlockTag {
                offset,
                tag: parse_id(
                    self.path(BLOCK_PREDICATE, referrer),
                    field_str(value, "tag", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "matching_blocks" => BlockPredicateSpec::MatchingBlocks {
                offset,
                blocks: parse_id_list(
                    self.path(BLOCK_PREDICATE, referrer),
                    field_of(value, "blocks", BLOCK_PREDICATE, referrer)?,
                )?,
            },
            "all_of" => BlockPredicateSpec::AllOf {
                predicates: field_array(value, "predicates", BLOCK_PREDICATE, referrer)?
                    .iter()
                    .map(|predicate| self.parse_block_predicate(referrer, predicate))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            "not" => BlockPredicateSpec::Not {
                predicate: Box::new(self.parse_block_predicate(
                    referrer,
                    field_of(value, "predicate", BLOCK_PREDICATE, referrer)?,
                )?),
            },
            _ => {
                return Err(ClosureError::UnsupportedType {
                    kind: BLOCK_PREDICATE,
                    type_id,
                    referrer: referrer.clone(),
                });
            }
        })
    }

    fn parse_block_state(
        &self,
        referrer: &Identifier,
        value: &Value,
    ) -> Result<BlockStateSpec, ClosureError> {
        let name = field_str(value, "Name", STATE_PROVIDER, referrer)?;
        let mut properties = Vec::new();
        if let Some(values) = value.get("Properties").and_then(Value::as_object) {
            for (key, value) in values {
                let Some(value) = value.as_str() else {
                    return Err(ClosureError::InvalidField {
                        kind: STATE_PROVIDER,
                        entry: referrer.clone(),
                        field: "Properties",
                        value: format!("{key}={value}"),
                    });
                };
                properties.push((key.clone(), value.to_owned()));
            }
        }
        Ok(BlockStateSpec {
            block: parse_id(self.path(STATE_PROVIDER, referrer), name)?,
            properties,
        })
    }

    fn parse_block_state_list(
        &self,
        referrer: &Identifier,
        parent: &Value,
        field: &'static str,
    ) -> Result<Vec<BlockStateSpec>, ClosureError> {
        field_array(parent, field, STATE_PROVIDER, referrer)?
            .iter()
            .map(|entry| self.parse_block_state(referrer, entry))
            .collect()
    }
}

fn field_of<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a Value, ClosureError> {
    value.get(field).ok_or_else(|| ClosureError::MissingField {
        kind,
        entry: entry.clone(),
        field,
    })
}

fn field_array<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a Vec<Value>, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_array()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an array".to_owned(),
        })
}

fn field_str<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a str, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_str()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a string".to_owned(),
        })
}

fn i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<i32, ClosureError> {
    let raw = field_of(value, field, kind, entry)?
        .as_i64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an integer".to_owned(),
        })?;
    i32::try_from(raw).map_err(|_| ClosureError::InvalidField {
        kind,
        entry: entry.clone(),
        field,
        value: raw.to_string(),
    })
}

fn i64_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<i64, ClosureError> {
    field_of(value, field, kind, entry)?
        .as_i64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an integer".to_owned(),
        })
}

fn f32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<f32, ClosureError> {
    let raw = field_of(value, field, kind, entry)?
        .as_f64()
        .ok_or_else(|| ClosureError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a number".to_owned(),
        })?;
    Ok(raw as f32)
}

fn parse_id(path: PathBuf, raw: &str) -> Result<Identifier, ClosureError> {
    Identifier::parse(raw.to_owned()).map_err(|_| ClosureError::InvalidIdentifier {
        path,
        value: raw.to_owned(),
    })
}

fn parse_id_list(path: PathBuf, value: &Value) -> Result<Vec<Identifier>, ClosureError> {
    match value {
        Value::String(single) => Ok(vec![parse_id(path, single)?]),
        Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                let Some(raw) = entry.as_str() else {
                    return Err(ClosureError::InvalidIdentifier {
                        path: path.clone(),
                        value: entry.to_string(),
                    });
                };
                parse_id(path.clone(), raw)
            })
            .collect(),
        other => Err(ClosureError::InvalidIdentifier {
            path,
            value: other.to_string(),
        }),
    }
}

fn parse_offset(
    value: &Value,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<(i32, i32, i32), ClosureError> {
    let Some(offset) = value.get("offset") else {
        return Ok((0, 0, 0));
    };
    let invalid = || ClosureError::InvalidField {
        kind,
        entry: entry.clone(),
        field: "offset",
        value: offset.to_string(),
    };
    let Some(entries) = offset.as_array() else {
        return Err(invalid());
    };
    if entries.len() != 3 {
        return Err(invalid());
    }
    let mut axes = [0i32; 3];
    for (index, axis) in entries.iter().enumerate() {
        let raw = axis.as_i64().ok_or_else(invalid)?;
        let raw = i32::try_from(raw).map_err(|_| invalid())?;
        // `Vec3i.offsetCodec(16)`.
        if !(-16..=16).contains(&raw) {
            return Err(invalid());
        }
        axes[index] = raw;
    }
    Ok((axes[0], axes[1], axes[2]))
}

fn parse_noise_parameters(
    value: &Value,
    referrer: &Identifier,
) -> Result<NoiseParametersSpec, ClosureError> {
    let amplitudes = field_array(value, "amplitudes", STATE_PROVIDER, referrer)?
        .iter()
        .map(|amplitude| {
            amplitude
                .as_f64()
                .ok_or_else(|| ClosureError::InvalidField {
                    kind: STATE_PROVIDER,
                    entry: referrer.clone(),
                    field: "amplitudes",
                    value: amplitude.to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NoiseParametersSpec {
        first_octave: i32_field(value, "firstOctave", STATE_PROVIDER, referrer)?,
        amplitudes,
    })
}

/// Vanilla's `TreeConfiguration.PLACE_BELOW_OVERWORLD_TRUNKS`, used when a tree
/// config omits `below_trunk_provider`:
/// `if_true(not(matching_tag(cannot_replace_below_tree_trunk))) -> dirt`.
fn below_overworld_trunks_provider() -> StateProviderSpec {
    StateProviderSpec::RuleBased {
        fallback: None,
        rules: vec![RuleBasedRule {
            predicate: BlockPredicateSpec::Not {
                predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, 0, 0),
                    tag: identifier("minecraft:cannot_replace_below_tree_trunk"),
                }),
            },
            then: StateProviderSpec::Simple {
                state: BlockStateSpec {
                    block: identifier("minecraft:dirt"),
                    properties: Vec::new(),
                },
            },
        }],
    }
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).expect("vanilla identifiers are valid")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

    // Synthetic Solaris-authored fixtures only: they mirror the vanilla JSON
    // shape without carrying any Mojang bytes.
    fn worldgen_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        for sub in ["placed_feature", "configured_feature", "template_pool"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        dir
    }

    fn write_json(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn id(value: &str) -> Identifier {
        Identifier::parse(value.to_owned()).unwrap()
    }

    fn flower_worldgen() -> TempDir {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/flower_fixture.json",
            r#"{
                "feature": "minecraft:flower_fixture",
                "placement": [
                    { "type": "minecraft:count", "count": 64 },
                    {
                        "type": "minecraft:random_offset",
                        "xz_spread": { "type": "minecraft:trapezoid", "min": -6, "max": 6, "plateau": 0 },
                        "y_spread": { "type": "minecraft:trapezoid", "min": -2, "max": 2, "plateau": 0 }
                    },
                    {
                        "type": "minecraft:block_predicate_filter",
                        "predicate": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" }
                    }
                ]
            }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/flower_fixture.json",
            r#"{
                "type": "minecraft:simple_block",
                "config": {
                    "to_place": {
                        "type": "minecraft:noise_threshold_provider",
                        "seed": 2345,
                        "noise": { "amplitudes": [1.0], "firstOctave": 0 },
                        "scale": 0.005,
                        "threshold": -0.8,
                        "high_chance": 0.33333334,
                        "default_state": { "Name": "minecraft:dandelion" },
                        "low_states": [
                            { "Name": "minecraft:orange_tulip" },
                            { "Name": "minecraft:red_tulip" }
                        ],
                        "high_states": [
                            { "Name": "minecraft:poppy" },
                            { "Name": "minecraft:azure_bluet" }
                        ]
                    }
                }
            }"#,
        );
        dir
    }

    #[test]
    fn simple_block_closure_resolves_provider_and_modifier_order() {
        let dir = flower_worldgen();
        let closure = FeatureClosure::new(dir.path());
        let spec = closure
            .load_placed_feature(&id("minecraft:flower_fixture"))
            .expect("fixture closure resolves");

        assert_eq!(spec.id, id("minecraft:flower_fixture"));
        assert_eq!(spec.feature.id, id("minecraft:flower_fixture"));
        assert_eq!(
            spec.placement,
            vec![
                PlacementModifierSpec::Count(IntProviderSpec::Constant(64)),
                PlacementModifierSpec::RandomOffset {
                    xz_spread: IntProviderSpec::Trapezoid {
                        min_inclusive: -6,
                        max_inclusive: 6,
                        plateau: 0,
                    },
                    y_spread: IntProviderSpec::Trapezoid {
                        min_inclusive: -2,
                        max_inclusive: 2,
                        plateau: 0,
                    },
                },
                PlacementModifierSpec::BlockPredicateFilter(BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, 0, 0),
                    tag: id("minecraft:air"),
                }),
            ]
        );

        let ConfiguredFeatureKind::SimpleBlock {
            to_place,
            schedule_tick,
        } = &spec.feature.kind
        else {
            panic!("expected simple_block");
        };
        assert!(!schedule_tick);
        let StateProviderSpec::NoiseThreshold(noise) = to_place else {
            panic!("expected noise_threshold_provider");
        };
        assert_eq!(noise.seed, 2345);
        assert_eq!(
            noise.noise,
            NoiseParametersSpec {
                first_octave: 0,
                amplitudes: vec![1.0],
            }
        );
        assert_eq!(noise.scale, 0.005);
        assert_eq!(noise.threshold, -0.8);
        assert_eq!(noise.default_state.block, id("minecraft:dandelion"));
        assert_eq!(noise.low_states.len(), 2);
        assert_eq!(noise.high_states.len(), 2);
        assert_eq!(noise.high_states[1].block, id("minecraft:azure_bluet"));
    }

    #[test]
    fn rotated_and_weighted_pile_providers_resolve() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/pile_fixture.json",
            r#"{ "feature": "minecraft:pile_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/pile_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:rotated_block_provider",
                        "state": {
                            "Name": "minecraft:hay_block",
                            "Properties": { "axis": "y" }
                        }
                    }
                }
            }"#,
        );
        let closure = FeatureClosure::new(dir.path());
        let spec = closure
            .load_placed_feature(&id("minecraft:pile_fixture"))
            .unwrap();
        let ConfiguredFeatureKind::BlockPile { state_provider } = &spec.feature.kind else {
            panic!("expected block_pile");
        };
        assert_eq!(
            state_provider,
            &StateProviderSpec::Rotated {
                block: id("minecraft:hay_block")
            }
        );
        assert!(spec.placement.is_empty());

        let weighted = closure
            .parse_state_provider(
                &id("minecraft:pile_fixture"),
                &serde_json::json!({
                    "type": "minecraft:weighted_state_provider",
                    "entries": [
                        { "data": { "Name": "minecraft:blue_ice" }, "weight": 1 },
                        { "data": { "Name": "minecraft:packed_ice" }, "weight": 5 }
                    ]
                }),
            )
            .unwrap();
        let StateProviderSpec::Weighted { entries } = weighted else {
            panic!("expected weighted_state_provider");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].weight, 1);
        assert_eq!(entries[1].state.block, id("minecraft:packed_ice"));
    }

    #[test]
    fn cactus_column_resolves_layers_direction_and_predicate() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/patch_cactus_fixture.json",
            r#"{
                "feature": "minecraft:cactus_fixture",
                "placement": [
                    { "type": "minecraft:count", "count": 10 },
                    {
                        "type": "minecraft:block_predicate_filter",
                        "predicate": {
                            "type": "minecraft:all_of",
                            "predicates": [
                                { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                                {
                                    "type": "minecraft:would_survive",
                                    "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                                }
                            ]
                        }
                    }
                ]
            }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/cactus_fixture.json",
            r#"{
                "type": "minecraft:block_column",
                "config": {
                    "allowed_placement": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                    "direction": "up",
                    "layers": [
                        {
                            "height": { "type": "minecraft:biased_to_bottom", "min_inclusive": 1, "max_inclusive": 3 },
                            "provider": {
                                "type": "minecraft:simple_state_provider",
                                "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                            }
                        },
                        {
                            "height": {
                                "type": "minecraft:weighted_list",
                                "distribution": [
                                    { "data": 0, "weight": 3 },
                                    { "data": 1, "weight": 1 }
                                ]
                            },
                            "provider": {
                                "type": "minecraft:simple_state_provider",
                                "state": { "Name": "minecraft:cactus_flower" }
                            }
                        }
                    ],
                    "prioritize_tip": false
                }
            }"#,
        );
        let closure = FeatureClosure::new(dir.path());
        let spec = closure
            .load_placed_feature(&id("minecraft:patch_cactus_fixture"))
            .unwrap();
        let ConfiguredFeatureKind::BlockColumn {
            layers,
            direction,
            allowed_placement,
            prioritize_tip,
        } = &spec.feature.kind
        else {
            panic!("expected block_column");
        };
        assert_eq!(*direction, ColumnDirection::Up);
        assert_eq!(direction.offset(), (0, 1, 0));
        assert!(!prioritize_tip);
        assert_eq!(
            allowed_placement,
            &BlockPredicateSpec::MatchingBlockTag {
                offset: (0, 0, 0),
                tag: id("minecraft:air"),
            }
        );
        assert_eq!(
            layers[0].height,
            IntProviderSpec::BiasedToBottom {
                min_inclusive: 1,
                max_inclusive: 3,
            }
        );
        assert_eq!(
            layers[1].height,
            IntProviderSpec::WeightedList(vec![
                WeightedInt {
                    provider: IntProviderSpec::Constant(0),
                    weight: 3,
                },
                WeightedInt {
                    provider: IntProviderSpec::Constant(1),
                    weight: 1,
                },
            ])
        );
        assert_eq!(
            layers[1].provider,
            StateProviderSpec::Simple {
                state: BlockStateSpec {
                    block: id("minecraft:cactus_flower"),
                    properties: Vec::new(),
                }
            }
        );
    }

    #[test]
    fn rule_based_provider_resolves_rules_and_fallback() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/tree_like_fixture.json",
            r#"{ "feature": "minecraft:tree_like_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/tree_like_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:rule_based_state_provider",
                        "fallback": {
                            "type": "minecraft:simple_state_provider",
                            "state": { "Name": "minecraft:dirt" }
                        },
                        "rules": [
                            {
                                "if_true": {
                                    "type": "minecraft:not",
                                    "predicate": {
                                        "type": "minecraft:matching_block_tag",
                                        "tag": "minecraft:cannot_replace_below_tree_trunk"
                                    }
                                },
                                "then": {
                                    "type": "minecraft:simple_state_provider",
                                    "state": { "Name": "minecraft:coarse_dirt" }
                                }
                            }
                        ]
                    }
                }
            }"#,
        );
        let closure = FeatureClosure::new(dir.path());
        let spec = closure
            .load_placed_feature(&id("minecraft:tree_like_fixture"))
            .unwrap();
        let ConfiguredFeatureKind::BlockPile { state_provider } = &spec.feature.kind else {
            panic!("expected block_pile");
        };
        let StateProviderSpec::RuleBased { fallback, rules } = state_provider else {
            panic!("expected rule_based_state_provider");
        };
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].predicate,
            BlockPredicateSpec::Not {
                predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, 0, 0),
                    tag: id("minecraft:cannot_replace_below_tree_trunk"),
                }),
            }
        );
        assert_eq!(
            fallback.as_deref(),
            Some(&StateProviderSpec::Simple {
                state: BlockStateSpec {
                    block: id("minecraft:dirt"),
                    properties: Vec::new(),
                },
            })
        );
    }

    #[test]
    fn matching_blocks_predicate_accepts_scalar_and_list_forms() {
        let dir = worldgen_dir();
        let closure = FeatureClosure::new(dir.path());
        let referrer = id("minecraft:fixture");
        for (body, expected) in [
            (
                serde_json::json!({ "type": "minecraft:matching_blocks", "blocks": "minecraft:grass_block" }),
                vec![id("minecraft:grass_block")],
            ),
            (
                serde_json::json!({
                    "type": "minecraft:matching_blocks",
                    "blocks": ["minecraft:sand", "minecraft:red_sand"],
                    "offset": [0, -1, 0]
                }),
                vec![id("minecraft:sand"), id("minecraft:red_sand")],
            ),
        ] {
            let predicate = closure.parse_block_predicate(&referrer, &body).unwrap();
            let BlockPredicateSpec::MatchingBlocks { blocks, .. } = predicate else {
                panic!("expected matching_blocks");
            };
            assert_eq!(blocks, expected);
        }
    }

    #[test]
    fn unsupported_configured_feature_type_fails_closed_naming_type_and_referrer() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/tree_fixture.json",
            r#"{ "feature": "minecraft:tree_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/tree_fixture.json",
            r#"{ "type": "minecraft:random_patch", "config": {} }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:tree_fixture"))
            .expect_err("random patches are outside the closure");
        assert!(matches!(
            error,
            ClosureError::UnsupportedType {
                kind: CONFIGURED_FEATURE,
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("minecraft:random_patch"), "{message}");
        assert!(message.contains("minecraft:tree_fixture"), "{message}");
    }

    #[test]
    fn unsupported_placement_modifier_fails_closed_naming_type_and_referrer() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/in_square_fixture.json",
            r#"{
                "feature": "minecraft:plain_fixture",
                "placement": [ { "type": "minecraft:in_square" } ]
            }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/plain_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:melon" }
                    }
                }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:in_square_fixture"))
            .expect_err("unreachable modifiers must fail closed");
        assert!(matches!(
            error,
            ClosureError::UnsupportedType {
                kind: PLACEMENT_MODIFIER,
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("minecraft:in_square"), "{message}");
        assert!(message.contains("minecraft:in_square_fixture"), "{message}");
    }

    #[test]
    fn unsupported_state_provider_and_predicate_fail_closed() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/dual_noise_fixture.json",
            r#"{ "feature": "minecraft:dual_noise_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/dual_noise_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": { "state_provider": { "type": "minecraft:dual_noise_provider" } }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:dual_noise_fixture"))
            .expect_err("unsupported providers must fail closed");
        let message = error.to_string();
        assert!(
            message.contains("minecraft:dual_noise_provider"),
            "{message}"
        );
        assert!(
            message.contains("minecraft:dual_noise_fixture"),
            "{message}"
        );

        write_json(
            dir.path(),
            "placed_feature/solid_filter_fixture.json",
            r#"{
                "feature": "minecraft:plain_fixture",
                "placement": [
                    {
                        "type": "minecraft:block_predicate_filter",
                        "predicate": { "type": "minecraft:solid" }
                    }
                ]
            }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/plain_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:melon" }
                    }
                }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:solid_filter_fixture"))
            .expect_err("unsupported predicates must fail closed");
        let message = error.to_string();
        assert!(message.contains("minecraft:solid"), "{message}");
        assert!(
            message.contains("minecraft:solid_filter_fixture"),
            "{message}"
        );
    }

    #[test]
    fn entries_outside_the_closure_are_never_loaded() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/plain_fixture.json",
            r#"{ "feature": "minecraft:plain_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/plain_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:melon" }
                    }
                }
            }"#,
        );
        // Unreachable and unsupported: a load that enumerated the registry
        // would fail here, resolve-by-reference must not read it at all.
        write_json(
            dir.path(),
            "placed_feature/unreachable_tree.json",
            r#"{ "feature": "minecraft:unreachable_tree", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/unreachable_tree.json",
            r#"{ "type": "minecraft:tree", "config": {} }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/unreferenced.json",
            r#"{ "type": "minecraft:not_a_thing", "config": {} }"#,
        );

        let spec = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:plain_fixture"))
            .expect("unreachable entries must not be loaded");
        assert_eq!(spec.id, id("minecraft:plain_fixture"));
    }

    #[test]
    fn pool_walk_reads_only_feature_pool_elements() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/pile_fixture.json",
            r#"{ "feature": "minecraft:pile_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/pile_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:snow", "Properties": { "layers": "1" } }
                    }
                }
            }"#,
        );
        write_json(
            dir.path(),
            "placed_feature/flower_fixture.json",
            r#"{ "feature": "minecraft:flower_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/flower_fixture.json",
            r#"{
                "type": "minecraft:simple_block",
                "config": {
                    "to_place": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:dandelion" }
                    }
                }
            }"#,
        );
        write_json(
            dir.path(),
            "template_pool/village_fixture/decor.json",
            r#"{
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/does_not_exist",
                            "processors": { "processors": [] },
                            "projection": "rigid"
                        },
                        "weight": 2
                    },
                    {
                        "element": {
                            "element_type": "minecraft:feature_pool_element",
                            "feature": "minecraft:pile_fixture",
                            "projection": "rigid"
                        },
                        "weight": 4
                    },
                    {
                        "element": {
                            "element_type": "minecraft:feature_pool_element",
                            "feature": "minecraft:flower_fixture",
                            "projection": "rigid"
                        },
                        "weight": 1
                    },
                    { "element": { "element_type": "minecraft:empty_pool_element" }, "weight": 2 }
                ],
                "fallback": "minecraft:empty"
            }"#,
        );

        let entries = FeatureClosure::new(dir.path())
            .load_pool_features(&id("minecraft:village_fixture/decor"))
            .expect("feature elements resolve");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].placed_feature, id("minecraft:pile_fixture"));
        assert_eq!(entries[0].weight, 4);
        assert_eq!(entries[0].pool, id("minecraft:village_fixture/decor"));
        assert_eq!(entries[1].placed_feature, id("minecraft:flower_fixture"));
        assert_eq!(entries[1].weight, 1);
    }

    #[test]
    fn pool_referencing_a_missing_feature_names_the_pool() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "template_pool/village_fixture/decor.json",
            r#"{
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:feature_pool_element",
                            "feature": "minecraft:absent_feature",
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_pool_features(&id("minecraft:village_fixture/decor"))
            .expect_err("missing placed features fail closed");
        assert!(matches!(
            error,
            ClosureError::MissingEntry {
                kind: PLACED_FEATURE,
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("minecraft:absent_feature"), "{message}");
        assert!(
            message.contains("minecraft:village_fixture/decor"),
            "{message}"
        );
    }

    #[test]
    fn non_minecraft_references_fail_closed() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/plugin_fixture.json",
            r#"{ "feature": "example:plugin_feature", "placement": [] }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:plugin_fixture"))
            .expect_err("non-vanilla configured features fail closed");
        assert!(matches!(
            error,
            ClosureError::UnsupportedNamespace {
                kind: CONFIGURED_FEATURE,
                ..
            }
        ));
        let message = error.to_string();
        assert!(message.contains("example:plugin_feature"), "{message}");
        assert!(message.contains("minecraft:plugin_fixture"), "{message}");
    }

    #[test]
    fn block_state_property_values_must_be_strings() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/bad_props_fixture.json",
            r#"{ "feature": "minecraft:plain_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/plain_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:snow", "Properties": { "layers": 1 } }
                    }
                }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:bad_props_fixture"))
            .expect_err("a non-string property value is a data error");
        let message = error.to_string();
        assert!(message.contains("layers=1"), "{message}");
    }

    #[test]
    fn pool_element_weight_is_required_and_bounded() {
        for (weight, expect_error) in [("\"weight\": 0", true), ("\"weight\": 151", true)] {
            let dir = worldgen_dir();
            write_json(
                dir.path(),
                "template_pool/village_fixture/decor.json",
                &format!(
                    r#"{{
                        "elements": [
                            {{
                                "element": {{
                                    "element_type": "minecraft:feature_pool_element",
                                    "feature": "minecraft:absent_feature",
                                    "projection": "rigid"
                                }},
                                {weight}
                            }}
                        ]
                    }}"#
                ),
            );
            let result = FeatureClosure::new(dir.path())
                .load_pool_features(&id("minecraft:village_fixture/decor"));
            assert_eq!(result.is_err(), expect_error, "weight {weight}");
        }

        // A missing weight is an error too, unlike a silent default of 1.
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "template_pool/village_fixture/decor.json",
            r#"{
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:empty_pool_element"
                        }
                    }
                ]
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_pool_features(&id("minecraft:village_fixture/decor"))
            .expect_err("weight is required");
        assert!(error.to_string().contains("weight"), "{error}");
    }

    #[test]
    fn list_pool_elements_fail_closed() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "template_pool/village_fixture/decor.json",
            r#"{
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:list_pool_element",
                            "elements": [
                                {
                                    "element_type": "minecraft:feature_pool_element",
                                    "feature": "minecraft:absent_feature",
                                    "projection": "rigid"
                                }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_pool_features(&id("minecraft:village_fixture/decor"))
            .expect_err("a list element can wrap feature elements, so it may not be skipped");
        let message = error.to_string();
        assert!(message.contains("minecraft:list_pool_element"), "{message}");
        assert!(
            message.contains("minecraft:village_fixture/decor"),
            "{message}"
        );
    }

    #[test]
    fn predicate_offsets_must_stay_within_vanillas_bound() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/offset_fixture.json",
            r#"{
                "feature": "minecraft:plain_fixture",
                "placement": [
                    {
                        "type": "minecraft:block_predicate_filter",
                        "predicate": {
                            "type": "minecraft:matching_block_tag",
                            "tag": "minecraft:air",
                            "offset": [0, 17, 0]
                        }
                    }
                ]
            }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/plain_fixture.json",
            r#"{
                "type": "minecraft:block_pile",
                "config": {
                    "state_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:melon" }
                    }
                }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:offset_fixture"))
            .expect_err("Vec3i.offsetCodec(16) bounds predicate offsets");
        assert!(error.to_string().contains("offset"), "{error}");
    }

    #[test]
    fn block_column_requires_prioritize_tip() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/column_fixture.json",
            r#"{ "feature": "minecraft:column_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/column_fixture.json",
            r#"{
                "type": "minecraft:block_column",
                "config": {
                    "allowed_placement": { "type": "minecraft:matching_block_tag", "tag": "minecraft:air" },
                    "direction": "up",
                    "layers": [
                        {
                            "height": 1,
                            "provider": {
                                "type": "minecraft:simple_state_provider",
                                "state": { "Name": "minecraft:cactus", "Properties": { "age": "0" } }
                            }
                        }
                    ]
                }
            }"#,
        );
        let error = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:column_fixture"))
            .expect_err("prioritize_tip has no default in vanilla");
        assert!(error.to_string().contains("prioritize_tip"), "{error}");
    }

    #[test]
    fn tree_configuration_resolves_placers_and_defaults() {
        let dir = worldgen_dir();
        write_json(
            dir.path(),
            "placed_feature/tree_fixture.json",
            r#"{ "feature": "minecraft:tree_fixture", "placement": [] }"#,
        );
        write_json(
            dir.path(),
            "configured_feature/tree_fixture.json",
            r#"{
                "type": "minecraft:tree",
                "config": {
                    "decorators": [],
                    "foliage_placer": { "type": "minecraft:acacia_foliage_placer", "offset": 0, "radius": 2 },
                    "foliage_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:acacia_leaves", "Properties": { "distance": "7", "persistent": "false", "waterlogged": "false" } }
                    },
                    "minimum_size": { "type": "minecraft:two_layers_feature_size" },
                    "trunk_placer": { "type": "minecraft:forking_trunk_placer", "base_height": 5, "height_rand_a": 2, "height_rand_b": 2 },
                    "trunk_provider": {
                        "type": "minecraft:simple_state_provider",
                        "state": { "Name": "minecraft:acacia_log", "Properties": { "axis": "y" } }
                    }
                }
            }"#,
        );
        let spec = FeatureClosure::new(dir.path())
            .load_placed_feature(&id("minecraft:tree_fixture"))
            .expect("tree closure resolves");
        let ConfiguredFeatureKind::Tree(tree) = &spec.feature.kind else {
            panic!("expected a tree");
        };
        assert_eq!(
            tree.trunk_placer,
            TrunkPlacerSpec::Forking(TrunkPlacerHeights {
                base_height: 5,
                height_rand_a: 2,
                height_rand_b: 2,
            })
        );
        // `ignore_vines` defaults to false and the below-trunk provider to
        // vanilla's `PLACE_BELOW_OVERWORLD_TRUNKS`.
        assert!(!tree.ignore_vines);
        let StateProviderSpec::RuleBased { fallback, rules } = &tree.below_trunk_provider else {
            panic!("expected the default below-trunk rule");
        };
        assert!(fallback.is_none());
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].predicate,
            BlockPredicateSpec::Not {
                predicate: Box::new(BlockPredicateSpec::MatchingBlockTag {
                    offset: (0, 0, 0),
                    tag: id("minecraft:cannot_replace_below_tree_trunk"),
                }),
            }
        );
        // The `two_layers_feature_size` codec's `.orElse` defaults.
        assert_eq!(tree.minimum_size.limit, 1);
        assert_eq!(tree.minimum_size.lower_size, 0);
        assert_eq!(tree.minimum_size.upper_size, 1);
        assert_eq!(tree.minimum_size.min_clipped_height, None);
    }
}
