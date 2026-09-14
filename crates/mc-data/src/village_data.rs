//! Vanilla worldgen structure registries, resolved by reference.
//!
//! Four JSON registries, each loaded by identifier:
//!
//! - `worldgen/structure_set/*.json` — [`StructureSetSpec`]: the placement plus
//!   the weighted structures the set names.
//! - `worldgen/structure/*.json` — [`StructureSpec`]: the `minecraft:jigsaw`
//!   settings a village needs (start pool, depth, start height, heightmap
//!   projection, terrain adaptation, centre distance, biome tag, step, spawn
//!   overrides).
//! - `worldgen/template_pool/*.json` — [`TemplatePoolSpec`]: weighted elements
//!   of every element type, each naming its piece or placed feature, its
//!   projection and its processor list reference, plus the fallback chain.
//! - `worldgen/processor_list/*.json` — [`ProcessorListSpec`]: `rule`,
//!   `block_age`, `gravity`, `jigsaw_replacement` and `protected_blocks`
//!   processors with their rule fields.
//!
//! Only entries the caller names are read; the vanilla registries are never
//! enumerated, so registries Solaris does not implement cannot fail a load.
//! Anything a loaded entry reaches that is unsupported or malformed fails
//! closed, naming both the type id and the entry that referenced it. Referrers
//! are a within-load notion: a nested entry (a rule test inside an inline
//! processor list, a processor inside a list, an element inside a pool) names
//! the entry that carries it, and a top-level load names itself, since the
//! loader is handed one id at a time and the caller resolves references itself.
//! Required and optional fields follow the 26.1.2 codecs
//! (`net/minecraft/world/level/levelgen/structure/**`): nothing vanilla
//! requires is defaulted, and nothing vanilla defaults is required. Values that
//! vanilla's codec rejects are rejected here too.
//!
//! Piece NBT is deliberately *not* read here: `mc-worldgen`'s structure loader
//! owns the template (palette, blocks, jigsaw blocks) and the jigsaw walk. This
//! module stops at the ids the JSON names, and the engine follows them.

use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;

use crate::vanilla_feature_closure::BlockStateSpec;
use crate::{Identifier, ResourcePath, ResourcePathError, read_json_resource};

/// How a reference is named in error messages, e.g. "template pool".
type EntryKind = &'static str;

const STRUCTURE_SET: EntryKind = "structure set";
const STRUCTURE_PLACEMENT: EntryKind = "structure placement";
const EXCLUSION_ZONE: EntryKind = "exclusion zone";
const STRUCTURE: EntryKind = "structure";
const HEIGHT_PROVIDER: EntryKind = "height provider";
const SPAWN_OVERRIDE: EntryKind = "structure spawn override";
const TEMPLATE_POOL: EntryKind = "template pool";
const POOL_ELEMENT: EntryKind = "template pool element";
const PROCESSOR_LIST: EntryKind = "processor list";
const PROCESSOR: EntryKind = "structure processor";
const RULE_TEST: EntryKind = "rule test";
const POS_RULE_TEST: EntryKind = "pos rule test";
const BLOCK_STATE: EntryKind = "block state";

/// `VerticalAnchor` bounds: `DimensionType.MIN_Y`..=`DimensionType.MAX_Y`.
const MIN_Y: i32 = -2032;
const MAX_Y: i32 = 2031;
/// `DimensionType.Y_SIZE`, the default and maximum vertical reach.
const Y_SIZE: i32 = MAX_Y - MIN_Y + 1;
/// `JigsawStructure.MAX_TOTAL_STRUCTURE_RANGE`.
const MAX_STRUCTURE_RANGE: i32 = 128;

#[derive(Debug, Error)]
pub enum VillageDataError {
    #[error("village data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("village data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid identifier {value:?} referenced by {referrer}")]
    InvalidIdentifier { referrer: Identifier, value: String },
    #[error(transparent)]
    ResourcePath(#[from] ResourcePathError),
    #[error("missing {kind} {id} referenced by {referrer} at {path}")]
    MissingEntry {
        kind: EntryKind,
        id: Identifier,
        referrer: Identifier,
        path: PathBuf,
    },
    #[error("non-minecraft {kind} {id} referenced by {referrer} is outside the vanilla cache")]
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
    #[error("{kind} {entry} has unsupported {field}: {value}")]
    UnsupportedField {
        kind: EntryKind,
        entry: Identifier,
        field: &'static str,
        value: String,
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

/// One weighted structure of a structure set, as `StructureSelectionEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureSetEntry {
    pub structure: Identifier,
    pub weight: i32,
}

/// `minecraft:random_spread`, the only placement type villages use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RandomSpreadType {
    Linear,
    Triangular,
}

/// `FrequencyReductionMethod`; `default` halves nothing and is the fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrequencyReductionMethod {
    Default,
    LegacyType1,
    LegacyType2,
    LegacyType3,
}

/// `StructurePlacement.ExclusionZone`: another set whose nearby placements
/// forbid this one. The named set is loaded by reference by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusionZone {
    pub other_set: Identifier,
    pub chunk_count: i32,
}

/// The base `StructurePlacement` fields every placement carries.
#[derive(Debug, Clone, PartialEq)]
pub struct RandomSpreadPlacement {
    pub spacing: i32,
    pub separation: i32,
    /// `ExtraCodecs.NON_NEGATIVE_INT`, required by the codec.
    pub salt: i32,
    /// `spread_type`, `linear` by default.
    pub spread_type: RandomSpreadType,
    /// `locate_offset`, zero by default and bounded to ±16 per axis.
    pub locate_offset: [i32; 3],
    /// `frequency_reduction_method`, `default` by default.
    pub frequency_reduction_method: FrequencyReductionMethod,
    /// `frequency`, `1.0` by default.
    pub frequency: f32,
    /// `exclusion_zone`, absent by default.
    pub exclusion_zone: Option<ExclusionZone>,
}

/// How a structure set scatters its structures. `minecraft:concentric_rings`
/// (strongholds) and anything else is not implemented and fails closed.
#[derive(Debug, Clone, PartialEq)]
pub enum PlacementSpec {
    RandomSpread(RandomSpreadPlacement),
}

/// A structure set: placement plus the weighted structures it names.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSetSpec {
    pub id: Identifier,
    pub placement: PlacementSpec,
    pub structures: Vec<StructureSetEntry>,
}

/// `biomes`, a homogeneous holder set: a `#`-prefixed tag, a single biome, or a
/// list of biomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BiomeSet {
    Tag(Identifier),
    Biomes(Vec<Identifier>),
}

/// `VerticalAnchor`: an absolute Y, or an offset from the dimension's floor or
/// ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}

/// A `HeightProvider`; the providers whose sampling Solaris needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightProviderSpec {
    Constant(VerticalAnchor),
    Uniform {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
    },
    Trapezoid {
        min_inclusive: VerticalAnchor,
        max_inclusive: VerticalAnchor,
        plateau: i32,
    },
}

/// `Heightmap.Types`, as named by `project_start_to_heightmap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeightmapType {
    WorldSurfaceWg,
    WorldSurface,
    OceanFloorWg,
    OceanFloor,
    MotionBlocking,
    MotionBlockingNoLeaves,
}

/// `GenerationStep.Decoration`, the generation step a structure claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureStep {
    RawGeneration,
    Lakes,
    LocalModifications,
    UndergroundStructures,
    SurfaceStructures,
    Strongholds,
    UndergroundOres,
    UndergroundDecoration,
    FluidSprings,
    VegetalDecoration,
    TopLayerModification,
}

/// `TerrainAdjustment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainAdaptation {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

/// `LiquidSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidSettings {
    IgnoreWaterlogging,
    ApplyWaterlogging,
}

/// `MobCategory`, the spawn override key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MobCategory {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}

/// `StructureSpawnOverride.BoundingBoxType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnBoundingBox {
    Piece,
    Full,
}

/// One `SpawnerData` of a weighted spawn list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedSpawn {
    pub weight: i32,
    pub entity_type: Identifier,
    pub min_count: i32,
    pub max_count: i32,
}

/// One `MobCategory`'s spawn override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnOverrideEntry {
    pub category: MobCategory,
    pub bounding_box: SpawnBoundingBox,
    pub spawns: Vec<WeightedSpawn>,
}

/// `JigsawStructure.MaxDistance`: `horizontal` and `vertical`, written as one
/// integer when they are equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxDistance {
    pub horizontal: i32,
    pub vertical: i32,
}

/// `DimensionPadding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DimensionPadding {
    pub bottom: i32,
    pub top: i32,
}

/// A `minecraft:jigsaw` structure, with the settings a village needs.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureSpec {
    pub id: Identifier,
    /// The `type` id, always `minecraft:jigsaw` for a loaded spec.
    pub structure_type: Identifier,
    pub biomes: BiomeSet,
    /// `spawn_overrides`, keyed by mob category; required by the codec.
    pub spawn_overrides: Vec<SpawnOverrideEntry>,
    /// `step`, required by the codec.
    pub step: StructureStep,
    /// `terrain_adaptation`, `none` by default.
    pub terrain_adaptation: TerrainAdaptation,
    pub start_pool: Identifier,
    /// `start_jigsaw_name`, absent by default.
    pub start_jigsaw_name: Option<Identifier>,
    /// `size`, the maximum jigsaw depth.
    pub size: i32,
    pub start_height: HeightProviderSpec,
    /// `use_expansion_hack`, required by the codec.
    pub use_expansion_hack: bool,
    pub project_start_to_heightmap: Option<HeightmapType>,
    pub max_distance_from_center: MaxDistance,
    /// `dimension_padding`, zero by default.
    pub dimension_padding: DimensionPadding,
    /// `liquid_settings`, `apply_waterlogging` by default.
    pub liquid_settings: LiquidSettings,
}

/// `StructureTemplatePool.Projection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Rigid,
    TerrainMatching,
}

/// The `processors` field of a single pool element: a processor list reference
/// or an inline list. Required by the codec, so an element always carries one.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorRef {
    List(Identifier),
    Inline(Vec<StructureProcessorSpec>),
}

/// The settings `single_pool_element` and `legacy_single_pool_element` share.
/// The two are distinct element types because vanilla resolves their jigsaw
/// blocks differently, so the spec keeps them apart.
#[derive(Debug, Clone, PartialEq)]
pub struct SingleElementSpec {
    pub location: Identifier,
    pub projection: Projection,
    pub processors: ProcessorRef,
    /// `override_liquid_settings`, absent by default.
    pub override_liquid_settings: Option<LiquidSettings>,
}

/// One pool element.
#[derive(Debug, Clone, PartialEq)]
pub enum PoolElementSpec {
    /// `empty_pool_element`; vanilla fixes its projection to `terrain_matching`
    /// and the codec carries no fields.
    Empty,
    Single(SingleElementSpec),
    LegacySingle(SingleElementSpec),
    /// `list_pool_element`, whose own elements are parsed recursively.
    List {
        elements: Vec<PoolElementSpec>,
        projection: Projection,
    },
    /// `feature_pool_element`; the placed feature is loaded by reference by the
    /// feature closure, so only its id is carried here.
    Feature {
        feature: Identifier,
        projection: Projection,
    },
}

/// One weighted element of a template pool.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolElementEntry {
    pub weight: i32,
    pub element: PoolElementSpec,
}

/// A template pool: its fallback and its weighted elements.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplatePoolSpec {
    pub id: Identifier,
    pub fallback: Identifier,
    pub elements: Vec<PoolElementEntry>,
}

/// `RuleTest`: what a rule matches against.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleTestSpec {
    AlwaysTrue,
    BlockMatch {
        block: Identifier,
    },
    BlockStateMatch {
        state: BlockStateSpec,
    },
    /// `tag`, a block tag id written without its `#`.
    TagMatch {
        tag: Identifier,
    },
    RandomBlockMatch {
        block: Identifier,
        probability: f32,
    },
    RandomBlockStateMatch {
        state: BlockStateSpec,
        probability: f32,
    },
}

/// `Direction.Axis`, as used by `axis_aligned_linear_pos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

/// `PosRuleTest`: what a rule matches against in template/world space.
#[derive(Debug, Clone, PartialEq)]
pub enum PosRuleTestSpec {
    AlwaysTrue,
    LinearPos {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
    },
    AxisAlignedLinearPos {
        min_chance: f32,
        max_chance: f32,
        min_dist: i32,
        max_dist: i32,
        axis: Axis,
    },
}

/// One `ProcessorRule`: first matching rule of a `minecraft:rule` processor
/// wins, and `output_state` is the outcome it writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorRuleSpec {
    pub input_predicate: RuleTestSpec,
    pub location_predicate: RuleTestSpec,
    /// `position_predicate`, `always_true` by default.
    pub position_predicate: PosRuleTestSpec,
    pub output_state: BlockStateSpec,
}

/// One processor of a processor list. `block_ignore`, `block_rot`, `nop`,
/// `blackstone_replace`, `lava_submerged_block` and `capped` are not
/// implemented and fail closed.
#[derive(Debug, Clone, PartialEq)]
pub enum StructureProcessorSpec {
    Rule {
        rules: Vec<ProcessorRuleSpec>,
    },
    BlockAge {
        mossiness: f32,
    },
    Gravity {
        heightmap: HeightmapType,
        offset: i32,
    },
    JigsawReplacement,
    /// `protected_blocks`; `cannot_replace` is the block tag id with its `#`
    /// stripped, since the codec demands the `#` in the stored form.
    ProtectedBlocks {
        cannot_replace: Identifier,
    },
}

/// A processor list, in application order.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessorListSpec {
    pub id: Identifier,
    pub processors: Vec<StructureProcessorSpec>,
}

/// Reads the four worldgen structure registries out of a vanilla cache's
/// `data/minecraft/worldgen` directory.
pub struct VillageDataLoader {
    worldgen_dir: PathBuf,
}

impl VillageDataLoader {
    #[must_use]
    pub fn new(worldgen_dir: impl Into<PathBuf>) -> Self {
        Self {
            worldgen_dir: worldgen_dir.into(),
        }
    }

    /// `structure_set/<id>.json`: placement and the weighted structure list.
    pub fn load_structure_set(
        &self,
        id: &Identifier,
    ) -> Result<StructureSetSpec, VillageDataError> {
        let value = self.read_entry(STRUCTURE_SET, "structure_set", id, id)?;
        let placement = parse_placement(id, field_of(&value, "placement", STRUCTURE_SET, id)?)?;
        let entries = field_array(&value, "structures", STRUCTURE_SET, id)?;
        let mut structures = Vec::with_capacity(entries.len());
        for entry in entries {
            structures.push(StructureSetEntry {
                structure: parse_id(id, field_str(entry, "structure", STRUCTURE_SET, id)?)?,
                // `ExtraCodecs.POSITIVE_INT`
                weight: ranged_i32_field(entry, "weight", STRUCTURE_SET, id, 1, i32::MAX)?,
            });
        }
        Ok(StructureSetSpec {
            id: id.clone(),
            placement,
            structures,
        })
    }

    /// `structure/<id>.json`: the settings a `minecraft:jigsaw` structure runs
    /// with.
    pub fn load_structure(&self, id: &Identifier) -> Result<StructureSpec, VillageDataError> {
        let value = self.read_entry(STRUCTURE, "structure", id, id)?;
        parse_structure(id, &value)
    }

    /// `template_pool/<id>.json`: the fallback and the weighted elements.
    pub fn load_template_pool(
        &self,
        id: &Identifier,
    ) -> Result<TemplatePoolSpec, VillageDataError> {
        let value = self.read_entry(TEMPLATE_POOL, "template_pool", id, id)?;
        parse_template_pool(id, &value)
    }

    /// `processor_list/<id>.json`: the processors in application order.
    pub fn load_processor_list(
        &self,
        id: &Identifier,
    ) -> Result<ProcessorListSpec, VillageDataError> {
        let value = self.read_entry(PROCESSOR_LIST, "processor_list", id, id)?;
        parse_processor_list(id, &value)
    }

    fn read_entry(
        &self,
        kind: EntryKind,
        dir: &str,
        id: &Identifier,
        referrer: &Identifier,
    ) -> Result<Value, VillageDataError> {
        if id.namespace() != "minecraft" {
            return Err(VillageDataError::UnsupportedNamespace {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
            });
        }
        let root = self.worldgen_dir.join(dir);
        let mut resource = ResourcePath::from_identifier_path(id)?;
        resource.set_extension("json")?;
        let lexical = resource.lexical_under(&root);
        let io_error = |path, source| VillageDataError::Io { path, source };
        let parse_error = |path, source| VillageDataError::Parse { path, source };
        let Some(opened) = resource.open_existing_under(&root)? else {
            return Err(VillageDataError::MissingEntry {
                kind,
                id: id.clone(),
                referrer: referrer.clone(),
                path: lexical,
            });
        };
        read_json_resource(opened, &io_error, &parse_error)
    }
}

fn parse_placement(
    referrer: &Identifier,
    value: &Value,
) -> Result<PlacementSpec, VillageDataError> {
    let type_id = parse_id(
        referrer,
        field_str(value, "type", STRUCTURE_PLACEMENT, referrer)?,
    )?;
    if type_id.path() != "random_spread" {
        return Err(VillageDataError::UnsupportedType {
            kind: STRUCTURE_PLACEMENT,
            type_id,
            referrer: referrer.clone(),
        });
    }
    let placement = RandomSpreadPlacement {
        spacing: ranged_i32_field(value, "spacing", STRUCTURE_PLACEMENT, referrer, 0, 4096)?,
        separation: ranged_i32_field(value, "separation", STRUCTURE_PLACEMENT, referrer, 0, 4096)?,
        salt: ranged_i32_field(value, "salt", STRUCTURE_PLACEMENT, referrer, 0, i32::MAX)?,
        spread_type: optional_enum_field(
            value,
            "spread_type",
            STRUCTURE_PLACEMENT,
            referrer,
            RANDOM_SPREAD_TYPES,
            RandomSpreadType::Linear,
        )?,
        locate_offset: parse_locate_offset(referrer, value)?,
        frequency_reduction_method: optional_enum_field(
            value,
            "frequency_reduction_method",
            STRUCTURE_PLACEMENT,
            referrer,
            FREQUENCY_REDUCTION_METHODS,
            FrequencyReductionMethod::Default,
        )?,
        frequency: optional_ranged_f32_field(
            value,
            "frequency",
            STRUCTURE_PLACEMENT,
            referrer,
            0.0,
            1.0,
            1.0,
        )?,
        exclusion_zone: parse_exclusion_zone(referrer, value.get("exclusion_zone"))?,
    };
    // `RandomSpreadStructurePlacement.validate`
    if placement.spacing <= placement.separation {
        return Err(VillageDataError::InvalidField {
            kind: STRUCTURE_PLACEMENT,
            entry: referrer.clone(),
            field: "spacing",
            value: format!(
                "{} is not larger than separation {}",
                placement.spacing, placement.separation
            ),
        });
    }
    Ok(PlacementSpec::RandomSpread(placement))
}

fn parse_locate_offset(referrer: &Identifier, value: &Value) -> Result<[i32; 3], VillageDataError> {
    // `Vec3i.offsetCodec(16)`, absent by default.
    let Some(offset) = value.get("locate_offset") else {
        return Ok([0, 0, 0]);
    };
    let entries = offset
        .as_array()
        .ok_or_else(|| VillageDataError::InvalidField {
            kind: STRUCTURE_PLACEMENT,
            entry: referrer.clone(),
            field: "locate_offset",
            value: offset.to_string(),
        })?;
    let [x, y, z] = entries.as_slice() else {
        return Err(VillageDataError::InvalidField {
            kind: STRUCTURE_PLACEMENT,
            entry: referrer.clone(),
            field: "locate_offset",
            value: offset.to_string(),
        });
    };
    let mut parsed = [0; 3];
    for (slot, raw) in parsed.iter_mut().zip([x, y, z]) {
        let coordinate = raw.as_i64().and_then(|raw| i32::try_from(raw).ok());
        // `offsetCodec(16)` accepts strictly less than 16 per axis.
        match coordinate {
            Some(coordinate) if (-15..=15).contains(&coordinate) => *slot = coordinate,
            _ => {
                return Err(VillageDataError::InvalidField {
                    kind: STRUCTURE_PLACEMENT,
                    entry: referrer.clone(),
                    field: "locate_offset",
                    value: offset.to_string(),
                });
            }
        }
    }
    Ok(parsed)
}

fn parse_exclusion_zone(
    referrer: &Identifier,
    value: Option<&Value>,
) -> Result<Option<ExclusionZone>, VillageDataError> {
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(Some(ExclusionZone {
        other_set: parse_id(
            referrer,
            field_str(value, "other_set", EXCLUSION_ZONE, referrer)?,
        )?,
        chunk_count: ranged_i32_field(value, "chunk_count", EXCLUSION_ZONE, referrer, 1, 16)?,
    }))
}

fn parse_structure(id: &Identifier, value: &Value) -> Result<StructureSpec, VillageDataError> {
    let structure_type = parse_id(id, field_str(value, "type", STRUCTURE, id)?)?;
    if structure_type.path() != "jigsaw" {
        return Err(VillageDataError::UnsupportedType {
            kind: STRUCTURE,
            type_id: structure_type,
            referrer: id.clone(),
        });
    }
    // Pool aliases rewrite which pool an element resolves to; ignoring them
    // would silently place the wrong pieces.
    if let Some(aliases) = value.get("pool_aliases")
        && !aliases.as_array().is_some_and(Vec::is_empty)
    {
        return Err(VillageDataError::UnsupportedField {
            kind: STRUCTURE,
            entry: id.clone(),
            field: "pool_aliases",
            value: aliases.to_string(),
        });
    }
    let terrain_adaptation = optional_enum_field(
        value,
        "terrain_adaptation",
        STRUCTURE,
        id,
        TERRAIN_ADAPTATIONS,
        TerrainAdaptation::None,
    )?;
    let max_distance_from_center = parse_max_distance(
        id,
        field_of(value, "max_distance_from_center", STRUCTURE, id)?,
    )?;
    // `JigsawStructure.verifyRange`
    let edge = match terrain_adaptation {
        TerrainAdaptation::None => 0,
        TerrainAdaptation::Bury
        | TerrainAdaptation::BeardThin
        | TerrainAdaptation::BeardBox
        | TerrainAdaptation::Encapsulate => 12,
    };
    if max_distance_from_center.horizontal + edge > MAX_STRUCTURE_RANGE {
        return Err(VillageDataError::InvalidField {
            kind: STRUCTURE,
            entry: id.clone(),
            field: "max_distance_from_center",
            value: format!(
                "{} plus terrain adaptation {edge} exceeds {MAX_STRUCTURE_RANGE}",
                max_distance_from_center.horizontal
            ),
        });
    }
    Ok(StructureSpec {
        id: id.clone(),
        structure_type,
        biomes: parse_biome_set(id, field_of(value, "biomes", STRUCTURE, id)?)?,
        spawn_overrides: parse_spawn_overrides(
            id,
            field_of(value, "spawn_overrides", STRUCTURE, id)?,
        )?,
        step: enum_field(value, "step", STRUCTURE, id, STRUCTURE_STEPS)?,
        terrain_adaptation,
        start_pool: parse_id(id, field_str(value, "start_pool", STRUCTURE, id)?)?,
        start_jigsaw_name: parse_optional_id(value, "start_jigsaw_name", STRUCTURE, id)?,
        // `Codec.intRange(0, 20)`
        size: ranged_i32_field(value, "size", STRUCTURE, id, 0, 20)?,
        start_height: parse_height_provider(id, field_of(value, "start_height", STRUCTURE, id)?)?,
        use_expansion_hack: bool_field(value, "use_expansion_hack", STRUCTURE, id)?,
        project_start_to_heightmap: parse_optional_enum_field(
            value,
            "project_start_to_heightmap",
            STRUCTURE,
            id,
            HEIGHTMAP_TYPES,
        )?,
        max_distance_from_center,
        dimension_padding: parse_dimension_padding(id, value.get("dimension_padding"))?,
        liquid_settings: optional_enum_field(
            value,
            "liquid_settings",
            STRUCTURE,
            id,
            LIQUID_SETTINGS,
            LiquidSettings::ApplyWaterlogging,
        )?,
    })
}

fn parse_biome_set(id: &Identifier, value: &Value) -> Result<BiomeSet, VillageDataError> {
    match value {
        // A `#`-prefixed string is a tag; a bare identifier is a one-entry list.
        Value::String(raw) => match raw.strip_prefix('#') {
            Some(tag) => Ok(BiomeSet::Tag(parse_id(id, tag)?)),
            None => Ok(BiomeSet::Biomes(vec![parse_id(id, raw)?])),
        },
        Value::Array(entries) => {
            let mut biomes = Vec::with_capacity(entries.len());
            for entry in entries {
                let Some(raw) = entry.as_str() else {
                    return Err(VillageDataError::InvalidField {
                        kind: STRUCTURE,
                        entry: id.clone(),
                        field: "biomes",
                        value: entry.to_string(),
                    });
                };
                biomes.push(parse_id(id, raw)?);
            }
            Ok(BiomeSet::Biomes(biomes))
        }
        other => Err(VillageDataError::InvalidField {
            kind: STRUCTURE,
            entry: id.clone(),
            field: "biomes",
            value: other.to_string(),
        }),
    }
}

fn parse_spawn_overrides(
    id: &Identifier,
    value: &Value,
) -> Result<Vec<SpawnOverrideEntry>, VillageDataError> {
    let Some(object) = value.as_object() else {
        return Err(VillageDataError::InvalidField {
            kind: STRUCTURE,
            entry: id.clone(),
            field: "spawn_overrides",
            value: value.to_string(),
        });
    };
    let mut overrides = Vec::with_capacity(object.len());
    for (category, override_value) in object {
        let Some(category) = mob_category(category) else {
            return Err(VillageDataError::InvalidField {
                kind: SPAWN_OVERRIDE,
                entry: id.clone(),
                field: "spawn_overrides",
                value: category.clone(),
            });
        };
        overrides.push(SpawnOverrideEntry {
            category,
            bounding_box: enum_field(
                override_value,
                "bounding_box",
                SPAWN_OVERRIDE,
                id,
                SPAWN_BOUNDING_BOXES,
            )?,
            spawns: parse_weighted_spawns(
                id,
                field_array(override_value, "spawns", SPAWN_OVERRIDE, id)?,
            )?,
        });
    }
    Ok(overrides)
}

fn parse_weighted_spawns(
    id: &Identifier,
    entries: &[Value],
) -> Result<Vec<WeightedSpawn>, VillageDataError> {
    let mut spawns = Vec::with_capacity(entries.len());
    for entry in entries {
        let data = field_of(entry, "data", SPAWN_OVERRIDE, id)?;
        let min_count = ranged_i32_field(data, "minCount", SPAWN_OVERRIDE, id, 1, i32::MAX)?;
        let max_count = ranged_i32_field(data, "maxCount", SPAWN_OVERRIDE, id, 1, i32::MAX)?;
        if min_count > max_count {
            return Err(VillageDataError::InvalidField {
                kind: SPAWN_OVERRIDE,
                entry: id.clone(),
                field: "minCount",
                value: format!("{min_count} is larger than maxCount {max_count}"),
            });
        }
        spawns.push(WeightedSpawn {
            weight: ranged_i32_field(entry, "weight", SPAWN_OVERRIDE, id, 0, i32::MAX)?,
            entity_type: parse_id(id, field_str(data, "type", SPAWN_OVERRIDE, id)?)?,
            min_count,
            max_count,
        });
    }
    Ok(spawns)
}

fn parse_max_distance(id: &Identifier, value: &Value) -> Result<MaxDistance, VillageDataError> {
    // `Codec.either(full, horizontal only)`: one integer means both axes.
    if let Some(horizontal) = value.as_i64().and_then(|raw| i32::try_from(raw).ok()) {
        checked_range(
            id,
            horizontal,
            "max_distance_from_center",
            1,
            MAX_STRUCTURE_RANGE,
        )?;
        return Ok(MaxDistance {
            horizontal,
            vertical: horizontal,
        });
    }
    let horizontal = ranged_i32_field(value, "horizontal", STRUCTURE, id, 1, MAX_STRUCTURE_RANGE)?;
    let vertical = optional_ranged_i32_field(value, "vertical", STRUCTURE, id, 1, Y_SIZE, Y_SIZE)?;
    Ok(MaxDistance {
        horizontal,
        vertical,
    })
}

fn parse_dimension_padding(
    id: &Identifier,
    value: Option<&Value>,
) -> Result<DimensionPadding, VillageDataError> {
    // `Codec.either(non-negative int, {bottom, top})`, both zero by default.
    let Some(value) = value else {
        return Ok(DimensionPadding { bottom: 0, top: 0 });
    };
    if let Some(both) = value.as_i64().and_then(|raw| i32::try_from(raw).ok()) {
        checked_range(id, both, "dimension_padding", 0, i32::MAX)?;
        return Ok(DimensionPadding {
            bottom: both,
            top: both,
        });
    }
    Ok(DimensionPadding {
        bottom: optional_ranged_i32_field(value, "bottom", STRUCTURE, id, 0, i32::MAX, 0)?,
        top: optional_ranged_i32_field(value, "top", STRUCTURE, id, 0, i32::MAX, 0)?,
    })
}

fn parse_height_provider(
    id: &Identifier,
    value: &Value,
) -> Result<HeightProviderSpec, VillageDataError> {
    // `HeightProvider.CODEC` is an either: a bare `VerticalAnchor` is the
    // constant provider, anything else dispatches on `type`.
    let Some(object) = value.as_object() else {
        return Err(VillageDataError::InvalidField {
            kind: HEIGHT_PROVIDER,
            entry: id.clone(),
            field: "start_height",
            value: value.to_string(),
        });
    };
    if !object.contains_key("type") {
        return Ok(HeightProviderSpec::Constant(parse_vertical_anchor(
            id,
            "start_height",
            value,
        )?));
    }
    let type_id = parse_id(id, field_str(value, "type", HEIGHT_PROVIDER, id)?)?;
    Ok(match type_id.path() {
        "constant" => HeightProviderSpec::Constant(parse_vertical_anchor(
            id,
            "start_height",
            field_of(value, "value", HEIGHT_PROVIDER, id)?,
        )?),
        "uniform" => HeightProviderSpec::Uniform {
            min_inclusive: parse_vertical_anchor(
                id,
                "start_height",
                field_of(value, "min_inclusive", HEIGHT_PROVIDER, id)?,
            )?,
            max_inclusive: parse_vertical_anchor(
                id,
                "start_height",
                field_of(value, "max_inclusive", HEIGHT_PROVIDER, id)?,
            )?,
        },
        "trapezoid" => HeightProviderSpec::Trapezoid {
            min_inclusive: parse_vertical_anchor(
                id,
                "start_height",
                field_of(value, "min_inclusive", HEIGHT_PROVIDER, id)?,
            )?,
            max_inclusive: parse_vertical_anchor(
                id,
                "start_height",
                field_of(value, "max_inclusive", HEIGHT_PROVIDER, id)?,
            )?,
            plateau: optional_i32_field(value, "plateau", HEIGHT_PROVIDER, id, 0)?,
        },
        _ => {
            return Err(VillageDataError::UnsupportedType {
                kind: HEIGHT_PROVIDER,
                type_id,
                referrer: id.clone(),
            });
        }
    })
}

fn parse_vertical_anchor(
    id: &Identifier,
    field: &'static str,
    value: &Value,
) -> Result<VerticalAnchor, VillageDataError> {
    let invalid = || VillageDataError::InvalidField {
        kind: HEIGHT_PROVIDER,
        entry: id.clone(),
        field,
        value: value.to_string(),
    };
    // `Codec.xor` of the three anchor codecs: exactly one key, in range.
    let Some(object) = value.as_object() else {
        return Err(invalid());
    };
    let mut keys = object.iter();
    let Some((name, raw)) = keys.next() else {
        return Err(invalid());
    };
    if keys.next().is_some() {
        return Err(invalid());
    }
    let Some(offset) = raw.as_i64().and_then(|raw| i32::try_from(raw).ok()) else {
        return Err(invalid());
    };
    if !(MIN_Y..=MAX_Y).contains(&offset) {
        return Err(invalid());
    }
    match name.as_str() {
        "absolute" => Ok(VerticalAnchor::Absolute(offset)),
        "above_bottom" => Ok(VerticalAnchor::AboveBottom(offset)),
        "below_top" => Ok(VerticalAnchor::BelowTop(offset)),
        _ => Err(invalid()),
    }
}

fn parse_template_pool(
    id: &Identifier,
    value: &Value,
) -> Result<TemplatePoolSpec, VillageDataError> {
    let fallback = parse_id(id, field_str(value, "fallback", TEMPLATE_POOL, id)?)?;
    let entries = field_array(value, "elements", TEMPLATE_POOL, id)?;
    let mut elements = Vec::with_capacity(entries.len());
    for entry in entries {
        elements.push(PoolElementEntry {
            // `Codec.intRange(1, 150)`
            weight: ranged_i32_field(entry, "weight", TEMPLATE_POOL, id, 1, 150)?,
            element: parse_pool_element(id, field_of(entry, "element", TEMPLATE_POOL, id)?)?,
        });
    }
    Ok(TemplatePoolSpec {
        id: id.clone(),
        fallback,
        elements,
    })
}

fn parse_pool_element(
    referrer: &Identifier,
    value: &Value,
) -> Result<PoolElementSpec, VillageDataError> {
    let type_id = parse_id(
        referrer,
        field_str(value, "element_type", POOL_ELEMENT, referrer)?,
    )?;
    Ok(match type_id.path() {
        // A unit codec: the element carries no fields of its own.
        "empty_pool_element" => PoolElementSpec::Empty,
        "single_pool_element" | "legacy_single_pool_element" => {
            let single = parse_single_element(referrer, value)?;
            if type_id.path() == "single_pool_element" {
                PoolElementSpec::Single(single)
            } else {
                PoolElementSpec::LegacySingle(single)
            }
        }
        "list_pool_element" => {
            let elements = field_array(value, "elements", POOL_ELEMENT, referrer)?;
            if elements.is_empty() {
                // Vanilla refuses an empty list element outright.
                return Err(VillageDataError::InvalidField {
                    kind: POOL_ELEMENT,
                    entry: referrer.clone(),
                    field: "elements",
                    value: "empty list pool element".to_owned(),
                });
            }
            let mut parsed = Vec::with_capacity(elements.len());
            for element in elements {
                parsed.push(parse_pool_element(referrer, element)?);
            }
            PoolElementSpec::List {
                elements: parsed,
                projection: enum_field(value, "projection", POOL_ELEMENT, referrer, PROJECTIONS)?,
            }
        }
        "feature_pool_element" => PoolElementSpec::Feature {
            feature: parse_id(
                referrer,
                field_str(value, "feature", POOL_ELEMENT, referrer)?,
            )?,
            projection: enum_field(value, "projection", POOL_ELEMENT, referrer, PROJECTIONS)?,
        },
        _ => {
            return Err(VillageDataError::UnsupportedType {
                kind: POOL_ELEMENT,
                type_id,
                referrer: referrer.clone(),
            });
        }
    })
}

fn parse_single_element(
    referrer: &Identifier,
    value: &Value,
) -> Result<SingleElementSpec, VillageDataError> {
    Ok(SingleElementSpec {
        location: parse_id(
            referrer,
            field_str(value, "location", POOL_ELEMENT, referrer)?,
        )?,
        projection: enum_field(value, "projection", POOL_ELEMENT, referrer, PROJECTIONS)?,
        processors: parse_processor_ref(
            referrer,
            field_of(value, "processors", POOL_ELEMENT, referrer)?,
        )?,
        override_liquid_settings: parse_optional_enum_field(
            value,
            "override_liquid_settings",
            POOL_ELEMENT,
            referrer,
            LIQUID_SETTINGS,
        )?,
    })
}

fn parse_processor_ref(
    referrer: &Identifier,
    value: &Value,
) -> Result<ProcessorRef, VillageDataError> {
    // `StructureProcessorType.LIST_CODEC`: a list reference, a bare processor
    // array, or `{"processors": [...]}`.
    match value {
        Value::String(raw) => Ok(ProcessorRef::List(parse_id(referrer, raw)?)),
        Value::Array(entries) => Ok(ProcessorRef::Inline(parse_processors(referrer, entries)?)),
        Value::Object(object) if object.contains_key("processors") => {
            Ok(ProcessorRef::Inline(parse_processors(
                referrer,
                field_array(value, "processors", PROCESSOR_LIST, referrer)?,
            )?))
        }
        other => Err(VillageDataError::InvalidField {
            kind: PROCESSOR_LIST,
            entry: referrer.clone(),
            field: "processors",
            value: other.to_string(),
        }),
    }
}

fn parse_processors(
    referrer: &Identifier,
    entries: &[Value],
) -> Result<Vec<StructureProcessorSpec>, VillageDataError> {
    let mut processors = Vec::with_capacity(entries.len());
    for entry in entries {
        processors.push(parse_processor(referrer, entry)?);
    }
    Ok(processors)
}

fn parse_processor_list(
    id: &Identifier,
    value: &Value,
) -> Result<ProcessorListSpec, VillageDataError> {
    // A list file holds either `{"processors": [...]}` or a bare array.
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(_) => field_array(value, "processors", PROCESSOR_LIST, id)?,
        other => {
            return Err(VillageDataError::InvalidField {
                kind: PROCESSOR_LIST,
                entry: id.clone(),
                field: "processors",
                value: other.to_string(),
            });
        }
    };
    Ok(ProcessorListSpec {
        id: id.clone(),
        processors: parse_processors(id, entries)?,
    })
}

fn parse_processor(
    referrer: &Identifier,
    value: &Value,
) -> Result<StructureProcessorSpec, VillageDataError> {
    let type_id = parse_id(
        referrer,
        field_str(value, "processor_type", PROCESSOR, referrer)?,
    )?;
    Ok(match type_id.path() {
        "rule" => {
            let entries = field_array(value, "rules", PROCESSOR, referrer)?;
            let mut rules = Vec::with_capacity(entries.len());
            for entry in entries {
                rules.push(parse_processor_rule(referrer, entry)?);
            }
            StructureProcessorSpec::Rule { rules }
        }
        "block_age" => StructureProcessorSpec::BlockAge {
            mossiness: f32_field(value, "mossiness", PROCESSOR, referrer)?,
        },
        "gravity" => StructureProcessorSpec::Gravity {
            // `orElse` on both fields: `world_surface_wg` and 0.
            heightmap: optional_enum_field(
                value,
                "heightmap",
                PROCESSOR,
                referrer,
                HEIGHTMAP_TYPES,
                HeightmapType::WorldSurfaceWg,
            )?,
            offset: optional_i32_field(value, "offset", PROCESSOR, referrer, 0)?,
        },
        "jigsaw_replacement" => StructureProcessorSpec::JigsawReplacement,
        "protected_blocks" => StructureProcessorSpec::ProtectedBlocks {
            cannot_replace: parse_hashed_tag(
                referrer,
                field_of(value, "value", PROCESSOR, referrer)?,
            )?,
        },
        _ => {
            return Err(VillageDataError::UnsupportedType {
                kind: PROCESSOR,
                type_id,
                referrer: referrer.clone(),
            });
        }
    })
}

fn parse_processor_rule(
    referrer: &Identifier,
    value: &Value,
) -> Result<ProcessorRuleSpec, VillageDataError> {
    // The only block entity modifier this layer accepts is vanilla's default;
    // `append_loot`, `append_static` and `clear` change item NBT and are not
    // implemented.
    if let Some(modifier) = value.get("block_entity_modifier") {
        let type_id = parse_id(referrer, field_str(modifier, "type", PROCESSOR, referrer)?)?;
        if type_id.path() != "passthrough" {
            return Err(VillageDataError::UnsupportedType {
                kind: PROCESSOR,
                type_id,
                referrer: referrer.clone(),
            });
        }
    }
    Ok(ProcessorRuleSpec {
        input_predicate: parse_rule_test(
            referrer,
            field_of(value, "input_predicate", PROCESSOR, referrer)?,
        )?,
        location_predicate: parse_rule_test(
            referrer,
            field_of(value, "location_predicate", PROCESSOR, referrer)?,
        )?,
        position_predicate: parse_pos_rule_test(value.get("position_predicate"), referrer)?,
        output_state: parse_block_state(
            referrer,
            field_of(value, "output_state", PROCESSOR, referrer)?,
        )?,
    })
}

fn parse_rule_test(referrer: &Identifier, value: &Value) -> Result<RuleTestSpec, VillageDataError> {
    let type_id = parse_id(
        referrer,
        field_str(value, "predicate_type", RULE_TEST, referrer)?,
    )?;
    Ok(match type_id.path() {
        "always_true" => RuleTestSpec::AlwaysTrue,
        "block_match" => RuleTestSpec::BlockMatch {
            block: parse_id(referrer, field_str(value, "block", RULE_TEST, referrer)?)?,
        },
        "blockstate_match" => RuleTestSpec::BlockStateMatch {
            state: parse_block_state(
                referrer,
                field_of(value, "block_state", RULE_TEST, referrer)?,
            )?,
        },
        "tag_match" => RuleTestSpec::TagMatch {
            tag: parse_id(referrer, field_str(value, "tag", RULE_TEST, referrer)?)?,
        },
        "random_block_match" => RuleTestSpec::RandomBlockMatch {
            block: parse_id(referrer, field_str(value, "block", RULE_TEST, referrer)?)?,
            probability: f32_field(value, "probability", RULE_TEST, referrer)?,
        },
        "random_blockstate_match" => RuleTestSpec::RandomBlockStateMatch {
            state: parse_block_state(
                referrer,
                field_of(value, "block_state", RULE_TEST, referrer)?,
            )?,
            probability: f32_field(value, "probability", RULE_TEST, referrer)?,
        },
        _ => {
            return Err(VillageDataError::UnsupportedType {
                kind: RULE_TEST,
                type_id,
                referrer: referrer.clone(),
            });
        }
    })
}

fn parse_pos_rule_test(
    value: Option<&Value>,
    referrer: &Identifier,
) -> Result<PosRuleTestSpec, VillageDataError> {
    // `PosRuleTest.CODEC`, `always_true` by default.
    let Some(value) = value else {
        return Ok(PosRuleTestSpec::AlwaysTrue);
    };
    let type_id = parse_id(
        referrer,
        field_str(value, "predicate_type", POS_RULE_TEST, referrer)?,
    )?;
    let chances = |value: &Value| -> Result<(f32, f32, i32, i32), VillageDataError> {
        Ok((
            optional_f32_field(value, "min_chance", POS_RULE_TEST, referrer, 0.0)?,
            optional_f32_field(value, "max_chance", POS_RULE_TEST, referrer, 0.0)?,
            optional_i32_field(value, "min_dist", POS_RULE_TEST, referrer, 0)?,
            optional_i32_field(value, "max_dist", POS_RULE_TEST, referrer, 0)?,
        ))
    };
    Ok(match type_id.path() {
        "always_true" => PosRuleTestSpec::AlwaysTrue,
        "linear_pos" => {
            let (min_chance, max_chance, min_dist, max_dist) = chances(value)?;
            PosRuleTestSpec::LinearPos {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
            }
        }
        "axis_aligned_linear_pos" => {
            let (min_chance, max_chance, min_dist, max_dist) = chances(value)?;
            PosRuleTestSpec::AxisAlignedLinearPos {
                min_chance,
                max_chance,
                min_dist,
                max_dist,
                axis: optional_enum_field(value, "axis", POS_RULE_TEST, referrer, AXES, Axis::Y)?,
            }
        }
        _ => {
            return Err(VillageDataError::UnsupportedType {
                kind: POS_RULE_TEST,
                type_id,
                referrer: referrer.clone(),
            });
        }
    })
}

/// `{"Name": ..., "Properties": {...}}`, the JSON form of a block state.
fn parse_block_state(
    referrer: &Identifier,
    value: &Value,
) -> Result<BlockStateSpec, VillageDataError> {
    let name = field_str(value, "Name", BLOCK_STATE, referrer)?;
    let mut properties = Vec::new();
    if let Some(values) = value.get("Properties") {
        let Some(values) = values.as_object() else {
            return Err(VillageDataError::InvalidField {
                kind: BLOCK_STATE,
                entry: referrer.clone(),
                field: "Properties",
                value: values.to_string(),
            });
        };
        for (key, value) in values {
            let Some(value) = value.as_str() else {
                return Err(VillageDataError::InvalidField {
                    kind: BLOCK_STATE,
                    entry: referrer.clone(),
                    field: "Properties",
                    value: format!("{key}={value}"),
                });
            };
            properties.push((key.clone(), value.to_owned()));
        }
    }
    Ok(BlockStateSpec {
        block: parse_id(referrer, name)?,
        properties,
    })
}

/// `TagKey.hashedCodec`: the stored form carries the `#`.
fn parse_hashed_tag(referrer: &Identifier, value: &Value) -> Result<Identifier, VillageDataError> {
    let Some(raw) = value.as_str() else {
        return Err(VillageDataError::InvalidField {
            kind: PROCESSOR,
            entry: referrer.clone(),
            field: "value",
            value: value.to_string(),
        });
    };
    let Some(tag) = raw.strip_prefix('#') else {
        return Err(VillageDataError::InvalidField {
            kind: PROCESSOR,
            entry: referrer.clone(),
            field: "value",
            value: raw.to_owned(),
        });
    };
    parse_id(referrer, tag)
}

const RANDOM_SPREAD_TYPES: &[(&str, RandomSpreadType)] = &[
    ("linear", RandomSpreadType::Linear),
    ("triangular", RandomSpreadType::Triangular),
];

const FREQUENCY_REDUCTION_METHODS: &[(&str, FrequencyReductionMethod)] = &[
    ("default", FrequencyReductionMethod::Default),
    ("legacy_type_1", FrequencyReductionMethod::LegacyType1),
    ("legacy_type_2", FrequencyReductionMethod::LegacyType2),
    ("legacy_type_3", FrequencyReductionMethod::LegacyType3),
];

const TERRAIN_ADAPTATIONS: &[(&str, TerrainAdaptation)] = &[
    ("none", TerrainAdaptation::None),
    ("bury", TerrainAdaptation::Bury),
    ("beard_thin", TerrainAdaptation::BeardThin),
    ("beard_box", TerrainAdaptation::BeardBox),
    ("encapsulate", TerrainAdaptation::Encapsulate),
];

const STRUCTURE_STEPS: &[(&str, StructureStep)] = &[
    ("raw_generation", StructureStep::RawGeneration),
    ("lakes", StructureStep::Lakes),
    ("local_modifications", StructureStep::LocalModifications),
    (
        "underground_structures",
        StructureStep::UndergroundStructures,
    ),
    ("surface_structures", StructureStep::SurfaceStructures),
    ("strongholds", StructureStep::Strongholds),
    ("underground_ores", StructureStep::UndergroundOres),
    (
        "underground_decoration",
        StructureStep::UndergroundDecoration,
    ),
    ("fluid_springs", StructureStep::FluidSprings),
    ("vegetal_decoration", StructureStep::VegetalDecoration),
    (
        "top_layer_modification",
        StructureStep::TopLayerModification,
    ),
];

const HEIGHTMAP_TYPES: &[(&str, HeightmapType)] = &[
    ("WORLD_SURFACE_WG", HeightmapType::WorldSurfaceWg),
    ("WORLD_SURFACE", HeightmapType::WorldSurface),
    ("OCEAN_FLOOR_WG", HeightmapType::OceanFloorWg),
    ("OCEAN_FLOOR", HeightmapType::OceanFloor),
    ("MOTION_BLOCKING", HeightmapType::MotionBlocking),
    (
        "MOTION_BLOCKING_NO_LEAVES",
        HeightmapType::MotionBlockingNoLeaves,
    ),
];

const LIQUID_SETTINGS: &[(&str, LiquidSettings)] = &[
    ("ignore_waterlogging", LiquidSettings::IgnoreWaterlogging),
    ("apply_waterlogging", LiquidSettings::ApplyWaterlogging),
];

const SPAWN_BOUNDING_BOXES: &[(&str, SpawnBoundingBox)] = &[
    ("piece", SpawnBoundingBox::Piece),
    ("full", SpawnBoundingBox::Full),
];

const PROJECTIONS: &[(&str, Projection)] = &[
    ("rigid", Projection::Rigid),
    ("terrain_matching", Projection::TerrainMatching),
];

const AXES: &[(&str, Axis)] = &[("x", Axis::X), ("y", Axis::Y), ("z", Axis::Z)];

fn mob_category(name: &str) -> Option<MobCategory> {
    Some(match name {
        "monster" => MobCategory::Monster,
        "creature" => MobCategory::Creature,
        "ambient" => MobCategory::Ambient,
        "axolotls" => MobCategory::Axolotls,
        "underground_water_creature" => MobCategory::UndergroundWaterCreature,
        "water_creature" => MobCategory::WaterCreature,
        "water_ambient" => MobCategory::WaterAmbient,
        "misc" => MobCategory::Misc,
        _ => return None,
    })
}

fn parse_id(referrer: &Identifier, value: &str) -> Result<Identifier, VillageDataError> {
    Identifier::parse(value.to_owned()).map_err(|_| VillageDataError::InvalidIdentifier {
        referrer: referrer.clone(),
        value: value.to_owned(),
    })
}

fn parse_optional_id(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<Option<Identifier>, VillageDataError> {
    match value.get(field) {
        None => Ok(None),
        Some(raw) => Ok(Some(parse_id(
            entry,
            raw.as_str().ok_or_else(|| VillageDataError::InvalidField {
                kind,
                entry: entry.clone(),
                field,
                value: raw.to_string(),
            })?,
        )?)),
    }
}

fn field_of<'a>(
    value: &'a Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<&'a Value, VillageDataError> {
    value
        .get(field)
        .ok_or_else(|| VillageDataError::MissingField {
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
) -> Result<&'a Vec<Value>, VillageDataError> {
    field_of(value, field, kind, entry)?
        .as_array()
        .ok_or_else(|| VillageDataError::InvalidField {
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
) -> Result<&'a str, VillageDataError> {
    field_of(value, field, kind, entry)?
        .as_str()
        .ok_or_else(|| VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a string".to_owned(),
        })
}

fn bool_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<bool, VillageDataError> {
    field_of(value, field, kind, entry)?
        .as_bool()
        .ok_or_else(|| VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a boolean".to_owned(),
        })
}

fn i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<i32, VillageDataError> {
    let raw = field_of(value, field, kind, entry)?
        .as_i64()
        .ok_or_else(|| VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not an integer".to_owned(),
        })?;
    i32::try_from(raw).map_err(|_| VillageDataError::InvalidField {
        kind,
        entry: entry.clone(),
        field,
        value: raw.to_string(),
    })
}

fn f32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
) -> Result<f32, VillageDataError> {
    let raw = field_of(value, field, kind, entry)?
        .as_f64()
        .ok_or_else(|| VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: "not a number".to_owned(),
        })?;
    Ok(raw as f32)
}

fn optional_i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    default: i32,
) -> Result<i32, VillageDataError> {
    match value.get(field) {
        None => Ok(default),
        Some(_) => i32_field(value, field, kind, entry),
    }
}

fn optional_f32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    default: f32,
) -> Result<f32, VillageDataError> {
    match value.get(field) {
        None => Ok(default),
        Some(_) => f32_field(value, field, kind, entry),
    }
}

fn enum_field<T: Copy>(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    table: &[(&str, T)],
) -> Result<T, VillageDataError> {
    let raw = field_str(value, field, kind, entry)?;
    table
        .iter()
        .find_map(|(name, parsed)| (*name == raw).then_some(*parsed))
        .ok_or_else(|| VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: raw.to_owned(),
        })
}

fn optional_enum_field<T: Copy>(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    table: &[(&str, T)],
    default: T,
) -> Result<T, VillageDataError> {
    match value.get(field) {
        None => Ok(default),
        Some(_) => enum_field(value, field, kind, entry, table),
    }
}

fn parse_optional_enum_field<T: Copy>(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    table: &[(&str, T)],
) -> Result<Option<T>, VillageDataError> {
    match value.get(field) {
        None => Ok(None),
        Some(_) => Ok(Some(enum_field(value, field, kind, entry, table)?)),
    }
}

fn checked_range(
    entry: &Identifier,
    parsed: i32,
    field: &'static str,
    min: i32,
    max: i32,
) -> Result<i32, VillageDataError> {
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(VillageDataError::InvalidField {
            kind: STRUCTURE,
            entry: entry.clone(),
            field,
            value: format!("{parsed} is outside {min}..={max}"),
        })
    }
}

fn ranged_i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    min: i32,
    max: i32,
) -> Result<i32, VillageDataError> {
    let parsed = i32_field(value, field, kind, entry)?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: format!("{parsed} is outside {min}..={max}"),
        })
    }
}

fn optional_ranged_i32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    min: i32,
    max: i32,
    default: i32,
) -> Result<i32, VillageDataError> {
    match value.get(field) {
        None => Ok(default),
        Some(_) => ranged_i32_field(value, field, kind, entry, min, max),
    }
}

fn optional_ranged_f32_field(
    value: &Value,
    field: &'static str,
    kind: EntryKind,
    entry: &Identifier,
    min: f32,
    max: f32,
    default: f32,
) -> Result<f32, VillageDataError> {
    if value.get(field).is_none() {
        return Ok(default);
    }
    let parsed = f32_field(value, field, kind, entry)?;
    if (min..=max).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(VillageDataError::InvalidField {
            kind,
            entry: entry.clone(),
            field,
            value: format!("{parsed} is outside {min}..={max}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    fn identifier(value: &str) -> Identifier {
        Identifier::parse(value.to_owned()).unwrap()
    }

    fn loader(root: &Path) -> VillageDataLoader {
        VillageDataLoader::new(root.join("data/minecraft/worldgen"))
    }

    fn write(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/structure_set/villages.json",
            r#"{
                "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 10387312 },
                "structures": [{ "structure": "minecraft:village_plains", "weight": 1 }]
            }"#,
        );
        write(
            dir.path(),
            "data/minecraft/worldgen/structure/village_plains.json",
            r##"{
                "type": "minecraft:jigsaw",
                "biomes": "#minecraft:has_structure/village_plains",
                "max_distance_from_center": 80,
                "project_start_to_heightmap": "WORLD_SURFACE_WG",
                "size": 6,
                "spawn_overrides": {},
                "start_height": { "absolute": 0 },
                "start_pool": "minecraft:village/plains/town_centers",
                "step": "surface_structures",
                "terrain_adaptation": "beard_thin",
                "use_expansion_hack": true
            }"##,
        );
        write(
            dir.path(),
            "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": "minecraft:zombie_plains",
                            "projection": "rigid"
                        },
                        "weight": 2
                    }
                ]
            }"#,
        );
        write(
            dir.path(),
            "data/minecraft/worldgen/template_pool/empty.json",
            r#"{ "fallback": "minecraft:empty", "elements": [] }"#,
        );
        write(
            dir.path(),
            "data/minecraft/worldgen/processor_list/zombie_plains.json",
            r#"{
                "processors": [
                    {
                        "processor_type": "minecraft:rule",
                        "rules": [
                            {
                                "input_predicate": { "predicate_type": "minecraft:always_true" },
                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                "output_state": { "Name": "minecraft:cobweb" }
                            }
                        ]
                    }
                ]
            }"#,
        );
        dir
    }

    /// Every field of a structure set survives the load: placement, its
    /// optional parameters, and each weighted structure.
    #[test]
    fn structure_set_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/structure_set/villages.json",
            r#"{
                "placement": {
                    "type": "minecraft:random_spread",
                    "spacing": 34,
                    "separation": 8,
                    "salt": 10387312,
                    "spread_type": "triangular",
                    "locate_offset": [4, 0, -4],
                    "frequency_reduction_method": "legacy_type_1",
                    "frequency": 0.5,
                    "exclusion_zone": { "other_set": "minecraft:monuments", "chunk_count": 4 }
                },
                "structures": [
                    { "structure": "minecraft:village_plains", "weight": 1 },
                    { "structure": "minecraft:village_desert", "weight": 3 }
                ]
            }"#,
        );
        let spec = loader(dir.path())
            .load_structure_set(&identifier("minecraft:villages"))
            .unwrap();
        assert_eq!(
            spec,
            StructureSetSpec {
                id: identifier("minecraft:villages"),
                placement: PlacementSpec::RandomSpread(RandomSpreadPlacement {
                    spacing: 34,
                    separation: 8,
                    salt: 10387312,
                    spread_type: RandomSpreadType::Triangular,
                    locate_offset: [4, 0, -4],
                    frequency_reduction_method: FrequencyReductionMethod::LegacyType1,
                    frequency: 0.5,
                    exclusion_zone: Some(ExclusionZone {
                        other_set: identifier("minecraft:monuments"),
                        chunk_count: 4,
                    }),
                }),
                structures: vec![
                    StructureSetEntry {
                        structure: identifier("minecraft:village_plains"),
                        weight: 1,
                    },
                    StructureSetEntry {
                        structure: identifier("minecraft:village_desert"),
                        weight: 3,
                    },
                ],
            }
        );
    }

    /// Placement fields vanilla defaults stay defaulted, and fields vanilla
    /// requires stay required.
    #[test]
    fn structure_set_optional_placement_fields_default() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/structure_set/villages.json",
            r#"{
                "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 10387312 },
                "structures": [{ "structure": "minecraft:village_plains", "weight": 1 }]
            }"#,
        );
        let set = loader(dir.path())
            .load_structure_set(&identifier("minecraft:villages"))
            .unwrap();
        let PlacementSpec::RandomSpread(placement) = set.placement;
        assert_eq!(placement.spread_type, RandomSpreadType::Linear);
        assert_eq!(
            placement.frequency_reduction_method,
            FrequencyReductionMethod::Default
        );
        assert_eq!(placement.frequency, 1.0);
        assert_eq!(placement.locate_offset, [0, 0, 0]);
        assert_eq!(placement.exclusion_zone, None);

        // `placement` is required by `StructureSet.DIRECT_CODEC`.
        write(
            dir.path(),
            "data/minecraft/worldgen/structure_set/villages.json",
            r#"{ "structures": [] }"#,
        );
        let error = loader(dir.path())
            .load_structure_set(&identifier("minecraft:villages"))
            .expect_err("placement is required");
        assert!(
            error.to_string().contains("is missing field placement"),
            "{error}"
        );
        assert!(
            error
                .to_string()
                .contains("structure set minecraft:villages"),
            "{error}"
        );
    }

    /// The jigsaw settings a village declares, including spawn overrides.
    #[test]
    fn structure_round_trips() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/structure/village_plains.json",
            r##"{
                "type": "minecraft:jigsaw",
                "biomes": ["minecraft:plains", "minecraft:meadow"],
                "max_distance_from_center": { "horizontal": 80, "vertical": 40 },
                "project_start_to_heightmap": "WORLD_SURFACE_WG",
                "size": 6,
                "spawn_overrides": {
                    "monster": {
                        "bounding_box": "piece",
                        "spawns": [
                            {
                                "data": { "type": "minecraft:zombie", "minCount": 1, "maxCount": 2 },
                                "weight": 1
                            }
                        ]
                    }
                },
                "start_height": { "type": "minecraft:uniform", "min_inclusive": { "absolute": 0 }, "max_inclusive": { "below_top": 10 } },
                "start_pool": "minecraft:village/plains/town_centers",
                "start_jigsaw_name": "minecraft:bottom",
                "step": "surface_structures",
                "terrain_adaptation": "beard_thin",
                "use_expansion_hack": true,
                "dimension_padding": 2,
                "liquid_settings": "ignore_waterlogging"
            }"##,
        );
        let spec = loader(dir.path())
            .load_structure(&identifier("minecraft:village_plains"))
            .unwrap();
        assert_eq!(
            spec,
            StructureSpec {
                id: identifier("minecraft:village_plains"),
                structure_type: identifier("minecraft:jigsaw"),
                biomes: BiomeSet::Biomes(vec![
                    identifier("minecraft:plains"),
                    identifier("minecraft:meadow"),
                ]),
                spawn_overrides: vec![SpawnOverrideEntry {
                    category: MobCategory::Monster,
                    bounding_box: SpawnBoundingBox::Piece,
                    spawns: vec![WeightedSpawn {
                        weight: 1,
                        entity_type: identifier("minecraft:zombie"),
                        min_count: 1,
                        max_count: 2,
                    }],
                }],
                step: StructureStep::SurfaceStructures,
                terrain_adaptation: TerrainAdaptation::BeardThin,
                start_pool: identifier("minecraft:village/plains/town_centers"),
                start_jigsaw_name: Some(identifier("minecraft:bottom")),
                size: 6,
                start_height: HeightProviderSpec::Uniform {
                    min_inclusive: VerticalAnchor::Absolute(0),
                    max_inclusive: VerticalAnchor::BelowTop(10),
                },
                use_expansion_hack: true,
                project_start_to_heightmap: Some(HeightmapType::WorldSurfaceWg),
                max_distance_from_center: MaxDistance {
                    horizontal: 80,
                    vertical: 40,
                },
                dimension_padding: DimensionPadding { bottom: 2, top: 2 },
                liquid_settings: LiquidSettings::IgnoreWaterlogging,
            }
        );
    }

    /// The fields the codec defaults, defaulted on the structure too.
    #[test]
    fn structure_optional_fields_default() {
        let dir = fixture();
        let spec = loader(dir.path())
            .load_structure(&identifier("minecraft:village_plains"))
            .unwrap();
        assert_eq!(
            spec,
            StructureSpec {
                id: identifier("minecraft:village_plains"),
                structure_type: identifier("minecraft:jigsaw"),
                biomes: BiomeSet::Tag(identifier("minecraft:has_structure/village_plains")),
                spawn_overrides: Vec::new(),
                step: StructureStep::SurfaceStructures,
                terrain_adaptation: TerrainAdaptation::BeardThin,
                start_pool: identifier("minecraft:village/plains/town_centers"),
                start_jigsaw_name: None,
                size: 6,
                start_height: HeightProviderSpec::Constant(VerticalAnchor::Absolute(0)),
                use_expansion_hack: true,
                project_start_to_heightmap: Some(HeightmapType::WorldSurfaceWg),
                max_distance_from_center: MaxDistance {
                    horizontal: 80,
                    vertical: 80,
                },
                dimension_padding: DimensionPadding { bottom: 0, top: 0 },
                liquid_settings: LiquidSettings::ApplyWaterlogging,
            }
        );
    }

    /// Every element type the codec has, in one pool: the list form carrying a
    /// feature element, an empty element, and both single element types with
    /// the reference and inline processor forms.
    #[test]
    fn template_pool_round_trips_every_element_type() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
            r##"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": "minecraft:zombie_plains",
                            "projection": "rigid"
                        },
                        "weight": 1
                    },
                    {
                        "element": {
                            "element_type": "minecraft:single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_2",
                            "processors": [
                                {
                                    "processor_type": "minecraft:protected_blocks",
                                    "value": "#minecraft:features_cannot_replace"
                                }
                            ],
                            "projection": "terrain_matching",
                            "override_liquid_settings": "ignore_waterlogging"
                        },
                        "weight": 2
                    },
                    {
                        "element": {
                            "element_type": "minecraft:list_pool_element",
                            "projection": "rigid",
                            "elements": [
                                {
                                    "element_type": "minecraft:feature_pool_element",
                                    "feature": "minecraft:pile_hay",
                                    "projection": "rigid"
                                },
                                { "element_type": "minecraft:empty_pool_element" }
                            ]
                        },
                        "weight": 3
                    }
                ]
            }"##,
        );
        let spec = loader(dir.path())
            .load_template_pool(&identifier("minecraft:village/plains/town_centers"))
            .unwrap();
        assert_eq!(
            spec,
            TemplatePoolSpec {
                id: identifier("minecraft:village/plains/town_centers"),
                fallback: identifier("minecraft:empty"),
                elements: vec![
                    PoolElementEntry {
                        weight: 1,
                        element: PoolElementSpec::LegacySingle(SingleElementSpec {
                            location: identifier(
                                "minecraft:village/plains/houses/plains_small_house_1"
                            ),
                            projection: Projection::Rigid,
                            processors: ProcessorRef::List(identifier("minecraft:zombie_plains")),
                            override_liquid_settings: None,
                        }),
                    },
                    PoolElementEntry {
                        weight: 2,
                        element: PoolElementSpec::Single(SingleElementSpec {
                            location: identifier(
                                "minecraft:village/plains/houses/plains_small_house_2"
                            ),
                            projection: Projection::TerrainMatching,
                            processors: ProcessorRef::Inline(vec![
                                StructureProcessorSpec::ProtectedBlocks {
                                    cannot_replace: identifier("minecraft:features_cannot_replace"),
                                }
                            ]),
                            override_liquid_settings: Some(LiquidSettings::IgnoreWaterlogging),
                        }),
                    },
                    PoolElementEntry {
                        weight: 3,
                        element: PoolElementSpec::List {
                            projection: Projection::Rigid,
                            elements: vec![
                                PoolElementSpec::Feature {
                                    feature: identifier("minecraft:pile_hay"),
                                    projection: Projection::Rigid,
                                },
                                PoolElementSpec::Empty,
                            ],
                        },
                    },
                ],
            }
        );
    }

    /// Every processor type this layer implements, with their fields: a rule
    /// with all three predicates, `block_age`, `gravity`,
    /// `jigsaw_replacement` and `protected_blocks`.
    #[test]
    fn processor_list_round_trips_every_supported_processor() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/processor_list/zombie_plains.json",
            r##"{
                "processors": [
                    {
                        "processor_type": "minecraft:rule",
                        "rules": [
                            {
                                "input_predicate": {
                                    "predicate_type": "minecraft:random_block_match",
                                    "block": "minecraft:cobblestone",
                                    "probability": 0.1
                                },
                                "location_predicate": {
                                    "predicate_type": "minecraft:tag_match",
                                    "tag": "minecraft:features_cannot_replace"
                                },
                                "position_predicate": {
                                    "predicate_type": "minecraft:axis_aligned_linear_pos",
                                    "min_chance": 0.0,
                                    "max_chance": 0.5,
                                    "min_dist": 1,
                                    "max_dist": 4,
                                    "axis": "y"
                                },
                                "output_state": {
                                    "Name": "minecraft:mossy_cobblestone",
                                    "Properties": { "snowy": "false" }
                                },
                                "block_entity_modifier": { "type": "minecraft:passthrough" }
                            },
                            {
                                "input_predicate": {
                                    "predicate_type": "minecraft:blockstate_match",
                                    "block_state": { "Name": "minecraft:dirt" }
                                },
                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                "output_state": { "Name": "minecraft:grass_block" }
                            }
                        ]
                    },
                    { "processor_type": "minecraft:block_age", "mossiness": 0.5 },
                    { "processor_type": "minecraft:gravity", "heightmap": "WORLD_SURFACE_WG", "offset": -1 },
                    { "processor_type": "minecraft:jigsaw_replacement" },
                    { "processor_type": "minecraft:protected_blocks", "value": "#minecraft:features_cannot_replace" }
                ]
            }"##,
        );
        let spec = loader(dir.path())
            .load_processor_list(&identifier("minecraft:zombie_plains"))
            .unwrap();
        assert_eq!(
            spec,
            ProcessorListSpec {
                id: identifier("minecraft:zombie_plains"),
                processors: vec![
                    StructureProcessorSpec::Rule {
                        rules: vec![
                            ProcessorRuleSpec {
                                input_predicate: RuleTestSpec::RandomBlockMatch {
                                    block: identifier("minecraft:cobblestone"),
                                    probability: 0.1,
                                },
                                location_predicate: RuleTestSpec::TagMatch {
                                    tag: identifier("minecraft:features_cannot_replace"),
                                },
                                position_predicate: PosRuleTestSpec::AxisAlignedLinearPos {
                                    min_chance: 0.0,
                                    max_chance: 0.5,
                                    min_dist: 1,
                                    max_dist: 4,
                                    axis: Axis::Y,
                                },
                                output_state: BlockStateSpec {
                                    block: identifier("minecraft:mossy_cobblestone"),
                                    properties: vec![("snowy".to_owned(), "false".to_owned())],
                                },
                            },
                            ProcessorRuleSpec {
                                input_predicate: RuleTestSpec::BlockStateMatch {
                                    state: BlockStateSpec {
                                        block: identifier("minecraft:dirt"),
                                        properties: Vec::new(),
                                    },
                                },
                                location_predicate: RuleTestSpec::AlwaysTrue,
                                position_predicate: PosRuleTestSpec::AlwaysTrue,
                                output_state: BlockStateSpec {
                                    block: identifier("minecraft:grass_block"),
                                    properties: Vec::new(),
                                },
                            },
                        ],
                    },
                    StructureProcessorSpec::BlockAge { mossiness: 0.5 },
                    StructureProcessorSpec::Gravity {
                        heightmap: HeightmapType::WorldSurfaceWg,
                        offset: -1,
                    },
                    StructureProcessorSpec::JigsawReplacement,
                    StructureProcessorSpec::ProtectedBlocks {
                        cannot_replace: identifier("minecraft:features_cannot_replace"),
                    },
                ],
            }
        );
    }

    /// An empty processor list is legal, and so is the bare-array file form.
    #[test]
    fn processor_list_accepts_the_empty_and_bare_array_forms() {
        let dir = tempfile::TempDir::new().unwrap();
        write(
            dir.path(),
            "data/minecraft/worldgen/processor_list/empty.json",
            r#"{ "processors": [] }"#,
        );
        write(
            dir.path(),
            "data/minecraft/worldgen/processor_list/bare.json",
            r#"[{ "processor_type": "minecraft:jigsaw_replacement" }]"#,
        );
        let empty = loader(dir.path())
            .load_processor_list(&identifier("minecraft:empty"))
            .unwrap();
        assert_eq!(empty.processors, Vec::new());
        let bare = loader(dir.path())
            .load_processor_list(&identifier("minecraft:bare"))
            .unwrap();
        assert_eq!(
            bare.processors,
            vec![StructureProcessorSpec::JigsawReplacement]
        );
    }

    /// Every unsupported type names both the type id and the entry that
    /// referenced it.
    #[test]
    fn unsupported_types_fail_closed() {
        let cases: &[(&str, &str, &str)] = &[
            // (fixture body, the file it replaces, the referring entry)
            (
                r#"{
                    "placement": { "type": "minecraft:concentric_rings", "spacing": 34, "separation": 8, "salt": 1 },
                    "structures": []
                }"#,
                "structure_set/villages.json",
                "minecraft:villages",
            ),
            (
                r##"{
                    "type": "minecraft:nether_fossil",
                    "biomes": "#minecraft:is_nether",
                    "spawn_overrides": {},
                    "step": "surface_structures"
                }"##,
                "structure/village_plains.json",
                "minecraft:village_plains",
            ),
            (
                r##"{
                    "type": "minecraft:jigsaw",
                    "biomes": "#minecraft:has_structure/village_plains",
                    "max_distance_from_center": 80,
                    "size": 6,
                    "spawn_overrides": {},
                    "start_height": { "type": "minecraft:weighted_list", "distribution": [] },
                    "start_pool": "minecraft:village/plains/town_centers",
                    "step": "surface_structures",
                    "use_expansion_hack": true
                }"##,
                "structure/village_plains.json",
                "minecraft:village_plains",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": { "element_type": "minecraft:absent_pool_element" },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "processors": [
                                    { "processor_type": "minecraft:capped", "delegate": {} }
                                ],
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "processors": [
                                    {
                                        "processor_type": "minecraft:rule",
                                        "rules": [
                                            {
                                                "input_predicate": { "predicate_type": "minecraft:absent_test" },
                                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                                "output_state": { "Name": "minecraft:air" }
                                            }
                                        ]
                                    }
                                ],
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "processors": [
                                    {
                                        "processor_type": "minecraft:rule",
                                        "rules": [
                                            {
                                                "input_predicate": { "predicate_type": "minecraft:always_true" },
                                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                                "position_predicate": { "predicate_type": "minecraft:absent_pos_test" },
                                                "output_state": { "Name": "minecraft:air" }
                                            }
                                        ]
                                    }
                                ],
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "processors": [
                                    {
                                        "processor_type": "minecraft:rule",
                                        "rules": [
                                            {
                                                "input_predicate": { "predicate_type": "minecraft:always_true" },
                                                "location_predicate": { "predicate_type": "minecraft:always_true" },
                                                "block_entity_modifier": { "type": "minecraft:append_loot", "loot_table": "minecraft:empty" },
                                                "output_state": { "Name": "minecraft:air" }
                                            }
                                        ]
                                    }
                                ],
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "processors": [
                                    { "processor_type": "minecraft:protected_blocks", "value": "minecraft:features_cannot_replace" }
                                ],
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "minecraft:village/plains/town_centers",
            ),
        ];
        let expected_type = [
            "minecraft:concentric_rings",
            "minecraft:nether_fossil",
            "minecraft:weighted_list",
            "minecraft:absent_pool_element",
            "minecraft:capped",
            "minecraft:absent_test",
            "minecraft:absent_pos_test",
            "minecraft:append_loot",
        ];
        for ((body, relative, referrer), type_id) in cases.iter().zip(expected_type) {
            let dir = fixture();
            write(
                dir.path(),
                &format!("data/minecraft/worldgen/{relative}"),
                body,
            );
            let error = load_all(dir.path()).expect_err("unsupported data fails closed");
            let message = error.to_string();
            assert!(
                message.contains(type_id),
                "{type_id} missing from {message}"
            );
            assert!(
                message.contains(referrer),
                "{referrer} missing from {message}"
            );
        }
        // The `protected_blocks` case rejects the unhashed tag name: the
        // `Type` outcome rather than `Value`.
        let dir = fixture();
        write(
            dir.path(),
            "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
            cases[8].0,
        );
        let error = load_all(dir.path()).expect_err("an unhashed protected block tag fails");
        assert!(
            error
                .to_string()
                .contains("has invalid value for value: minecraft:features_cannot_replace"),
            "{error}"
        );
    }

    /// Required fields are required, and the error names the entry and field.
    #[test]
    fn missing_required_fields_fail_closed() {
        let cases: &[(&str, &str, &str)] = &[
            (
                r##"{
                    "type": "minecraft:jigsaw",
                    "biomes": "#minecraft:has_structure/village_plains",
                    "max_distance_from_center": 80,
                    "size": 6,
                    "start_height": { "absolute": 0 },
                    "start_pool": "minecraft:village/plains/town_centers",
                    "step": "surface_structures",
                    "use_expansion_hack": true
                }"##,
                "structure/village_plains.json",
                "is missing field spawn_overrides",
            ),
            (
                r##"{
                    "type": "minecraft:jigsaw",
                    "biomes": "#minecraft:has_structure/village_plains",
                    "max_distance_from_center": 80,
                    "spawn_overrides": {},
                    "size": 6,
                    "start_height": { "absolute": 0 },
                    "start_pool": "minecraft:village/plains/town_centers",
                    "step": "surface_structures"
                }"##,
                "structure/village_plains.json",
                "is missing field use_expansion_hack",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "is missing field projection",
            ),
            (
                r#"{
                    "fallback": "minecraft:empty",
                    "elements": [
                        {
                            "element": {
                                "element_type": "minecraft:legacy_single_pool_element",
                                "location": "minecraft:village/plains/houses/plains_small_house_1",
                                "projection": "rigid"
                            },
                            "weight": 1
                        }
                    ]
                }"#,
                "template_pool/village/plains/town_centers.json",
                "is missing field processors",
            ),
            (
                r#"{
                    "processors": [
                        {
                            "processor_type": "minecraft:rule",
                            "rules": [
                                {
                                    "location_predicate": { "predicate_type": "minecraft:always_true" },
                                    "output_state": { "Name": "minecraft:air" }
                                }
                            ]
                        }
                    ]
                }"#,
                "processor_list/zombie_plains.json",
                "is missing field input_predicate",
            ),
            (
                r#"{
                    "processors": [{ "processor_type": "minecraft:block_age" }]
                }"#,
                "processor_list/zombie_plains.json",
                "is missing field mossiness",
            ),
        ];
        let entries = [
            "minecraft:village_plains",
            "minecraft:village_plains",
            "minecraft:village/plains/town_centers",
            "minecraft:village/plains/town_centers",
            "minecraft:zombie_plains",
            "minecraft:zombie_plains",
        ];
        for ((body, relative, expected), entry) in cases.iter().zip(entries) {
            let dir = fixture();
            write(
                dir.path(),
                &format!("data/minecraft/worldgen/{relative}"),
                body,
            );
            let error = load_all(dir.path()).expect_err("missing field fails closed");
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "{expected} missing from {message}"
            );
            assert!(message.contains(entry), "{entry} missing from {message}");
        }
    }

    /// A structure outside the vanilla namespace never resolves: the cache
    /// only holds `data/minecraft`.
    #[test]
    fn non_vanilla_namespace_fails_closed() {
        let dir = fixture();
        let error = loader(dir.path())
            .load_structure_set(&identifier("solaris:villages"))
            .expect_err("a non-vanilla structure set fails closed");
        assert!(
            error
                .to_string()
                .contains("non-minecraft structure set solaris:villages"),
            "{error}"
        );
    }

    /// A missing entry names the id, the entry that referenced it and the path
    /// that was searched. Inside one load the referrer is the entry that
    /// carries the reference; a top-level load is its own referrer.
    #[test]
    fn missing_entry_names_referrer() {
        let dir = fixture();
        let error = loader(dir.path())
            .load_template_pool(&identifier("minecraft:village/plains/absent"))
            .expect_err("the absent pool fails");
        let message = error.to_string();
        assert!(
            message.contains("missing template pool minecraft:village/plains/absent"),
            "{message}"
        );
        assert!(message.contains("village/plains/absent.json"), "{message}");

        // A pool's element carries an inline processor list, so an unsupported
        // rule test there is reported against the pool that carries it.
        write(
            dir.path(),
            "data/minecraft/worldgen/template_pool/village/plains/town_centers.json",
            r#"{
                "fallback": "minecraft:empty",
                "elements": [
                    {
                        "element": {
                            "element_type": "minecraft:legacy_single_pool_element",
                            "location": "minecraft:village/plains/houses/plains_small_house_1",
                            "processors": [
                                {
                                    "processor_type": "minecraft:rule",
                                    "rules": [
                                        {
                                            "input_predicate": { "predicate_type": "minecraft:absent_test" },
                                            "location_predicate": { "predicate_type": "minecraft:always_true" },
                                            "output_state": { "Name": "minecraft:air" }
                                        }
                                    ]
                                }
                            ],
                            "projection": "rigid"
                        },
                        "weight": 1
                    }
                ]
            }"#,
        );
        let error = load_all(dir.path()).expect_err("the absent rule test fails");
        let message = error.to_string();
        assert!(message.contains("minecraft:absent_test"), "{message}");
        assert!(
            message.contains("referenced by minecraft:village/plains/town_centers"),
            "{message}"
        );
    }

    /// Loads the fixture closure the way an engine would: set, then each
    /// structure, its start pool and fallback chain, and every processor list
    /// those elements name.
    fn load_all(root: &Path) -> Result<(), VillageDataError> {
        let loader = loader(root);
        let set = loader.load_structure_set(&identifier("minecraft:villages"))?;
        let mut pools: BTreeSet<Identifier> = BTreeSet::new();
        for entry in &set.structures {
            let structure = loader.load_structure(&entry.structure)?;
            let mut next = Some(structure.start_pool.clone());
            while let Some(pool_id) = next.take() {
                let pool = loader.load_template_pool(&pool_id)?;
                for element in &pool.elements {
                    let single = match &element.element {
                        PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
                            single
                        }
                        PoolElementSpec::List { .. }
                        | PoolElementSpec::Feature { .. }
                        | PoolElementSpec::Empty => continue,
                    };
                    if let ProcessorRef::List(list) = &single.processors {
                        loader.load_processor_list(list)?;
                    }
                }
                if pool.fallback != pool_id {
                    next = Some(pool.fallback.clone());
                }
                pools.insert(pool_id);
            }
        }
        assert!(!pools.is_empty());
        Ok(())
    }

    /// The local vanilla content cache, when it is present. An explicit
    /// `SOLARIS_CONTENT_CACHE` pins the search to that root — an operator who
    /// names a cache does not want another one silently standing in for it —
    /// otherwise the workspace cache and the documented scratch cache are tried.
    fn real_cache_worldgen() -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("SOLARIS_CONTENT_CACHE") {
            return find_village_cache([PathBuf::from(dir)]);
        }
        let mut candidates = Vec::new();
        if let Some(workspace) = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
        {
            candidates.push(workspace.join("data/vanilla"));
        }
        candidates.push(PathBuf::from("/tmp/jdk-cold2"));
        find_village_cache(candidates)
    }

    /// The first candidate that actually holds the villages structure set.
    fn find_village_cache(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
        candidates
            .into_iter()
            .map(|root| root.join("data/minecraft/worldgen"))
            .find(|worldgen| worldgen.join("structure_set/villages.json").is_file())
    }

    /// What drives the loud skip: a candidate only counts when it holds the
    /// villages set, and an absent cache is reported as absent.
    #[test]
    fn cache_detection_distinguishes_present_from_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(find_village_cache([dir.path().to_path_buf()]), None);
        write(
            dir.path(),
            "data/minecraft/worldgen/structure_set/villages.json",
            r#"{ "placement": { "type": "minecraft:random_spread", "spacing": 34, "separation": 8, "salt": 1 }, "structures": [] }"#,
        );
        assert_eq!(
            find_village_cache([dir.path().to_path_buf()]),
            Some(dir.path().join("data/minecraft/worldgen"))
        );
    }

    /// The real-data proof: the villages set, all five village structures with
    /// their biome tags and start pools, and the processor lists those pools
    /// name. Skips loudly when no cache is present.
    #[test]
    fn real_cache_villages_closure_loads() {
        let Some(worldgen) = real_cache_worldgen() else {
            eprintln!(
                "SKIP real_cache_villages_closure_loads: no vanilla content cache with \
                 data/minecraft/worldgen/structure_set/villages.json (set SOLARIS_CONTENT_CACHE, \
                 or place one at /tmp/jdk-cold2)"
            );
            return;
        };
        println!("real cache: {}", worldgen.display());
        let loader = VillageDataLoader::new(&worldgen);

        let set = loader
            .load_structure_set(&identifier("minecraft:villages"))
            .expect("the villages structure set resolves");
        let PlacementSpec::RandomSpread(placement) = set.placement;
        assert_eq!((placement.spacing, placement.separation), (34, 8));
        assert_eq!(placement.salt, 10387312);
        assert_eq!(placement.spread_type, RandomSpreadType::Linear);
        assert_eq!(placement.frequency, 1.0);
        assert_eq!(
            placement.frequency_reduction_method,
            FrequencyReductionMethod::Default
        );
        assert_eq!(placement.locate_offset, [0, 0, 0]);
        assert_eq!(placement.exclusion_zone, None);
        println!(
            "structure set {} placement=random_spread spacing={} separation={} salt={} spread={:?}",
            set.id, placement.spacing, placement.separation, placement.salt, placement.spread_type
        );

        let ids: Vec<Identifier> = set
            .structures
            .iter()
            .map(|entry| entry.structure.clone())
            .collect();
        assert_eq!(ids.len(), 5);
        println!("structure set {} names {} structures", set.id, ids.len());

        let mut pools = BTreeSet::new();
        let mut processor_lists = BTreeSet::new();
        let mut elements = 0;
        for entry in &set.structures {
            let structure = loader
                .load_structure(&entry.structure)
                .expect("every village structure resolves");
            assert_eq!(entry.weight, 1);
            assert_eq!(structure.structure_type, identifier("minecraft:jigsaw"));
            assert_eq!(structure.size, 6);
            assert_eq!(
                structure.start_height,
                HeightProviderSpec::Constant(VerticalAnchor::Absolute(0))
            );
            assert_eq!(
                structure.project_start_to_heightmap,
                Some(HeightmapType::WorldSurfaceWg)
            );
            assert_eq!(structure.terrain_adaptation, TerrainAdaptation::BeardThin);
            assert_eq!(
                structure.max_distance_from_center,
                MaxDistance {
                    horizontal: 80,
                    vertical: 80
                }
            );
            assert!(structure.use_expansion_hack);
            assert_eq!(structure.step, StructureStep::SurfaceStructures);
            assert!(structure.spawn_overrides.is_empty());
            assert_eq!(structure.start_jigsaw_name, None);
            let BiomeSet::Tag(tag) = &structure.biomes else {
                panic!("village {} uses a biome tag", structure.id);
            };
            assert_eq!(
                tag.to_string(),
                format!("minecraft:has_structure/{}", structure.id.path())
            );
            assert!(
                structure.start_pool.path().ends_with("/town_centers"),
                "{}",
                structure.start_pool
            );
            println!(
                "structure {} biome_tag={} start_pool={} size={} start_height={:?} terrain={:?} max_distance={:?}",
                structure.id,
                tag,
                structure.start_pool,
                structure.size,
                structure.start_height,
                structure.terrain_adaptation,
                structure.max_distance_from_center
            );

            let mut next = Some(structure.start_pool.clone());
            while let Some(pool_id) = next.take() {
                if !pools.insert(pool_id.clone()) {
                    continue;
                }
                let pool = loader
                    .load_template_pool(&pool_id)
                    .expect("every referenced pool resolves");
                println!(
                    "pool {} elements={} fallback={}",
                    pool.id,
                    pool.elements.len(),
                    pool.fallback
                );
                elements += pool.elements.len();
                for element in &pool.elements {
                    let single = match &element.element {
                        PoolElementSpec::Single(single) | PoolElementSpec::LegacySingle(single) => {
                            single
                        }
                        PoolElementSpec::Feature { .. } | PoolElementSpec::Empty => continue,
                        PoolElementSpec::List { .. } => {
                            panic!("a village start pool holds no list element")
                        }
                    };
                    match &single.processors {
                        ProcessorRef::List(list) => {
                            processor_lists.insert(list.clone());
                        }
                        ProcessorRef::Inline(processors) => assert!(
                            processors.is_empty(),
                            "village start pool elements carry empty inline lists"
                        ),
                    }
                }
                if pool.fallback != pool_id {
                    next = Some(pool.fallback.clone());
                }
            }
        }
        println!(
            "village start-pool closure: pools={} elements={} processor_lists={}",
            pools.len(),
            elements,
            processor_lists.len()
        );
        assert_eq!(pools.len(), 6, "five start pools plus minecraft:empty");
        assert_eq!(elements, 32);
        let expected_lists: Vec<String> = processor_lists.iter().map(ToString::to_string).collect();
        assert_eq!(
            expected_lists,
            vec![
                "minecraft:mossify_10_percent",
                "minecraft:mossify_20_percent",
                "minecraft:mossify_70_percent",
                "minecraft:zombie_desert",
                "minecraft:zombie_plains",
                "minecraft:zombie_savanna",
                "minecraft:zombie_taiga",
            ]
        );
        let mut rules = 0;
        for list_id in &processor_lists {
            let list = loader
                .load_processor_list(list_id)
                .expect("every referenced processor list resolves");
            let list_rules: usize = list
                .processors
                .iter()
                .map(|processor| match processor {
                    StructureProcessorSpec::Rule { rules } => rules.len(),
                    other => panic!("{list_id} holds a non-rule processor {other:?}"),
                })
                .sum();
            rules += list_rules;
            println!(
                "processor_list {} processors={} rules={}",
                list.id,
                list.processors.len(),
                list_rules
            );
        }
        assert_eq!(rules, 56);
        println!("village start-pool closure: rule processors={rules}");
    }
}
