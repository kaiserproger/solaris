use super::reduction::{
    ReductionContext, ReductionError, ReductionField, UnsupportedReduction, reduce_damage,
};
use super::source::{DamageFlags, DamageSource, DamageSourceKind};
use super::state::{
    HURT_DURATION_TICKS, INVULNERABLE_DURATION_TICKS, LivingLifecycle, LivingState, StateError,
};

pub const VANILLA_RANDOM_KNOCKBACK_DIRECTION_SQUARED: f64 = 1.0e-5_f32 as f64;
const VANILLA_BASE_KNOCKBACK: f64 = 0.4_f32 as f64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub source: DamageSource,
    pub amount: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageContext {
    pub immunity: TargetImmunityContext,
    pub fire_resistance: bool,
    pub reductions: ReductionContext,
    pub knockback: Option<KnockbackInput>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CreativePlayerStatus {
    #[default]
    NotCreative,
    Creative,
    /// The adapter has not resolved `DamageSource.isCreativePlayer()`.
    Unsupported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EnchantmentImmunity {
    #[default]
    NotImmune,
    Immune,
    /// The adapter has not evaluated
    /// `EnchantmentHelper.isImmuneToDamage` for this source.
    UnsupportedSourceEvaluation,
}

/// Caller-provided inputs for the complete verified
/// `Entity.isInvulnerableToBase || LivingEntity enchantment immunity`
/// predicate. Fields remain distinct because the invulnerability tag and a
/// creative causing player bypass only `ordinary_invulnerable`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetImmunityContext {
    pub ordinary_invulnerable: bool,
    pub fire_immune: bool,
    pub fall_damage_immune: bool,
    pub source_creative_player: CreativePlayerStatus,
    pub enchantment: EnchantmentImmunity,
}

/// Caller-resolved direction and current motion needed by
/// `LivingEntity.knockback`. Projectile adapters must supply the direction from
/// `calculateHorizontalHurtKnockbackDirection`, not a guessed source position.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KnockbackInput {
    pub direction_x: f64,
    pub direction_z: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub velocity_z: f64,
    pub on_ground: bool,
    pub resistance: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KnockbackVelocity {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum KnockbackOutcome {
    #[default]
    None,
    Velocity(KnockbackVelocity),
    /// Vanilla samples random horizontal components until the squared length
    /// reaches `1.0E-5F`. The pure kernel reports that requirement to its RNG
    /// owning adapter.
    RandomDirectionRequired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LifecycleTransition {
    #[default]
    None,
    StartedDying,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DamageApplied {
    pub raw_amount: f32,
    pub cooldown_damage: f32,
    pub after_armor: f32,
    pub after_magic: f32,
    pub absorbed: f32,
    /// Post-absorption damage recorded by vanilla even when it exceeds health.
    pub health_damage: f32,
    /// Actual scalar health delta, bounded by the old health.
    pub health_lost: f32,
    pub fresh_hurt: bool,
    pub mark_hurt: bool,
    pub knockback: KnockbackOutcome,
    pub lifecycle: LifecycleTransition,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DamageOutcome {
    Applied(DamageApplied),
    Rejected(DamageRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageRejection {
    InvalidState(StateError),
    NonFinite(DamageInputField),
    OutOfRange(DamageInputField),
    Unsupported(UnsupportedRule),
    Invulnerable,
    Removed,
    Dead,
    FireImmune,
    FallDamageImmune,
    EnchantmentImmune,
    FireResistance,
    HurtCooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageInputField {
    Amount,
    Armor,
    ArmorToughness,
    EnchantmentProtection,
    KnockbackDirection,
    KnockbackVelocity,
    KnockbackResistance,
    DerivedReduction,
    DerivedKnockback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedRule {
    DamageSource,
    CreativePlayerStatus,
    EnchantmentImmunityEvaluation,
    WeaponArmorEffectiveness,
    EnchantmentSourceEvaluation,
}

pub fn apply_damage(
    state: &mut LivingState,
    event: DamageEvent,
    context: DamageContext,
    output: &mut DamageOutcome,
) {
    if let Err(error) = state.validate() {
        *output = DamageOutcome::Rejected(DamageRejection::InvalidState(error));
        return;
    }
    if !event.amount.is_finite() {
        *output = DamageOutcome::Rejected(DamageRejection::NonFinite(DamageInputField::Amount));
        return;
    }
    if event.source.kind() == DamageSourceKind::Unsupported {
        *output =
            DamageOutcome::Rejected(DamageRejection::Unsupported(UnsupportedRule::DamageSource));
        return;
    }
    let flags = event.source.flags();
    match evaluate_immunity(state.lifecycle, flags, context.immunity) {
        Ok(Some(rejection)) => {
            *output = DamageOutcome::Rejected(rejection);
            return;
        }
        Ok(None) => {}
        Err(unsupported) => {
            *output = DamageOutcome::Rejected(DamageRejection::Unsupported(unsupported));
            return;
        }
    }
    if state.lifecycle != LivingLifecycle::Alive {
        *output = DamageOutcome::Rejected(DamageRejection::Dead);
        return;
    }
    if flags.contains(DamageFlags::IS_FIRE) && context.fire_resistance {
        *output = DamageOutcome::Rejected(DamageRejection::FireResistance);
        return;
    }

    let raw_amount = if event.amount < 0.0 {
        0.0
    } else {
        event.amount
    };
    let protected = state.invulnerable_time > HURT_DURATION_TICKS
        && !flags.contains(DamageFlags::BYPASSES_COOLDOWN);
    let (cooldown_damage, fresh_hurt) = if protected {
        if raw_amount <= state.last_hurt {
            *output = DamageOutcome::Rejected(DamageRejection::HurtCooldown);
            return;
        }
        (raw_amount - state.last_hurt, false)
    } else {
        (raw_amount, true)
    };

    let reduced = match reduce_damage(cooldown_damage, flags, context.reductions) {
        Ok(reduced) => reduced,
        Err(error) => {
            *output = DamageOutcome::Rejected(map_reduction_error(error));
            return;
        }
    };
    let knockback = if fresh_hurt && !flags.contains(DamageFlags::NO_KNOCKBACK) {
        match calculate_knockback(context.knockback) {
            Ok(knockback) => knockback,
            Err(rejection) => {
                *output = DamageOutcome::Rejected(rejection);
                return;
            }
        }
    } else {
        KnockbackOutcome::None
    };

    let (health_damage, absorbed, next_absorption) = if reduced.after_magic == f32::INFINITY {
        // A finite Java float can overflow resistance multiplication. It is
        // lethal. Vanilla's Infinity - Infinity bookkeeping yields NaN here;
        // Solaris consumes finite absorption to keep authoritative state finite.
        (f32::INFINITY, state.absorption, 0.0)
    } else {
        let health_damage = (reduced.after_magic - state.absorption).max(0.0);
        let absorbed = reduced.after_magic - health_damage;
        let next_absorption = (state.absorption - absorbed).max(0.0);
        if !health_damage.is_finite() || !absorbed.is_finite() || !next_absorption.is_finite() {
            *output = DamageOutcome::Rejected(DamageRejection::NonFinite(
                DamageInputField::DerivedReduction,
            ));
            return;
        }
        (health_damage, absorbed, next_absorption)
    };

    let mut next = *state;
    next.last_hurt = raw_amount;
    if fresh_hurt {
        next.invulnerable_time = INVULNERABLE_DURATION_TICKS;
        next.hurt_time = HURT_DURATION_TICKS;
    }
    next.absorption = next_absorption;
    let remaining_health = next.health - health_damage;
    next.health = if remaining_health <= 0.0 {
        0.0
    } else {
        remaining_health
    };
    let health_lost = state.health - next.health;
    let lifecycle = if next.health <= 0.0 {
        next.lifecycle = LivingLifecycle::Dying;
        next.death_time = 0;
        LifecycleTransition::StartedDying
    } else {
        LifecycleTransition::None
    };

    *state = next;
    *output = DamageOutcome::Applied(DamageApplied {
        raw_amount,
        cooldown_damage,
        after_armor: reduced.after_armor,
        after_magic: reduced.after_magic,
        absorbed,
        health_damage,
        health_lost,
        fresh_hurt,
        mark_hurt: fresh_hurt && !flags.contains(DamageFlags::NO_IMPACT),
        knockback,
        lifecycle,
    });
}

fn evaluate_immunity(
    lifecycle: LivingLifecycle,
    flags: DamageFlags,
    immunity: TargetImmunityContext,
) -> Result<Option<DamageRejection>, UnsupportedRule> {
    if lifecycle == LivingLifecycle::Removed {
        return Ok(Some(DamageRejection::Removed));
    }

    if immunity.ordinary_invulnerable && !flags.contains(DamageFlags::BYPASSES_INVULNERABILITY) {
        match immunity.source_creative_player {
            CreativePlayerStatus::NotCreative => {
                return Ok(Some(DamageRejection::Invulnerable));
            }
            CreativePlayerStatus::Creative => {}
            CreativePlayerStatus::Unsupported => {
                return Err(UnsupportedRule::CreativePlayerStatus);
            }
        }
    }
    if flags.contains(DamageFlags::IS_FIRE) && immunity.fire_immune {
        return Ok(Some(DamageRejection::FireImmune));
    }
    if flags.contains(DamageFlags::IS_FALL) && immunity.fall_damage_immune {
        return Ok(Some(DamageRejection::FallDamageImmune));
    }

    match immunity.enchantment {
        EnchantmentImmunity::NotImmune => Ok(None),
        EnchantmentImmunity::Immune => Ok(Some(DamageRejection::EnchantmentImmune)),
        EnchantmentImmunity::UnsupportedSourceEvaluation => {
            Err(UnsupportedRule::EnchantmentImmunityEvaluation)
        }
    }
}

fn calculate_knockback(input: Option<KnockbackInput>) -> Result<KnockbackOutcome, DamageRejection> {
    let Some(input) = input else {
        return Ok(KnockbackOutcome::RandomDirectionRequired);
    };
    if !input.direction_x.is_finite() || !input.direction_z.is_finite() {
        return Err(DamageRejection::NonFinite(
            DamageInputField::KnockbackDirection,
        ));
    }
    if !input.velocity_x.is_finite()
        || !input.velocity_y.is_finite()
        || !input.velocity_z.is_finite()
    {
        return Err(DamageRejection::NonFinite(
            DamageInputField::KnockbackVelocity,
        ));
    }
    if !input.resistance.is_finite() {
        return Err(DamageRejection::NonFinite(
            DamageInputField::KnockbackResistance,
        ));
    }
    if !(0.0..=1.0).contains(&input.resistance) {
        return Err(DamageRejection::OutOfRange(
            DamageInputField::KnockbackResistance,
        ));
    }

    let power = VANILLA_BASE_KNOCKBACK * (1.0 - input.resistance);
    if power <= 0.0 {
        return Ok(KnockbackOutcome::None);
    }
    let length_squared =
        input.direction_x * input.direction_x + input.direction_z * input.direction_z;
    if !length_squared.is_finite() {
        return Err(DamageRejection::NonFinite(
            DamageInputField::DerivedKnockback,
        ));
    }
    if length_squared < VANILLA_RANDOM_KNOCKBACK_DIRECTION_SQUARED {
        return Ok(KnockbackOutcome::RandomDirectionRequired);
    }

    let length = length_squared.sqrt();
    let vector_x = input.direction_x / length * power;
    let vector_z = input.direction_z / length * power;
    let velocity = KnockbackVelocity {
        x: input.velocity_x / 2.0 - vector_x,
        y: if input.on_ground {
            (input.velocity_y / 2.0 + power).min(0.4)
        } else {
            input.velocity_y
        },
        z: input.velocity_z / 2.0 - vector_z,
    };
    if !velocity.x.is_finite() || !velocity.y.is_finite() || !velocity.z.is_finite() {
        return Err(DamageRejection::NonFinite(
            DamageInputField::DerivedKnockback,
        ));
    }
    Ok(KnockbackOutcome::Velocity(velocity))
}

fn map_reduction_error(error: ReductionError) -> DamageRejection {
    match error {
        ReductionError::NonFinite(field) => DamageRejection::NonFinite(map_reduction_field(field)),
        ReductionError::OutOfRange(field) => {
            DamageRejection::OutOfRange(map_reduction_field(field))
        }
        ReductionError::Unsupported(UnsupportedReduction::WeaponArmorEffectiveness) => {
            DamageRejection::Unsupported(UnsupportedRule::WeaponArmorEffectiveness)
        }
        ReductionError::Unsupported(UnsupportedReduction::EnchantmentSourceEvaluation) => {
            DamageRejection::Unsupported(UnsupportedRule::EnchantmentSourceEvaluation)
        }
    }
}

const fn map_reduction_field(field: ReductionField) -> DamageInputField {
    match field {
        ReductionField::Armor => DamageInputField::Armor,
        ReductionField::ArmorToughness => DamageInputField::ArmorToughness,
        ReductionField::EnchantmentProtection => DamageInputField::EnchantmentProtection,
        ReductionField::Derived => DamageInputField::DerivedReduction,
    }
}
