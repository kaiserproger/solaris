use super::source::DamageFlags;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ArmorEffectiveness {
    #[default]
    Unmodified,
    /// `EnchantmentHelper.modifyArmorEffectiveness` needs the weapon, victim,
    /// source, registry, and level. This scalar kernel does not guess it.
    UnsupportedWeaponEnchantment,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum EnchantmentProtection {
    #[default]
    None,
    /// Source-aware aggregate returned by vanilla's
    /// `EnchantmentHelper.getDamageProtection`.
    OracleAggregate(f32),
    /// The caller has equipment but has not evaluated its source-aware effects.
    UnsupportedSourceEvaluation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResistanceEffect {
    pub amplifier: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReductionContext {
    /// Floored `LivingEntity.getArmorValue()` aggregate (`0..=30`).
    pub armor: u8,
    pub armor_toughness: f32,
    pub armor_effectiveness: ArmorEffectiveness,
    pub resistance: Option<ResistanceEffect>,
    pub enchantment: EnchantmentProtection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReductionField {
    Armor,
    ArmorToughness,
    EnchantmentProtection,
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnsupportedReduction {
    WeaponArmorEffectiveness,
    EnchantmentSourceEvaluation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReductionError {
    NonFinite(ReductionField),
    OutOfRange(ReductionField),
    Unsupported(UnsupportedReduction),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReducedDamage {
    pub after_armor: f32,
    pub after_magic: f32,
}

pub(crate) fn reduce_damage(
    damage: f32,
    flags: DamageFlags,
    context: ReductionContext,
) -> Result<ReducedDamage, ReductionError> {
    let after_armor = if flags.contains(DamageFlags::BYPASSES_ARMOR) {
        damage
    } else {
        if !context.armor_toughness.is_finite() {
            return Err(ReductionError::NonFinite(ReductionField::ArmorToughness));
        }
        if context.armor > 30 {
            return Err(ReductionError::OutOfRange(ReductionField::Armor));
        }
        if !(0.0..=20.0).contains(&context.armor_toughness) {
            return Err(ReductionError::OutOfRange(ReductionField::ArmorToughness));
        }
        if context.armor_effectiveness == ArmorEffectiveness::UnsupportedWeaponEnchantment {
            return Err(ReductionError::Unsupported(
                UnsupportedReduction::WeaponArmorEffectiveness,
            ));
        }

        let armor = f32::from(context.armor);
        let toughness = 2.0 + context.armor_toughness / 4.0;
        let minimum_armor = armor * 0.2;
        let real_armor = java_clamp(armor - damage / toughness, minimum_armor, 20.0);
        let armor_fraction = real_armor / 25.0;
        damage * (1.0 - armor_fraction)
    };
    valid_derived(after_armor)?;

    let after_magic = reduce_magic(after_armor, flags, context)?;
    valid_derived(after_magic)?;
    Ok(ReducedDamage {
        after_armor,
        after_magic,
    })
}

fn reduce_magic(
    mut damage: f32,
    flags: DamageFlags,
    context: ReductionContext,
) -> Result<f32, ReductionError> {
    if flags.contains(DamageFlags::BYPASSES_EFFECTS) {
        return Ok(damage);
    }

    if let Some(resistance) = context.resistance
        && !flags.contains(DamageFlags::BYPASSES_RESISTANCE)
    {
        let absorb_value = (i32::from(resistance.amplifier) + 1) * 5;
        let remaining = 25 - absorb_value;
        damage = ((damage * remaining as f32) / 25.0).max(0.0);
        valid_derived(damage)?;
    }

    if damage <= 0.0 {
        return Ok(0.0);
    }
    if flags.contains(DamageFlags::BYPASSES_ENCHANTMENTS) {
        return Ok(damage);
    }

    match context.enchantment {
        EnchantmentProtection::None => Ok(damage),
        EnchantmentProtection::OracleAggregate(points) => {
            if !points.is_finite() {
                return Err(ReductionError::NonFinite(
                    ReductionField::EnchantmentProtection,
                ));
            }
            let real_armor = java_clamp(points, 0.0, 20.0);
            Ok(damage * (1.0 - real_armor / 25.0))
        }
        EnchantmentProtection::UnsupportedSourceEvaluation => Err(ReductionError::Unsupported(
            UnsupportedReduction::EnchantmentSourceEvaluation,
        )),
    }
}

fn valid_derived(value: f32) -> Result<(), ReductionError> {
    if value.is_finite() || (value.is_infinite() && value.is_sign_positive()) {
        Ok(())
    } else {
        Err(ReductionError::NonFinite(ReductionField::Derived))
    }
}

fn java_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min { min } else { value.min(max) }
}
