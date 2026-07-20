use super::{EffectInstance, EffectKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetEffectContext {
    pub inverted_heal_and_harm: bool,
}

impl TargetEffectContext {
    pub const LIVING: Self = Self {
        inverted_heal_and_harm: false,
    };
    pub const INVERTED_HEAL_AND_HARM: Self = Self {
        inverted_heal_and_harm: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectDamageSource {
    Magic,
    IndirectMagic,
    Wither,
}

/// A gameplay operation whose execution remains caller-owned.
///
/// `HealIfBelowMax` uses a strict `health < max_health` comparison and
/// `MagicDamageIfHealthAbove` uses `health > minimum_health`, matching the Java
/// `float` predicates. Player-only actions are no-ops for non-player targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectAction {
    HealIfBelowMax {
        amount: f32,
    },
    MagicDamageIfHealthAbove {
        amount: f32,
        minimum_health: f32,
    },
    Damage {
        amount: f32,
        source: EffectDamageSource,
    },
    ExhaustPlayer {
        amount: f32,
    },
    FeedPlayer {
        food: i32,
        saturation_modifier: f32,
    },
    Heal {
        amount: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectApplication {
    None,
    /// The decompiled effect callback always returns `true`; execution still
    /// belongs to the caller before tick commit.
    Supported(EffectAction),
    /// The caller must evaluate both `shouldApplyEffectTickThisTick` and the
    /// callback result, then resolve the pending tick.
    CallerOwned {
        tick_count: i32,
        amplifier: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantDelivery {
    Direct,
    Indirect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InstantApplication {
    Supported(EffectAction),
    CallerOwned {
        amplifier: u8,
        scale: f64,
        delivery: InstantDelivery,
        target: TargetEffectContext,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstantPlanError {
    NotInstant(EffectKind),
}

/// Plans `MobEffect.applyInstantenousEffect` without mutating target state.
///
/// `delivery` only selects direct magic versus indirect magic damage for the
/// heal/harm override. Saturation inherits the base implementation and ignores
/// both `scale` and `delivery`. Heal/harm accepts every `f64`; Rust's float to
/// integer cast has the same boundary results as Java here: NaN becomes zero
/// and infinities saturate to the corresponding `i32` bound.
pub fn plan_instant_application(
    effect: EffectInstance,
    target: TargetEffectContext,
    scale: f64,
    delivery: InstantDelivery,
) -> Result<InstantApplication, InstantPlanError> {
    let action = match effect.kind {
        EffectKind::Saturation => EffectAction::FeedPlayer {
            food: i32::from(effect.amplifier) + 1,
            saturation_modifier: 1.0,
        },
        EffectKind::InstantHealth => scaled_heal_or_harm(
            false,
            effect.amplifier,
            target.inverted_heal_and_harm,
            scale,
            delivery,
        ),
        EffectKind::InstantDamage => scaled_heal_or_harm(
            true,
            effect.amplifier,
            target.inverted_heal_and_harm,
            scale,
            delivery,
        ),
        EffectKind::CallerOwned => {
            return Ok(InstantApplication::CallerOwned {
                amplifier: effect.amplifier,
                scale,
                delivery,
                target,
            });
        }
        kind => return Err(InstantPlanError::NotInstant(kind)),
    };
    Ok(InstantApplication::Supported(action))
}

pub(crate) fn tick_application(
    effect: EffectInstance,
    entity_tick_count: i32,
    target: TargetEffectContext,
) -> EffectApplication {
    let tick_count = if effect.is_infinite() {
        entity_tick_count
    } else {
        effect.duration
    };
    let applies = match effect.kind {
        EffectKind::Regeneration => applies_at_interval(tick_count, 50, effect.amplifier),
        EffectKind::Poison => applies_at_interval(tick_count, 25, effect.amplifier),
        EffectKind::Wither => applies_at_interval(tick_count, 40, effect.amplifier),
        EffectKind::Hunger => true,
        EffectKind::Saturation | EffectKind::InstantHealth | EffectKind::InstantDamage => {
            tick_count >= 1
        }
        EffectKind::CallerOwned => {
            return EffectApplication::CallerOwned {
                tick_count,
                amplifier: effect.amplifier,
            };
        }
    };
    if !applies {
        return EffectApplication::None;
    }

    let action = match effect.kind {
        EffectKind::Regeneration => EffectAction::HealIfBelowMax { amount: 1.0 },
        EffectKind::Poison => EffectAction::MagicDamageIfHealthAbove {
            amount: 1.0,
            minimum_health: 1.0,
        },
        EffectKind::Wither => EffectAction::Damage {
            amount: 1.0,
            source: EffectDamageSource::Wither,
        },
        EffectKind::Hunger => EffectAction::ExhaustPlayer {
            amount: 0.005_f32 * (f32::from(effect.amplifier) + 1.0_f32),
        },
        EffectKind::Saturation => EffectAction::FeedPlayer {
            food: i32::from(effect.amplifier) + 1,
            saturation_modifier: 1.0,
        },
        EffectKind::InstantHealth => {
            unscaled_heal_or_harm(false, effect.amplifier, target.inverted_heal_and_harm)
        }
        EffectKind::InstantDamage => {
            unscaled_heal_or_harm(true, effect.amplifier, target.inverted_heal_and_harm)
        }
        EffectKind::CallerOwned => unreachable!("caller-owned effects returned above"),
    };
    EffectApplication::Supported(action)
}

fn applies_at_interval(tick_count: i32, base: i32, amplifier: u8) -> bool {
    let interval = base >> (u32::from(amplifier) & 31);
    interval <= 0 || tick_count % interval == 0
}

fn unscaled_heal_or_harm(is_harm: bool, amplifier: u8, inverted: bool) -> EffectAction {
    if is_harm == inverted {
        let shifted = 4_i32.wrapping_shl(u32::from(amplifier) & 31).max(0);
        EffectAction::Heal {
            amount: shifted as f32,
        }
    } else {
        let shifted = 6_i32.wrapping_shl(u32::from(amplifier) & 31);
        EffectAction::Damage {
            amount: shifted as f32,
            source: EffectDamageSource::Magic,
        }
    }
}

fn scaled_heal_or_harm(
    is_harm: bool,
    amplifier: u8,
    inverted: bool,
    scale: f64,
    delivery: InstantDelivery,
) -> EffectAction {
    let (base, heals) = if is_harm == inverted {
        (4_i32.wrapping_shl(u32::from(amplifier) & 31), true)
    } else {
        (6_i32.wrapping_shl(u32::from(amplifier) & 31), false)
    };
    let amount = (scale * f64::from(base) + 0.5) as i32;
    if heals {
        EffectAction::Heal {
            amount: amount as f32,
        }
    } else {
        EffectAction::Damage {
            amount: amount as f32,
            source: match delivery {
                InstantDelivery::Direct => EffectDamageSource::Magic,
                InstantDelivery::Indirect => EffectDamageSource::IndirectMagic,
            },
        }
    }
}
