//! Compiled block-state providers.
//!
//! Semantics follow `BlockStateProvider` and the implementations the village
//! closure reaches: `SimpleStateProvider`, `WeightedStateProvider`,
//! `RotatedBlockProvider`, `NoiseThresholdProvider`,
//! `RuleBasedStateProvider`. Noise-backed providers build their `NormalNoise`
//! once, at compile time, exactly as `NoiseBasedStateProvider`'s constructor
//! does (the seed is the provider's own, not the world's).

use mc_data::Identifier;
use mc_data::vanilla_feature_closure::StateProviderSpec;
use mc_world::{BlockPos, BlockStateId};

use super::placement::require_implemented_survival;
use super::random::{LegacyRandom, RandomSource};
use super::synth::NormalNoise;
use super::{BlockSemantics, CompileError, FeatureLevel};

/// A compiled `BlockStateProvider`.
#[derive(Debug, Clone)]
pub enum CompiledStateProvider {
    Simple(BlockStateId),
    Weighted {
        entries: Vec<(BlockStateId, i32)>,
        total_weight: i32,
    },
    /// `RotatedBlockProvider` keeps the written block and draws
    /// `Direction.Axis.getRandom(random)`; `trySetValue` leaves the default
    /// state when the block has no `axis` property.
    Rotated {
        default_state: BlockStateId,
        axis_states: Option<[BlockStateId; 3]>,
    },
    NoiseThreshold {
        noise: NormalNoise,
        scale: f32,
        threshold: f32,
        high_chance: f32,
        default_state: BlockStateId,
        low_states: Vec<BlockStateId>,
        high_states: Vec<BlockStateId>,
    },
    RuleBased {
        fallback: Option<Box<CompiledStateProvider>>,
        rules: Vec<(super::CompiledPredicate, CompiledStateProvider)>,
    },
}

impl CompiledStateProvider {
    pub(crate) fn compile(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &StateProviderSpec,
    ) -> Result<Self, CompileError> {
        Ok(match spec {
            StateProviderSpec::Simple { state } => {
                let state = semantics.resolve_state(owner, state)?;
                Self::Simple(state)
            }
            StateProviderSpec::Weighted { entries } => {
                let mut compiled = Vec::with_capacity(entries.len());
                let mut total_weight = 0i32;
                for entry in entries {
                    let state = semantics.resolve_state(owner, &entry.state)?;
                    total_weight = total_weight.saturating_add(entry.weight);
                    compiled.push((state, entry.weight));
                }
                if compiled.is_empty() || total_weight <= 0 {
                    return Err(CompileError::EmptyWeightedList {
                        owner: owner.clone(),
                    });
                }
                Self::Weighted {
                    entries: compiled,
                    total_weight,
                }
            }
            StateProviderSpec::Rotated { block } => {
                let default_state = semantics
                    .blocks()
                    .block(block)
                    .map(|block| block.default)
                    .ok_or_else(|| CompileError::UnknownBlockState {
                        owner: owner.clone(),
                        block: block.clone(),
                    })?;
                Self::Rotated {
                    default_state,
                    axis_states: semantics.axis_states(block),
                }
            }
            StateProviderSpec::NoiseThreshold(spec) => {
                if spec.low_states.is_empty() {
                    return Err(CompileError::EmptyNoiseStates {
                        owner: owner.clone(),
                        field: "low_states",
                    });
                }
                if spec.high_states.is_empty() {
                    return Err(CompileError::EmptyNoiseStates {
                        owner: owner.clone(),
                        field: "high_states",
                    });
                }
                let mut seed = LegacyRandom::new(spec.seed);
                Self::NoiseThreshold {
                    noise: NormalNoise::create(
                        &mut seed,
                        spec.noise.first_octave,
                        &spec.noise.amplitudes,
                    ),
                    scale: spec.scale,
                    threshold: spec.threshold,
                    high_chance: spec.high_chance,
                    default_state: semantics.resolve_state(owner, &spec.default_state)?,
                    low_states: spec
                        .low_states
                        .iter()
                        .map(|state| semantics.resolve_state(owner, state))
                        .collect::<Result<Vec<_>, _>>()?,
                    high_states: spec
                        .high_states
                        .iter()
                        .map(|state| semantics.resolve_state(owner, state))
                        .collect::<Result<Vec<_>, _>>()?,
                }
            }
            StateProviderSpec::RuleBased { fallback, rules } => Self::RuleBased {
                fallback: fallback
                    .as_ref()
                    .map(|fallback| Self::compile(semantics, owner, fallback).map(Box::new))
                    .transpose()?,
                rules: rules
                    .iter()
                    .map(|rule| {
                        Ok((
                            super::CompiledPredicate::compile(semantics, owner, &rule.predicate)?,
                            Self::compile(semantics, owner, &rule.then)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, CompileError>>()?,
            },
        })
    }

    /// Vanilla `BlockStateProvider.getState`.
    #[must_use]
    pub fn state(
        &self,
        level: &dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        pos: BlockPos,
    ) -> BlockStateId {
        self.optional_state(level, semantics, random, pos)
            .unwrap_or_else(|| level.block_state(pos))
    }

    /// Vanilla `BlockStateProvider.getOptionalState`: only
    /// `RuleBasedStateProvider` can return nothing (no rule matched and no
    /// fallback), which `SimpleBlockFeature` turns into "do not place".
    #[must_use]
    pub fn optional_state(
        &self,
        level: &dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        pos: BlockPos,
    ) -> Option<BlockStateId> {
        match self {
            Self::Simple(state) => Some(*state),
            Self::Weighted {
                entries,
                total_weight,
            } => {
                let mut selection = random.next_int_bounded(*total_weight);
                for (state, weight) in entries {
                    selection -= weight;
                    if selection < 0 {
                        return Some(*state);
                    }
                }
                unreachable!("weighted state selection is bounded by the total weight")
            }
            Self::Rotated {
                default_state,
                axis_states,
            } => {
                let axis = random.next_int_bounded(3) as usize;
                Some(match axis_states {
                    Some(states) => states[axis],
                    None => *default_state,
                })
            }
            Self::NoiseThreshold {
                noise,
                scale,
                threshold,
                high_chance,
                default_state,
                low_states,
                high_states,
            } => {
                let local_value = noise.value(
                    f64::from(pos.x) * f64::from(*scale),
                    f64::from(pos.y) * f64::from(*scale),
                    f64::from(pos.z) * f64::from(*scale),
                );
                if local_value < f64::from(*threshold) {
                    let index = random.next_int_bounded(low_states.len() as i32);
                    Some(low_states[index as usize])
                } else if random.next_float() < *high_chance {
                    let index = random.next_int_bounded(high_states.len() as i32);
                    Some(high_states[index as usize])
                } else {
                    Some(*default_state)
                }
            }
            Self::RuleBased { fallback, rules } => {
                // Vanilla resolves nested providers with `getState`, not
                // `getOptionalState`: a nested rule-based provider with no
                // matching rule and no fallback yields the *existing* block
                // state, which is non-null, so only this provider's own
                // no-rule/no-fallback case returns nothing.
                for (predicate, then) in rules {
                    if predicate.test(level, semantics, pos) {
                        return Some(then.state(level, semantics, random, pos));
                    }
                }
                fallback
                    .as_ref()
                    .map(|fallback| fallback.state(level, semantics, random, pos))
            }
        }
    }

    /// Validate that every state this provider can produce has an implemented
    /// `canSurvive` in the A1 layer. Called for `minecraft:simple_block`'s
    /// `to_place`, which is the only path that consults survival.
    pub(crate) fn require_implemented_survival(
        &self,
        owner: &Identifier,
        semantics: &BlockSemantics<'_>,
    ) -> Result<(), CompileError> {
        match self {
            Self::Simple(state) => require_state_survival(owner, semantics, *state),
            Self::Weighted { entries, .. } => {
                for (state, _) in entries {
                    require_state_survival(owner, semantics, *state)?;
                }
                Ok(())
            }
            Self::Rotated {
                default_state,
                axis_states,
            } => {
                require_state_survival(owner, semantics, *default_state)?;
                if let Some(states) = axis_states {
                    for state in states {
                        require_state_survival(owner, semantics, *state)?;
                    }
                }
                Ok(())
            }
            Self::NoiseThreshold {
                default_state,
                low_states,
                high_states,
                ..
            } => {
                for state in std::iter::once(default_state)
                    .chain(low_states)
                    .chain(high_states)
                {
                    require_state_survival(owner, semantics, *state)?;
                }
                Ok(())
            }
            Self::RuleBased { fallback, rules } => {
                for (_, then) in rules {
                    then.require_implemented_survival(owner, semantics)?;
                }
                if let Some(fallback) = fallback {
                    fallback.require_implemented_survival(owner, semantics)?;
                }
                Ok(())
            }
        }
    }
}

impl CompiledStateProvider {
    /// Every state this provider can produce (rule-based branches included).
    pub(crate) fn producible_states(&self) -> Vec<BlockStateId> {
        let mut states = Vec::new();
        self.collect_producible_states(&mut states);
        states
    }

    fn collect_producible_states(&self, out: &mut Vec<BlockStateId>) {
        match self {
            Self::Simple(state) => out.push(*state),
            Self::Weighted { entries, .. } => {
                out.extend(entries.iter().map(|(state, _)| *state));
            }
            Self::Rotated {
                default_state,
                axis_states,
            } => {
                out.push(*default_state);
                if let Some(states) = axis_states {
                    out.extend(states.iter().copied());
                }
            }
            Self::NoiseThreshold {
                default_state,
                low_states,
                high_states,
                ..
            } => {
                out.push(*default_state);
                out.extend(low_states.iter().copied());
                out.extend(high_states.iter().copied());
            }
            Self::RuleBased { fallback, rules } => {
                for (_, then) in rules {
                    then.collect_producible_states(out);
                }
                if let Some(fallback) = fallback {
                    fallback.collect_producible_states(out);
                }
            }
        }
    }
}

/// Resolved states are registry states, so the block is always present; an
/// absent one is a programming error rather than a data error.
fn require_state_survival(
    owner: &Identifier,
    semantics: &BlockSemantics<'_>,
    state: BlockStateId,
) -> Result<(), CompileError> {
    let Some(block) = semantics.blocks().by_id(state) else {
        debug_assert!(
            false,
            "resolved state {state:?} is not in the block registry"
        );
        return Ok(());
    };
    let block = block.block.id.clone();
    if super::is_multi_block_place(block.path()) {
        return Err(CompileError::UnsupportedMultiBlockPlacement {
            owner: owner.clone(),
            block,
        });
    }
    require_implemented_survival(owner, semantics, &block)
}
