//! Compiled placement modifiers, int providers and block predicates.
//!
//! Semantics follow `CountPlacement`/`RandomOffsetPlacement`/
//! `BlockPredicateFilter`, `IntProvider`'s reachable implementations, and the
//! block predicates the village closure reaches.

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::{
    BlockPredicateSpec, IntProviderSpec, PlacementModifierSpec,
};
use mc_world::{BlockPos, BlockStateId};

use super::random::RandomSource;
use super::{BlockSemantics, CompileError, FeatureLevel};

/// A compiled `IntProvider`.
#[derive(Debug, Clone)]
pub(crate) enum CompiledIntProvider {
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
    WeightedList {
        entries: Vec<(CompiledIntProvider, i32)>,
        total_weight: i32,
    },
}

impl CompiledIntProvider {
    pub(crate) fn compile(
        owner: &Identifier,
        spec: &IntProviderSpec,
    ) -> Result<Self, CompileError> {
        Ok(match spec {
            IntProviderSpec::Constant(value) => Self::Constant(*value),
            IntProviderSpec::Uniform {
                min_inclusive,
                max_inclusive,
            } => Self::Uniform {
                min_inclusive: *min_inclusive,
                max_inclusive: *max_inclusive,
            },
            IntProviderSpec::Trapezoid {
                min_inclusive,
                max_inclusive,
                plateau,
            } => Self::Trapezoid {
                min_inclusive: *min_inclusive,
                max_inclusive: *max_inclusive,
                plateau: *plateau,
            },
            IntProviderSpec::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => Self::BiasedToBottom {
                min_inclusive: *min_inclusive,
                max_inclusive: *max_inclusive,
            },
            IntProviderSpec::WeightedList(entries) => {
                let mut compiled = Vec::with_capacity(entries.len());
                let mut total_weight = 0i32;
                for entry in entries {
                    compiled.push((Self::compile(owner, &entry.provider)?, entry.weight));
                    total_weight = total_weight.saturating_add(entry.weight);
                }
                if compiled.is_empty() || total_weight <= 0 {
                    return Err(CompileError::EmptyWeightedList {
                        owner: owner.clone(),
                    });
                }
                Self::WeightedList {
                    entries: compiled,
                    total_weight,
                }
            }
        })
    }

    /// Vanilla `IntProvider.sample`.
    pub(crate) fn sample(&self, random: &mut impl RandomSource) -> i32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform {
                min_inclusive,
                max_inclusive,
            } => random_between_inclusive(random, *min_inclusive, *max_inclusive),
            Self::Trapezoid {
                min_inclusive,
                max_inclusive,
                plateau,
            } => {
                if *plateau == 0 && *max_inclusive == -*min_inclusive {
                    return random.next_int_bounded(*max_inclusive + 1)
                        - random.next_int_bounded(*max_inclusive + 1);
                }
                let range = *max_inclusive - *min_inclusive;
                if *plateau == range {
                    return random_between_inclusive(random, *min_inclusive, *max_inclusive);
                }
                let plateau_start = (range - *plateau) / 2;
                let plateau_end = range - plateau_start;
                *min_inclusive
                    + random_between_inclusive(random, 0, plateau_end)
                    + random_between_inclusive(random, 0, plateau_start)
            }
            Self::BiasedToBottom {
                min_inclusive,
                max_inclusive,
            } => {
                let outer_bound = random.next_int_bounded(*max_inclusive - *min_inclusive + 1) + 1;
                *min_inclusive + random.next_int_bounded(outer_bound)
            }
            Self::WeightedList {
                entries,
                total_weight,
            } => {
                let mut selection = random.next_int_bounded(*total_weight);
                for (provider, weight) in entries {
                    selection -= weight;
                    if selection < 0 {
                        return provider.sample(random);
                    }
                }
                unreachable!("weighted int selection is bounded by the total weight")
            }
        }
    }
}

/// `Mth.randomBetweenInclusive`.
fn random_between_inclusive(random: &mut impl RandomSource, min: i32, max: i32) -> i32 {
    random.next_int_bounded(max - min + 1) + min
}

/// A compiled `BlockPredicate`.
#[derive(Debug, Clone)]
pub enum CompiledPredicate {
    WouldSurvive {
        offset: (i32, i32, i32),
        state: BlockStateId,
    },
    MatchingBlockTag {
        offset: (i32, i32, i32),
        tag: Identifier,
    },
    MatchingBlocks {
        offset: (i32, i32, i32),
        blocks: Vec<Identifier>,
    },
    AllOf(Vec<CompiledPredicate>),
    Not(Box<CompiledPredicate>),
}

impl CompiledPredicate {
    pub(crate) fn compile(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &BlockPredicateSpec,
    ) -> Result<Self, CompileError> {
        Ok(match spec {
            BlockPredicateSpec::WouldSurvive { offset, state } => {
                require_implemented_survival(owner, semantics, &state.block)?;
                Self::WouldSurvive {
                    offset: *offset,
                    state: semantics.resolve_state(owner, state)?,
                }
            }
            BlockPredicateSpec::MatchingBlockTag { offset, tag } => Self::MatchingBlockTag {
                offset: *offset,
                tag: tag.clone(),
            },
            BlockPredicateSpec::MatchingBlocks { offset, blocks } => Self::MatchingBlocks {
                offset: *offset,
                blocks: blocks.clone(),
            },
            BlockPredicateSpec::AllOf { predicates } => Self::AllOf(
                predicates
                    .iter()
                    .map(|predicate| Self::compile(semantics, owner, predicate))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            BlockPredicateSpec::Not { predicate } => {
                Self::Not(Box::new(Self::compile(semantics, owner, predicate)?))
            }
        })
    }

    /// Vanilla `BlockPredicate.test(level, pos)`.
    #[must_use]
    pub fn test(
        &self,
        level: &dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        pos: BlockPos,
    ) -> bool {
        match self {
            Self::WouldSurvive { offset, state } => {
                semantics.can_survive(level, *state, offset_pos(pos, *offset))
            }
            Self::MatchingBlockTag { offset, tag } => {
                semantics.in_tag(level.block_state(offset_pos(pos, *offset)), tag)
            }
            Self::MatchingBlocks { offset, blocks } => {
                let state = level.block_state(offset_pos(pos, *offset));
                blocks.iter().any(|block| semantics.is_block(state, block))
            }
            Self::AllOf(predicates) => predicates
                .iter()
                .all(|predicate| predicate.test(level, semantics, pos)),
            Self::Not(predicate) => !predicate.test(level, semantics, pos),
        }
    }
}

/// The A1 layer models survival for the vegetation and cactus families only;
/// anything else that a predicate or provider would ask `canSurvive` about is
/// refused at compile time rather than answered wrongly at run time.
pub(crate) fn require_implemented_survival(
    owner: &Identifier,
    semantics: &BlockSemantics<'_>,
    block: &Identifier,
) -> Result<(), CompileError> {
    if semantics.survival_is_implemented(block) {
        return Ok(());
    }
    Err(CompileError::UnsupportedBlockBehaviour {
        owner: owner.clone(),
        block: block.clone(),
    })
}

fn offset_pos(pos: BlockPos, offset: (i32, i32, i32)) -> BlockPos {
    BlockPos {
        x: pos.x + offset.0,
        y: pos.y + offset.1,
        z: pos.z + offset.2,
    }
}

/// A compiled placement modifier from the placed feature's `placement` list.
#[derive(Debug, Clone)]
pub(crate) enum CompiledModifier {
    Count(CompiledIntProvider),
    RandomOffset {
        xz_spread: CompiledIntProvider,
        y_spread: CompiledIntProvider,
    },
    BlockPredicateFilter(CompiledPredicate),
}

impl CompiledModifier {
    pub(crate) fn compile(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &PlacementModifierSpec,
    ) -> Result<Self, CompileError> {
        Ok(match spec {
            PlacementModifierSpec::Count(count) => {
                Self::Count(CompiledIntProvider::compile(owner, count)?)
            }
            PlacementModifierSpec::RandomOffset {
                xz_spread,
                y_spread,
            } => Self::RandomOffset {
                xz_spread: CompiledIntProvider::compile(owner, xz_spread)?,
                y_spread: CompiledIntProvider::compile(owner, y_spread)?,
            },
            PlacementModifierSpec::BlockPredicateFilter(predicate) => {
                Self::BlockPredicateFilter(CompiledPredicate::compile(semantics, owner, predicate)?)
            }
        })
    }
}
