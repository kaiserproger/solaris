//! Data-driven vanilla placed-feature executor (checkpoint A1, partial).
//!
//! ## What this is
//!
//! A dispatch-by-type-id executor over the vanilla worldgen closure that the
//! village `feature_pool_element` entries reach. `mc-data`'s
//! [`mc_data::vanilla_feature_closure`] resolves that closure by reference into
//! typed specs and fails closed on anything unsupported; this module compiles
//! those specs against the block registry and places them. The single data →
//! execution entry point is [`compile_pool_features`], which resolves a
//! structure pool's feature elements by reference and compiles each one. It
//! covers:
//!
//! - a [`RandomSource`] with vanilla's `LegacyRandomSource` (48-bit LCG) bit
//!   layout,
//! - vanilla `NormalNoise` (Perlin octaves over the legacy positional factory)
//!   for `noise_threshold_provider`,
//! - the four reachable feature types `minecraft:simple_block`,
//!   `minecraft:block_pile`, `minecraft:block_column`, `minecraft:tree` (the
//!   four trunk/foliage placer pairs the village closure reaches, with the
//!   leaf-`distance` pass),
//! - the five reachable state providers `simple`, `weighted`, `rotated`,
//!   `noise_threshold`, `rule_based`,
//! - the three reachable placement modifiers `count`, `random_offset`,
//!   `block_predicate_filter`, applied in list order with vanilla's lazy
//!   per-position interleaving,
//! - the block predicates the closure reaches: `would_survive`,
//!   `matching_block_tag`, `matching_blocks`, `all_of`, `not`.
//!
//! ## Partial-layer contract
//!
//! This is **checkpoints A1+A2 of a partial layer**. It is library code
//! exercised by tests: nothing in the live `settlement_profile = "vanilla"` path
//! calls it, no config flag or entry point activates it, no worldgen identity
//! changed, and `PlainsVillagePrototype` still owns village generation. The
//! whole reachable village decor closure — all 19 feature elements, 13 distinct
//! features — now loads and places; what remains is village assembly
//! (checkpoint B), which will drive [`CompiledPlacedFeature`] over the real
//! generation world.
//!
//! Everything outside the surface above fails closed by name at load or compile
//! time rather than being approximated:
//!
//! - other configured feature types, placement modifiers, state providers,
//!   trunk/foliage placers, feature sizes, tree decorators and root placers,
//! - `schedule_tick` on `simple_block` (the layer has no tick scheduler),
//! - states that are double plants (`minecraft:tall_grass`,
//!   `minecraft:large_fern`, `minecraft:pitcher_plant`, and the tall flowers) or
//!   `minecraft:pale_moss_carpet`, whose placement branches are not implemented,
//! - a foliage provider that can place a state without a `distance` property,
//!   which vanilla's leaf-distance pass would throw on.
//!
//! `TreeFeature.place`'s final `StructureTemplate.updateShapeAtEdge` pass is not
//! reproduced: it runs `BlockState.updateShape` on the tree's own blocks, and
//! every block the reachable trees write (logs, leaves, dirt) returns its state
//! unchanged there, so the pass writes nothing. A decoration or block whose
//! shape depends on its neighbours would need it.

mod execute;
mod placement;
mod provider;
mod random;
mod synth;
mod tree;

#[cfg(test)]
pub(crate) use tree::{JavaHashSet, java_hash};

use mc_data::Identifier;
use mc_data::block_facts::{SturdyFace, has_full_sturdy_face};
use mc_data::collision_shapes::{COLLISION_UNITS_PER_BLOCK, vanilla_collision_shapes};
use mc_data::vanilla_feature_closure::BlockStateSpec;
use mc_world::{BlockPos, BlockRegistry, BlockStateId};
use std::sync::Arc;
use thiserror::Error;

pub use execute::CompiledPlacedFeature;
pub use placement::CompiledPredicate;
pub use provider::CompiledStateProvider;
pub use random::{
    LegacyPositionalRandomFactory, LegacyRandom, PositionalRandomSource, RandomSource,
    WorldgenRandom, XoroshiroRandom, java_string_hash,
};
pub use synth::NormalNoise;

/// Blocks vanilla registers as `AirBlock` with the air property
/// (`Blocks.AIR`, `Blocks.CAVE_AIR`, `Blocks.VOID_AIR`).
const AIR_BLOCKS: &[&str] = &["air", "cave_air", "void_air"];

/// Blocks vanilla registers with `BlockBehaviour.Properties.liquid()`
/// (`Blocks.WATER`, `Blocks.LAVA`, `Blocks.BUBBLE_COLUMN`).
const LIQUID_BLOCKS: &[&str] = &["water", "lava", "bubble_column"];

/// Vanilla `VegetationBlock` subclasses that inherit
/// `VegetationBlock.canSurvive` unchanged, i.e. "the block below is in
/// `minecraft:supports_vegetation`". Taken from the 26.1.2 block classes
/// (`Blocks` registrations plus the `extends` chain): subclasses that override
/// `canSurvive`/`mayPlaceOn`, and the `DoublePlantBlock` family, are excluded,
/// so the executor refuses only what it truly does not model.
const VEGETATION_BLOCKS: &[&str] = &[
    "acacia_sapling",
    "allium",
    "azure_bluet",
    "birch_sapling",
    "blue_orchid",
    "bush",
    "cherry_sapling",
    "closed_eyeblossom",
    "cornflower",
    "dandelion",
    "dark_oak_sapling",
    "fern",
    "firefly_bush",
    "golden_dandelion",
    "jungle_sapling",
    "lily_of_the_valley",
    "oak_sapling",
    "open_eyeblossom",
    "orange_tulip",
    "oxeye_daisy",
    "pale_oak_sapling",
    "pink_petals",
    "pink_tulip",
    "poppy",
    "red_tulip",
    "short_grass",
    "spruce_sapling",
    "sweet_berry_bush",
    "torchflower",
    "white_tulip",
    "wildflowers",
];

/// Vanilla `DoublePlantBlock` subclasses: two-block placements that
/// `minecraft:simple_block` places with `DoublePlantBlock.placeAt`.
const DOUBLE_PLANT_BLOCKS: &[&str] = &[
    "tall_grass",
    "large_fern",
    "pitcher_plant",
    "sunflower",
    "lilac",
    "rose_bush",
    "peony",
];

/// Vanilla `MossyCarpetBlock` subclasses.
const MOSSY_CARPET_BLOCKS: &[&str] = &["pale_moss_carpet"];

/// The world a placed feature writes into.
///
/// This is the subset of vanilla `WorldGenLevel` the A1 executor uses. Village
/// assembly (checkpoint B) supplies the adapter over the real generation world;
/// tests supply an in-memory level.
pub trait FeatureLevel {
    /// Vanilla `LevelReader.getMinY`.
    fn min_y(&self) -> i32;
    /// Vanilla `LevelReader.getMaxY`.
    fn max_y(&self) -> i32;
    /// Vanilla `LevelReader.getBlockState`.
    fn block_state(&self, pos: BlockPos) -> BlockStateId;
    /// Vanilla `WorldGenLevel.setBlock`; update flags are irrelevant to worldgen
    /// writes and are not modelled.
    fn set_block(&mut self, pos: BlockPos, state: BlockStateId);
    /// Vanilla `LevelReader.isFluidAtPosition(pos, water source)`, used by
    /// `FoliagePlacer.tryPlaceLeaf` to set a waterlogged leaf state.
    fn is_water_source_at(&self, pos: BlockPos) -> bool;
}

/// Block-tag membership, as `TagKey<Block>` resolution needs it.
pub trait BlockTagIndex {
    fn block_in_tag(&self, tag: &Identifier, block: &Identifier) -> bool;
}

/// Worldgen biome-tag membership: whether `biome` is in the tag `tag`.
///
/// Vanilla structures gate on tags such as
/// `minecraft:has_structure/village_plains`, so the village plan lookup asks
/// about a tag and resolves the biome at the structure's start column. The tag
/// contents are data; the biome at a column is the terrain's own plan-free
/// answer, supplied at the call site.
pub trait BiomeTagIndex {
    fn biome_in_tag(&self, tag: &Identifier, biome: &Identifier) -> bool;
}

/// Tag membership over the resolved vanilla tag set the server already loaded.
///
/// Vanilla tags hold registry raw ids, so membership is a raw-id lookup: a block
/// carries its own, and a biome's is its index in the `worldgen/biome` registry
/// — the same index [`mc_data::tags`] resolved the tag entries against. One
/// instance answers both questions, so the tag set is loaded once and no second
/// tag authority exists.
pub struct CacheTags {
    blocks: Arc<BlockRegistry>,
    biomes: Arc<mc_data::VanillaData>,
    tags: Arc<mc_data::tags::TagsData>,
}

impl CacheTags {
    #[must_use]
    pub fn new(
        blocks: Arc<BlockRegistry>,
        biomes: Arc<mc_data::VanillaData>,
        tags: Arc<mc_data::tags::TagsData>,
    ) -> Self {
        Self {
            blocks,
            biomes,
            tags,
        }
    }

    /// The biome's raw id in the `worldgen/biome` registry, i.e. its index in
    /// the registry the tag set was resolved against.
    #[must_use]
    pub fn biome_raw_id(&self, biome: &Identifier) -> Option<i32> {
        let registry = self.biomes.registry("worldgen/biome")?;
        registry
            .entries
            .iter()
            .position(|entry| entry == biome)
            .and_then(|index| i32::try_from(index).ok())
    }
}

impl BlockTagIndex for CacheTags {
    fn block_in_tag(&self, tag: &Identifier, block: &Identifier) -> bool {
        self.blocks.block(block).is_some_and(|block| {
            self.tags
                .contains_raw_id("minecraft:block", tag.as_str(), block.raw_id)
        })
    }
}

impl BiomeTagIndex for CacheTags {
    fn biome_in_tag(&self, tag: &Identifier, biome: &Identifier) -> bool {
        self.biome_raw_id(biome).is_some_and(|raw_id| {
            self.tags
                .contains_raw_id("minecraft:worldgen/biome", tag.as_str(), raw_id)
        })
    }
}

/// Why a placement stopped short. Placement is fallible because a faithful
/// execution can reach a boundary this layer refuses to cross silently.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlaceError {
    #[error(
        "java.util.HashMap would treeify a bin ({bin_size} entries at capacity {capacity}) while \
         updating tree block {pos:?}; leaf distance ordering past the treeify boundary is not \
         modelled, so placement stops rather than diverging from vanilla"
    )]
    TreeOrderBoundary {
        bin_size: usize,
        capacity: usize,
        pos: BlockPos,
    },
}

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("block state {block} in {owner} does not resolve in the block registry")]
    UnknownBlockState {
        owner: Identifier,
        block: Identifier,
    },
    #[error("{owner} names a weighted state provider with no positive total weight")]
    EmptyWeightedList { owner: Identifier },
    #[error("{owner} names a noise threshold provider with an empty state list: {field}")]
    EmptyNoiseStates {
        owner: Identifier,
        field: &'static str,
    },
    #[error("{owner} sets schedule_tick, which this partial layer does not implement")]
    UnsupportedScheduleTick { owner: Identifier },
    #[error(
        "{owner} can place {block}, whose multi-block placement this partial layer does not implement"
    )]
    UnsupportedMultiBlockPlacement {
        owner: Identifier,
        block: Identifier,
    },
    #[error("{owner} needs {block}'s block behaviour, which this partial layer does not implement")]
    UnsupportedBlockBehaviour {
        owner: Identifier,
        block: Identifier,
    },
    #[error("{owner} places {block}, which lacks the {property} property vanilla's tree pass sets")]
    MissingStateProperty {
        owner: Identifier,
        block: Identifier,
        property: &'static str,
    },
}

/// The 26.1.2 block behaviours the A1 executor models, resolved through the
/// block registry and the embedded collision-shape table.
pub struct BlockSemantics<'a> {
    blocks: &'a BlockRegistry,
    tags: &'a dyn BlockTagIndex,
}

impl<'a> BlockSemantics<'a> {
    #[must_use]
    pub fn new(blocks: &'a BlockRegistry, tags: &'a dyn BlockTagIndex) -> Self {
        Self { blocks, tags }
    }

    #[must_use]
    pub fn blocks(&self) -> &BlockRegistry {
        self.blocks
    }

    /// Vanilla `BlockState.isAir()`.
    #[must_use]
    pub fn is_air(&self, state: BlockStateId) -> bool {
        self.block_path(state)
            .is_some_and(|path| AIR_BLOCKS.contains(&path))
    }

    /// Vanilla `BlockState.liquid()`.
    #[must_use]
    pub fn is_liquid(&self, state: BlockStateId) -> bool {
        self.block_path(state)
            .is_some_and(|path| LIQUID_BLOCKS.contains(&path))
    }

    /// Vanilla `BlockState.isSolid()`: `BlockStateBase.calculateSolid`, i.e. the
    /// collision shape is non-empty and is at least `0.729...` on average or a
    /// full block tall. The report does not carry `forceSolidOn`/`forceSolidOff`;
    /// no block the closure reaches sets either.
    #[must_use]
    pub fn is_solid(&self, state: BlockStateId) -> bool {
        let Some(shape) = self.collision_shape(state) else {
            return false;
        };
        if shape.is_empty() {
            return false;
        }
        let mut bounds = [i16::MAX, i16::MAX, i16::MAX, i16::MIN, i16::MIN, i16::MIN];
        for collision_box in shape.iter() {
            let [min_x, min_y, min_z, max_x, max_y, max_z] = collision_box.coordinates();
            for (index, value) in [min_x, min_y, min_z].into_iter().enumerate() {
                bounds[index] = bounds[index].min(value);
            }
            for (index, value) in [max_x, max_y, max_z].into_iter().enumerate() {
                bounds[index + 3] = bounds[index + 3].max(value);
            }
        }
        let units = f64::from(COLLISION_UNITS_PER_BLOCK);
        let sizes = [
            f64::from(bounds[3] - bounds[0]) / units,
            f64::from(bounds[4] - bounds[1]) / units,
            f64::from(bounds[5] - bounds[2]) / units,
        ];
        let average = (sizes[0] + sizes[1] + sizes[2]) / 3.0;
        average >= 0.729_166_666_666_666_6 || sizes[1] >= 1.0
    }

    /// Vanilla `BlockState.is(Block)`.
    #[must_use]
    pub fn is_block(&self, state: BlockStateId, block: &Identifier) -> bool {
        self.blocks
            .by_id(state)
            .is_some_and(|state| state.block.id == *block)
    }

    /// Vanilla `BlockState.is(TagKey<Block>)`.
    #[must_use]
    pub fn in_tag(&self, state: BlockStateId, tag: &Identifier) -> bool {
        self.blocks
            .by_id(state)
            .is_some_and(|state| self.tags.block_in_tag(tag, &state.block.id))
    }

    /// Vanilla `BlockState.isFaceSturdy(level, pos, face)`.
    #[must_use]
    pub fn is_face_sturdy(
        &self,
        level: &dyn FeatureLevel,
        pos: BlockPos,
        face: SturdyFace,
    ) -> bool {
        self.blocks
            .by_id(level.block_state(pos))
            .is_some_and(|state| {
                has_full_sturdy_face(state.id.0, &state.block.id, &state.properties, face)
            })
    }

    /// Vanilla `BlockState.canSurvive(level, pos)` for the block behaviours the
    /// A1 layer models: the `VegetationBlock` family (the block below is in
    /// `minecraft:supports_vegetation`) and `CactusBlock`.
    ///
    /// Callers reach this only for blocks [`Self::survival_is_implemented`]
    /// accepts; the compile step refuses any other state a provider can produce.
    #[must_use]
    pub fn can_survive(
        &self,
        level: &dyn FeatureLevel,
        state: BlockStateId,
        pos: BlockPos,
    ) -> bool {
        let Some(path) = self.block_path(state) else {
            return false;
        };
        if VEGETATION_BLOCKS.contains(&path) {
            let below = Self::below(pos);
            return self.in_tag(
                level.block_state(below),
                &identifier("minecraft:supports_vegetation"),
            );
        }
        if path == "cactus" {
            return self.cactus_can_survive(level, pos);
        }
        debug_assert!(false, "can_survive called for unmodelled block {path}");
        false
    }

    /// Whether `can_survive` models this block, i.e. whether a provider may
    /// produce it.
    #[must_use]
    pub fn survival_is_implemented(&self, block: &Identifier) -> bool {
        let path = block.path();
        VEGETATION_BLOCKS.contains(&path) || path == "cactus"
    }

    /// Resolve a block state written in JSON. Property values are matched
    /// exactly, as `BlockState.CODEC` does.
    pub fn resolve_state(
        &self,
        owner: &Identifier,
        spec: &BlockStateSpec,
    ) -> Result<BlockStateId, CompileError> {
        self.blocks
            .by_name_and_props(&spec.block, &spec.properties)
            .ok_or_else(|| CompileError::UnknownBlockState {
                owner: owner.clone(),
                block: spec.block.clone(),
            })
    }

    /// State ids of `block` for the three `RotatedPillarBlock.AXIS` values, when
    /// the block carries that property (`RotatedBlockProvider` sets the axis on
    /// the written block's default state).
    #[must_use]
    pub fn axis_states(&self, block: &Identifier) -> Option<[BlockStateId; 3]> {
        let schema = self.blocks.block(block)?;
        if !schema
            .properties
            .iter()
            .any(|(name, values)| name == "axis" && values.iter().any(|value| value == "x"))
        {
            return None;
        }
        let mut states = [BlockStateId(0); 3];
        for (index, axis) in ["x", "y", "z"].into_iter().enumerate() {
            states[index] = self
                .blocks
                .by_name_and_props(block, &[("axis".to_owned(), axis.to_owned())])?;
        }
        Some(states)
    }

    pub(crate) fn is_lava(&self, state: BlockStateId) -> bool {
        self.block_path(state) == Some("lava")
    }

    fn cactus_can_survive(&self, level: &dyn FeatureLevel, pos: BlockPos) -> bool {
        let horizontal = [
            BlockPos {
                x: pos.x + 1,
                ..pos
            },
            BlockPos {
                x: pos.x - 1,
                ..pos
            },
            BlockPos {
                z: pos.z + 1,
                ..pos
            },
            BlockPos {
                z: pos.z - 1,
                ..pos
            },
        ];
        for neighbor in horizontal {
            let neighbor_state = level.block_state(neighbor);
            if self.is_solid(neighbor_state) || self.is_lava(neighbor_state) {
                return false;
            }
        }
        let below_state = level.block_state(Self::below(pos));
        let supported = self.is_block(below_state, &identifier("minecraft:cactus"))
            || self.in_tag(below_state, &identifier("minecraft:supports_cactus"));
        supported && !self.is_liquid(level.block_state(Self::above(pos)))
    }

    /// Vanilla `BlockState.is(TagKey<Block>)` for the tags the tree feature
    /// reads.
    #[must_use]
    pub fn is_leaves(&self, state: BlockStateId) -> bool {
        self.in_tag(state, &identifier("minecraft:leaves"))
    }

    #[must_use]
    pub fn is_log(&self, state: BlockStateId) -> bool {
        self.in_tag(state, &identifier("minecraft:logs"))
    }

    /// `minecraft:replaceable_by_trees`, i.e. `TreeFeature.validTreePos`'s
    /// second disjunct.
    #[must_use]
    pub fn is_replaceable_by_trees(&self, state: BlockStateId) -> bool {
        self.in_tag(state, &identifier("minecraft:replaceable_by_trees"))
    }

    /// `TreeFeature.validTreePos`.
    #[must_use]
    pub fn is_valid_tree_pos(&self, state: BlockStateId) -> bool {
        self.is_air(state) || self.is_replaceable_by_trees(state)
    }

    /// Vanilla `BlockState.is(Blocks.VINE)`.
    #[must_use]
    pub fn is_vine(&self, state: BlockStateId) -> bool {
        self.is_block(state, &identifier("minecraft:vine"))
    }

    /// Property lookup on a resolved state, e.g. `persistent`/`distance`.
    #[must_use]
    pub fn property(&self, state: BlockStateId, name: &str) -> Option<&str> {
        let state = self.blocks.by_id(state)?;
        state
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// A copy of `state` with one property replaced, as `BlockState.setValue`
    /// does; `None` when the state does not carry the property.
    #[must_use]
    pub fn with_property(
        &self,
        state: BlockStateId,
        name: &str,
        value: &str,
    ) -> Option<BlockStateId> {
        let resolved = self.blocks.by_id(state)?;
        if !resolved.properties.iter().any(|(key, _)| key == name) {
            return None;
        }
        let properties: Vec<(String, String)> = resolved
            .properties
            .iter()
            .map(|(key, current)| {
                if key == name {
                    (key.clone(), value.to_owned())
                } else {
                    (key.clone(), current.clone())
                }
            })
            .collect();
        self.blocks
            .by_name_and_props(&resolved.block.id, &properties)
    }

    /// Vanilla `LeavesBlock.getOptionalDistanceAt`: states in
    /// `minecraft:prevents_nearby_leaf_decay` (the `#logs` tag) are distance 0,
    /// any other state with a `distance` property reports it, everything else
    /// has none.
    #[must_use]
    pub fn leaf_distance(&self, state: BlockStateId) -> Option<i32> {
        if self.in_tag(state, &identifier("minecraft:prevents_nearby_leaf_decay")) {
            return Some(0);
        }
        self.property(state, "distance")
            .and_then(|value| value.parse::<i32>().ok())
    }

    fn block_path(&self, state: BlockStateId) -> Option<&str> {
        self.blocks.by_id(state).map(|state| state.block.id.path())
    }

    fn collision_shape(
        &self,
        state: BlockStateId,
    ) -> Option<mc_data::collision_shapes::CollisionShape<'_>> {
        let state = self.blocks.by_id(state)?;
        vanilla_collision_shapes().get_for_state(state.id.0, &state.block.id, &state.properties)
    }

    fn below(pos: BlockPos) -> BlockPos {
        BlockPos {
            y: pos.y - 1,
            ..pos
        }
    }

    fn above(pos: BlockPos) -> BlockPos {
        BlockPos {
            y: pos.y + 1,
            ..pos
        }
    }
}

/// Vanilla blocks whose `minecraft:simple_block` placement is a two-block
/// `placeAt` (double plants) or `MossyCarpetBlock.placeAt`, neither of which the
/// A1 partial layer implements.
pub(crate) fn is_multi_block_place(path: &str) -> bool {
    DOUBLE_PLANT_BLOCKS.contains(&path) || MOSSY_CARPET_BLOCKS.contains(&path)
}

/// One village `feature_pool_element`, resolved and compiled by
/// [`compile_pool_features`].
#[derive(Debug, Clone)]
pub struct CompiledPoolFeature {
    pub pool: Identifier,
    pub placed_feature: Identifier,
    /// Template-pool element weight, carried for the jigsaw assembly step.
    pub weight: u32,
    pub feature: CompiledPlacedFeature,
}

#[derive(Debug, Error)]
pub enum GlueError {
    #[error(transparent)]
    Closure(#[from] mc_data::vanilla_feature_closure::ClosureError),
    #[error(transparent)]
    Compile(#[from] CompileError),
}

/// Resolve a structure template pool's feature elements from a vanilla
/// `worldgen` directory and compile them: the single data → execution entry
/// point for the village decor closure.
///
/// Resolution is by reference ([`mc_data::vanilla_feature_closure::FeatureClosure`]),
/// so entries outside the pool's closure are never read; anything the closure
/// reaches that this partial layer does not implement fails closed, naming the
/// type id and the referring entry.
pub fn compile_pool_features(
    worldgen_dir: impl AsRef<std::path::Path>,
    pool: &Identifier,
    semantics: &BlockSemantics<'_>,
) -> Result<Vec<CompiledPoolFeature>, GlueError> {
    let closure = mc_data::vanilla_feature_closure::FeatureClosure::new(worldgen_dir.as_ref());
    closure
        .load_pool_features(pool)?
        .into_iter()
        .map(|entry| {
            Ok(CompiledPoolFeature {
                pool: entry.pool,
                placed_feature: entry.placed_feature,
                weight: entry.weight,
                feature: CompiledPlacedFeature::compile(&entry.spec, semantics)?,
            })
        })
        .collect()
}

fn identifier(value: &str) -> Identifier {
    Identifier::parse(value.to_owned()).expect("vanilla block and tag identifiers are valid")
}
