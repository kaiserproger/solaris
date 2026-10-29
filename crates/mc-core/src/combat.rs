//! Player melee (ADR 0013): vanilla `Player.attack` damage, critical hits,
//! knockback, sweep and weapon wear. Item behaviour comes from item data
//! (attribute modifiers, weapon component, `#minecraft:swords`), never from
//! item names, so plugin items behave like vanilla ones.

use mc_data::ItemStack;

use crate::inventory::ItemRules;

/// Player base `attack_speed` attribute.
const BASE_ATTACK_SPEED: f32 = 4.0;
/// Player base `attack_damage` attribute.
const BASE_ATTACK_DAMAGE: f32 = 1.0;
/// Knockback every melee hit applies (`LivingEntity.hurt`).
pub const HIT_KNOCKBACK: f64 = 0.4;
/// Extra knockback of a sprinting full-strength hit, before the 0.5 scale.
const SPRINT_KNOCKBACK: f32 = 1.0;

/// Attacker state at the moment of the swing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Swing {
    /// Ticks since the attacker's previous swing (or item switch).
    pub ticks_since_reset: u32,
    pub falling: bool,
    pub on_ground: bool,
    pub sprinting: bool,
    /// In water, on a ladder, blind or riding: no critical hit.
    pub crit_blocked: bool,
    pub melee_bonus: f32,
    pub attack_speed_multiplier: f64,
}

/// What one melee hit does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeleeHit {
    /// Damage before the target's armor and enchantment reduction.
    pub damage: f32,
    pub critical: bool,
    /// Extra knockback strength (sprint and Knockback enchantment), applied
    /// on top of [`HIT_KNOCKBACK`] along the attacker's look direction.
    pub bonus_knockback: f64,
    /// Sweep damage to other targets near the hit, when this hit sweeps.
    pub sweep_damage: Option<f32>,
    /// Durability the held item loses.
    pub weapon_wear: u32,
    /// Strength scale 0..1; a full swing is 1.
    pub strength: f32,
}

/// Item data stores attribute amounts as `f32`; vanilla attributes are
/// doubles written with a few decimals (`-2.4`). Recover the decimal value so
/// a full recharge lands exactly on vanilla's tick.
fn attribute(amount: f32) -> f64 {
    (f64::from(amount) * 1.0e6).round() / 1.0e6
}

#[must_use]
pub fn attack_speed(rules: &ItemRules<'_>, held: &ItemStack) -> f64 {
    let modifier = facts(rules, held).and_then(|facts| facts.attack_speed_modifier);
    (f64::from(BASE_ATTACK_SPEED) + modifier.map_or(0.0, attribute)).max(0.0)
}

/// Vanilla `getAttackStrengthScale(0.5)`. Zero attack speed never recharges.
#[must_use]
pub fn attack_strength(rules: &ItemRules<'_>, held: &ItemStack, ticks_since_reset: u32) -> f32 {
    let speed = attack_speed(rules, held);
    ((f64::from(ticks_since_reset) + 0.5) * speed / 20.0).clamp(0.0, 1.0) as f32
}

/// Vanilla `Player.attack` against a living target.
///
/// `held` is the live held stack (enchantments, weapon component, wear);
/// `attribute_held` is the MC-28289 snapshot the server held as of the
/// player's last tick update, used ONLY for the `attack_damage` /
/// `attack_speed` attribute values. Pass the same stack for both when no
/// same-tick swap is in play.
#[must_use]
pub fn melee_hit(rules: &ItemRules<'_>, held: &ItemStack, swing: Swing) -> MeleeHit {
    melee_hit_with_attributes(rules, held, held, swing)
}

/// [`melee_hit`] with split attribute/live stacks (MC-28289).
#[must_use]
pub fn melee_hit_with_attributes(
    rules: &ItemRules<'_>,
    held: &ItemStack,
    attribute_held: &ItemStack,
    swing: Swing,
) -> MeleeHit {
    melee_hit_with_effective_attributes(rules, held, attribute_held, swing, None)
}

pub(crate) fn melee_hit_with_effective_attributes(
    rules: &ItemRules<'_>,
    held: &ItemStack,
    attribute_held: &ItemStack,
    swing: Swing,
    attributes: Option<(f64, f64)>,
) -> MeleeHit {
    let item = facts(rules, held);
    let attribute_item = facts(rules, attribute_held);
    let base = attributes
        .map_or_else(
            || {
                f64::from(BASE_ATTACK_DAMAGE)
                    + attribute_item
                        .and_then(|facts| facts.attack_damage_modifier)
                        .map_or(0.0, attribute)
                    + f64::from(swing.melee_bonus)
            },
            |(damage, _)| damage,
        )
        .max(0.0) as f32;
    let speed = attributes.map_or_else(
        || attack_speed(rules, attribute_held) * swing.attack_speed_multiplier,
        |(_, speed)| speed,
    );
    let strength =
        ((f64::from(swing.ticks_since_reset) + 0.5) * speed / 20.0).clamp(0.0, 1.0) as f32;
    let full = strength > 0.9;
    let enchanted = enchantment_bonus(held) * strength;
    let mut damage = base * (0.2 + strength * strength * 0.8);
    let sprint_hit = swing.sprinting && full;
    let critical =
        full && swing.falling && !swing.on_ground && !swing.crit_blocked && !swing.sprinting;
    if critical {
        damage *= 1.5;
    }
    let mut bonus_knockback = f64::from(level(held, "minecraft:knockback"));
    if sprint_hit {
        bonus_knockback += f64::from(SPRINT_KNOCKBACK);
    }
    let sweeps = full && !critical && !sprint_hit && swing.on_ground && is_sword(rules, held);
    let sweep_damage = sweeps.then(|| {
        let level = level(held, "minecraft:sweeping_edge") as f32;
        (1.0 + level / (level + 1.0) * damage + enchantment_bonus(held)) * strength
    });
    let weapon_wear = if held.is_empty() {
        0
    } else {
        item.and_then(|facts| facts.weapon_damage_per_attack)
            .unwrap_or(0)
    };
    MeleeHit {
        damage: damage + enchanted,
        critical,
        bonus_knockback: bonus_knockback * 0.5,
        sweep_damage,
        weapon_wear,
        strength,
    }
}

/// Fall and enchantment context retained until hit admission.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaceAttack {
    pub fall_distance: f64,
    pub breach: u32,
    pub wind_burst: u32,
}

/// MaceItem.canSmashAttack: strictly above 1.5 blocks, never while gliding.
pub(crate) fn mace_attack(
    rules: &ItemRules<'_>,
    held: &ItemStack,
    fall_distance: f64,
    gliding: bool,
) -> Option<MaceAttack> {
    if held.is_empty() || rules.items.name_of(held.item_id)?.as_str() != "minecraft:mace" {
        return None;
    }
    Some(MaceAttack {
        fall_distance: if fall_distance > 1.5 && !gliding {
            fall_distance
        } else {
            0.0
        },
        breach: crate::enchantments::level(held, "breach"),
        wind_burst: crate::enchantments::level(held, "wind_burst"),
    })
}

pub(crate) fn smash_damage(held: &ItemStack, attack: MaceAttack) -> f32 {
    let distance = attack.fall_distance;
    let base = if distance <= 3.0 {
        distance * 4.0
    } else if distance <= 8.0 {
        12.0 + (distance - 3.0) * 2.0
    } else {
        22.0 + distance - 8.0
    };
    base as f32 + crate::enchantments::density_damage(held, distance)
}

/// Vanilla `LivingEntity.knockback`: new velocity of a target pushed with
/// `strength`. `(dir_x, dir_z)` points from the target toward the attacker
/// (vanilla passes `(sin yaw, -cos yaw)` of the attacker); the push goes the
/// other way.
#[must_use]
pub fn knockback_velocity(
    velocity: [f64; 3],
    on_ground: bool,
    strength: f64,
    dir_x: f64,
    dir_z: f64,
    knockback_resistance: f64,
) -> [f64; 3] {
    let strength = strength * (1.0 - knockback_resistance.clamp(0.0, 1.0));
    let length = (dir_x * dir_x + dir_z * dir_z).sqrt();
    if strength <= 0.0 || length < 1.0e-5 {
        return velocity;
    }
    let push_x = dir_x / length * strength;
    let push_z = dir_z / length * strength;
    [
        velocity[0] / 2.0 - push_x,
        if on_ground {
            (velocity[1] / 2.0 + strength).min(0.4)
        } else {
            velocity[1]
        },
        velocity[2] / 2.0 - push_z,
    ]
}

fn facts<'a>(
    rules: &ItemRules<'a>,
    held: &ItemStack,
) -> Option<&'a mc_data::item_components::ItemFacts> {
    (!held.is_empty())
        .then(|| rules.facts.facts_for_stack(held, rules.items))
        .flatten()
}

fn level(stack: &ItemStack, id: &str) -> i32 {
    stack
        .enchantments
        .iter()
        .filter(|enchantment| enchantment.id.as_str() == id)
        .map(|enchantment| enchantment.level.max(0))
        .max()
        .unwrap_or(0)
}

/// Sharpness: 1 + 0.5 per level above the first.
fn enchantment_bonus(held: &ItemStack) -> f32 {
    let sharpness = level(held, "minecraft:sharpness");
    if sharpness <= 0 {
        0.0
    } else {
        1.0 + 0.5 * (sharpness - 1) as f32
    }
}

pub(crate) fn fire_aspect_ticks(held: &ItemStack) -> u32 {
    (level(held, "minecraft:fire_aspect").max(0) as u32).saturating_mul(80)
}

fn is_sword(rules: &ItemRules<'_>, held: &ItemStack) -> bool {
    !held.is_empty()
        && i32::try_from(held.item_id).is_ok_and(|raw| {
            rules
                .tags
                .contains_raw_id("minecraft:item", "minecraft:swords", raw)
        })
}

#[cfg(test)]
#[path = "combat_tests.rs"]
mod tests;
