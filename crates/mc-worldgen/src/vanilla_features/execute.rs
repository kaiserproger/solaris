//! Compiled placed features and their execution.
//!
//! `CompiledPlacedFeature` mirrors vanilla `PlacedFeature.place`: build the
//! position stream by folding the placement modifiers over the origin, then run
//! the configured feature at each surviving position. The fold is written as a
//! lazy pull (`placement::walk` is a recursion, not a collect) because vanilla's
//! `Stream.flatMap` chain interleaves the modifiers' draws with the feature's
//! own draws: for `count → random_offset → filter`, the offset draw for the
//! second position happens *after* the feature has placed the first one. A
//! collect-then-place pipeline would consume the same draws in a different order
//! and diverge from vanilla.

use mc_data::block_facts::SturdyFace;
use mc_data::vanilla_feature_closure::{ConfiguredFeatureKind, PlacedFeatureSpec};
use mc_world::BlockPos;

use super::placement::{CompiledIntProvider, CompiledModifier};
use super::provider::CompiledStateProvider;
use super::random::RandomSource;
use super::{BlockSemantics, CompileError, FeatureLevel, PlaceError};

/// A configured feature compiled against the block registry.
#[derive(Debug, Clone)]
enum CompiledConfiguredFeature {
    SimpleBlock {
        to_place: CompiledStateProvider,
    },
    BlockPile {
        state_provider: CompiledStateProvider,
    },
    BlockColumn {
        layers: Vec<CompiledColumnLayer>,
        direction: (i32, i32, i32),
        allowed_placement: super::CompiledPredicate,
        prioritize_tip: bool,
    },
    Tree(Box<super::tree::CompiledTree>),
}

#[derive(Debug, Clone)]
struct CompiledColumnLayer {
    height: CompiledIntProvider,
    provider: CompiledStateProvider,
}

/// A placed feature whose specs, states and noise are fully resolved: nothing
/// is looked up at execution time except the world itself.
#[derive(Debug, Clone)]
pub struct CompiledPlacedFeature {
    feature: CompiledConfiguredFeature,
    placement: Vec<CompiledModifier>,
}

impl CompiledPlacedFeature {
    /// Compile a resolved closure entry. Fails closed for this partial layer:
    /// unmodelled block behaviour, unsupported `schedule_tick`, tree parts the
    /// layer does not implement, and providers whose states cannot be resolved
    /// in the block registry.
    pub fn compile(
        spec: &PlacedFeatureSpec,
        semantics: &BlockSemantics<'_>,
    ) -> Result<Self, CompileError> {
        let owner = &spec.feature.id;
        let feature = match &spec.feature.kind {
            ConfiguredFeatureKind::SimpleBlock {
                to_place,
                schedule_tick,
            } => {
                if *schedule_tick {
                    return Err(CompileError::UnsupportedScheduleTick {
                        owner: owner.clone(),
                    });
                }
                let to_place = CompiledStateProvider::compile(semantics, owner, to_place)?;
                to_place.require_implemented_survival(owner, semantics)?;
                CompiledConfiguredFeature::SimpleBlock { to_place }
            }
            ConfiguredFeatureKind::BlockPile { state_provider } => {
                CompiledConfiguredFeature::BlockPile {
                    state_provider: CompiledStateProvider::compile(
                        semantics,
                        owner,
                        state_provider,
                    )?,
                }
            }
            ConfiguredFeatureKind::BlockColumn {
                layers,
                direction,
                allowed_placement,
                prioritize_tip,
            } => CompiledConfiguredFeature::BlockColumn {
                layers: layers
                    .iter()
                    .map(|layer| {
                        Ok(CompiledColumnLayer {
                            height: CompiledIntProvider::compile(owner, &layer.height)?,
                            provider: CompiledStateProvider::compile(
                                semantics,
                                owner,
                                &layer.provider,
                            )?,
                        })
                    })
                    .collect::<Result<Vec<_>, CompileError>>()?,
                direction: direction.offset(),
                allowed_placement: super::CompiledPredicate::compile(
                    semantics,
                    owner,
                    allowed_placement,
                )?,
                prioritize_tip: *prioritize_tip,
            },
            ConfiguredFeatureKind::Tree(tree) => CompiledConfiguredFeature::Tree(Box::new(
                super::tree::CompiledTree::compile(semantics, owner, tree)?,
            )),
        };
        let placement = spec
            .placement
            .iter()
            .map(|modifier| CompiledModifier::compile(semantics, owner, modifier))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { feature, placement })
    }

    /// Vanilla `PlacedFeature.place(level, generator, random, origin)`: true
    /// when any placement attempt reported that it placed something.
    ///
    /// Fallible because a faithful execution can reach a boundary this layer
    /// refuses to cross silently ([`PlaceError`]).
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
    ) -> Result<bool, PlaceError> {
        let mut placed_any = false;
        self.walk(0, level, semantics, random, origin, &mut placed_any)?;
        Ok(placed_any)
    }

    fn walk(
        &self,
        index: usize,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        pos: BlockPos,
        placed_any: &mut bool,
    ) -> Result<(), PlaceError> {
        let Some(modifier) = self.placement.get(index) else {
            if self.place_feature(level, semantics, random, pos)? {
                *placed_any = true;
            }
            return Ok(());
        };
        match modifier {
            CompiledModifier::Count(count) => {
                let count = count.sample(random);
                for _ in 0..count {
                    self.walk(index + 1, level, semantics, random, pos, placed_any)?;
                }
            }
            CompiledModifier::RandomOffset {
                xz_spread,
                y_spread,
            } => {
                let x = pos.x + xz_spread.sample(random);
                let y = pos.y + y_spread.sample(random);
                let z = pos.z + xz_spread.sample(random);
                self.walk(
                    index + 1,
                    level,
                    semantics,
                    random,
                    BlockPos { x, y, z },
                    placed_any,
                )?;
            }
            CompiledModifier::BlockPredicateFilter(predicate) => {
                if predicate.test(level, semantics, pos) {
                    self.walk(index + 1, level, semantics, random, pos, placed_any)?;
                }
            }
        }
        Ok(())
    }

    fn place_feature(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
    ) -> Result<bool, PlaceError> {
        match &self.feature {
            CompiledConfiguredFeature::SimpleBlock { to_place } => {
                Ok(self.place_simple_block(level, semantics, random, origin, to_place))
            }
            CompiledConfiguredFeature::BlockPile { state_provider } => {
                Ok(self.place_block_pile(level, semantics, random, origin, state_provider))
            }
            CompiledConfiguredFeature::Tree(tree) => tree.place(level, semantics, random, origin),
            CompiledConfiguredFeature::BlockColumn {
                layers,
                direction,
                allowed_placement,
                prioritize_tip,
            } => Ok(self.place_block_column(
                level,
                semantics,
                random,
                origin,
                layers,
                *direction,
                allowed_placement,
                *prioritize_tip,
            )),
        }
    }

    /// Vanilla `SimpleBlockFeature.place`.
    fn place_simple_block(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
        to_place: &CompiledStateProvider,
    ) -> bool {
        let Some(state) = to_place.optional_state(level, semantics, random, origin) else {
            return false;
        };
        if !semantics.can_survive(level, state, origin) {
            return false;
        }
        // Multi-block placements (double plants, mossy carpets) are refused at
        // compile time; vanilla routes those through `placeAt`.
        debug_assert!(
            semantics
                .blocks()
                .by_id(state)
                .is_some_and(|state| !super::is_multi_block_place(state.block.id.path())),
            "multi-block placement must fail closed at compile time"
        );
        level.set_block(origin, state);
        true
    }

    /// Vanilla `BlockPileFeature.place`.
    fn place_block_pile(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
        state_provider: &CompiledStateProvider,
    ) -> bool {
        if origin.y < level.min_y() + 5 {
            return false;
        }
        let xr = 2 + random.next_int_bounded(2);
        let zr = 2 + random.next_int_bounded(2);

        let min = BlockPos {
            x: origin.x - xr,
            y: origin.y,
            z: origin.z - zr,
        };
        let max = BlockPos {
            x: origin.x + xr,
            y: origin.y + 1,
            z: origin.z + zr,
        };
        let width = (max.x - min.x + 1) as usize;
        let height = (max.y - min.y + 1) as usize;
        let depth = (max.z - min.z + 1) as usize;
        for index in 0..width * height * depth {
            let pos = BlockPos {
                x: min.x + (index % width) as i32,
                y: min.y + ((index / width) % height) as i32,
                z: min.z + ((index / width) / height) as i32,
            };
            let xd = origin.x - pos.x;
            let zd = origin.z - pos.z;
            let radius = (xd * xd + zd * zd) as f32;
            if radius <= random.next_float() * 10.0 - random.next_float() * 6.0
                || random.next_float() < 0.031
            {
                self.try_place_pile_block(level, semantics, random, pos, state_provider);
            }
        }
        true
    }

    /// Vanilla `BlockPileFeature.tryPlaceBlock` / `mayPlaceOn`.
    fn try_place_pile_block(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        pos: BlockPos,
        state_provider: &CompiledStateProvider,
    ) {
        if !semantics.is_air(level.block_state(pos)) {
            return;
        }
        let below = BlockPos {
            y: pos.y - 1,
            ..pos
        };
        let placeable = if semantics.is_block(
            level.block_state(below),
            &super::identifier("minecraft:dirt_path"),
        ) {
            random.next_boolean()
        } else {
            semantics.is_face_sturdy(level, below, SturdyFace::Up)
        };
        if !placeable {
            return;
        }
        let state = state_provider.state(level, semantics, random, pos);
        level.set_block(pos, state);
    }

    /// Vanilla `BlockColumnFeature.place`, including its tip truncation. Note
    /// the vanilla quirk this reproduces: `allowedPlacement` is tested at
    /// `origin + direction * (y + 1)` while the blocks are written from
    /// `origin + direction * y`.
    #[allow(clippy::too_many_arguments)]
    fn place_block_column(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
        layers: &[CompiledColumnLayer],
        direction: (i32, i32, i32),
        allowed_placement: &super::CompiledPredicate,
        prioritize_tip: bool,
    ) -> bool {
        let mut layer_heights: Vec<i32> = layers
            .iter()
            .map(|layer| layer.height.sample(random))
            .collect();
        let total_height: i32 = layer_heights.iter().sum();
        if total_height == 0 {
            return false;
        }
        let step = |pos: BlockPos| BlockPos {
            x: pos.x + direction.0,
            y: pos.y + direction.1,
            z: pos.z + direction.2,
        };

        let mut check = step(origin);
        for y in 0..total_height {
            if !allowed_placement.test(level, semantics, check) {
                truncate(&mut layer_heights, total_height, y, prioritize_tip);
                break;
            }
            check = step(check);
        }

        let mut place = origin;
        for (layer, height) in layers.iter().zip(&layer_heights) {
            for _ in 0..*height {
                let state = layer.provider.state(level, semantics, random, place);
                level.set_block(place, state);
                place = step(place);
            }
        }
        true
    }
}

/// Vanilla `BlockColumnFeature.truncate`: remove `totalHeight - newHeight`
/// blocks from the tip-most layer first when `prioritizeTip`, from the deepest
/// layer first otherwise.
fn truncate(layer_heights: &mut [i32], total_height: i32, new_height: i32, prioritize_tip: bool) {
    let mut amount_to_remove = total_height - new_height;
    let direction: i32 = if prioritize_tip { 1 } else { -1 };
    let start: i32 = if prioritize_tip {
        0
    } else {
        layer_heights.len() as i32 - 1
    };
    let end: i32 = if prioritize_tip {
        layer_heights.len() as i32
    } else {
        -1
    };
    let mut index = start;
    while index != end && amount_to_remove > 0 {
        let layer = layer_heights[index as usize];
        let remove = layer.min(amount_to_remove);
        amount_to_remove -= remove;
        layer_heights[index as usize] -= remove;
        index += direction;
    }
}
