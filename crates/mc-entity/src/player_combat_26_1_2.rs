//! Protocol-neutral player-combat math for Java Edition 26.1.2.

use crate::Vec3;

const PLAYER_HURT_RESISTANCE_TICKS: u64 = 10;
pub const SHIELD_ACTIVATION_DELAY_TICKS: u64 = 5;
pub const SHIELD_FRONT_ARC_DOT_MIN: f64 = 0.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HurtResolution {
    Rejected,
    Apply { amount: f32, fresh_hurt: bool },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HurtResistance {
    last_full_hurt_tick: Option<u64>,
    last_hurt: f32,
}

impl HurtResistance {
    #[must_use]
    pub fn preview(self, current_tick: u64, amount: f32) -> (HurtResolution, Self) {
        let mut next = self;
        let resolution = next.resolve(current_tick, amount);
        (resolution, next)
    }

    pub fn resolve(&mut self, current_tick: u64, amount: f32) -> HurtResolution {
        if !amount.is_finite() || amount <= 0.0 {
            return HurtResolution::Rejected;
        }
        if let Some(last_tick) = self.last_full_hurt_tick
            && current_tick.saturating_sub(last_tick) < PLAYER_HURT_RESISTANCE_TICKS
        {
            if amount <= self.last_hurt {
                return HurtResolution::Rejected;
            }
            let difference = amount - self.last_hurt;
            self.last_hurt = amount;
            return HurtResolution::Apply {
                amount: difference,
                fresh_hurt: false,
            };
        }
        self.last_full_hurt_tick = Some(current_tick);
        self.last_hurt = amount;
        HurtResolution::Apply {
            amount,
            fresh_hurt: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knockback {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[must_use]
pub fn melee_knockback(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
) -> Option<Knockback> {
    knockback_with_strength(
        target_x,
        target_z,
        target_on_ground,
        source,
        0.400_000_005_960_464_5,
    )
}

#[must_use]
pub fn shield_block_knockback(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
) -> Option<Knockback> {
    knockback_with_strength(target_x, target_z, target_on_ground, source, 0.5)
}

#[must_use]
pub fn horizontal_look_direction(yaw: f32) -> Vec3 {
    let yaw = f64::from(yaw).to_radians();
    Vec3::new(-yaw.sin(), 0.0, yaw.cos())
}

#[must_use]
pub fn shield_blocks_damage_since(
    player_position: Vec3,
    player_yaw: f32,
    source_origin: Option<Vec3>,
    current_tick: u64,
    started_tick: u64,
) -> bool {
    if current_tick.saturating_sub(started_tick) < SHIELD_ACTIVATION_DELAY_TICKS {
        return false;
    }
    let Some(source_origin) = source_origin else {
        return false;
    };
    let incoming = Vec3::new(
        source_origin.x - player_position.x,
        0.0,
        source_origin.z - player_position.z,
    );
    let incoming_len = (incoming.x * incoming.x + incoming.z * incoming.z).sqrt();
    if incoming_len <= f64::EPSILON {
        return false;
    }
    let look = horizontal_look_direction(player_yaw);
    let dot = (look.x * incoming.x + look.z * incoming.z) / incoming_len;
    dot >= SHIELD_FRONT_ARC_DOT_MIN
}

#[must_use]
pub fn shield_durability_damage(blocked_damage: f32) -> i32 {
    if blocked_damage < 3.0 {
        return 0;
    }
    if !blocked_damage.is_finite() {
        return i32::MAX;
    }
    let scaled = blocked_damage.max(0.0).floor();
    if scaled >= (i32::MAX - 1) as f32 {
        i32::MAX
    } else {
        (scaled as i32).saturating_add(1).max(1)
    }
}

#[must_use]
pub fn shield_disable_ticks(seconds: f32, cooldown_scale: f32) -> Option<u64> {
    if !seconds.is_finite()
        || !cooldown_scale.is_finite()
        || seconds <= 0.0
        || cooldown_scale <= 0.0
    {
        return None;
    }
    let ticks = (seconds * cooldown_scale * 20.0).round();
    (ticks > 0.0 && ticks <= u64::MAX as f32).then_some(ticks as u64)
}

fn knockback_with_strength(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
    strength: f64,
) -> Option<Knockback> {
    let direction_x = source.x - target_x;
    let direction_z = source.z - target_z;
    let length_squared = direction_x * direction_x + direction_z * direction_z;
    if length_squared < 9.999_999_747_378_752e-6 {
        return None;
    }
    let scale = strength / length_squared.sqrt();
    Some(Knockback {
        x: -direction_x * scale,
        y: if target_on_ground { 0.4 } else { 0.0 },
        z: -direction_z * scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hurt_resistance_applies_only_incremental_damage_inside_the_resistance_window() {
        let mut resistance = HurtResistance::default();
        assert_eq!(
            resistance.resolve(10, 4.0),
            HurtResolution::Apply {
                amount: 4.0,
                fresh_hurt: true,
            }
        );
        assert_eq!(resistance.resolve(12, 3.0), HurtResolution::Rejected);
        assert_eq!(
            resistance.resolve(12, 6.5),
            HurtResolution::Apply {
                amount: 2.5,
                fresh_hurt: false,
            }
        );
        assert_eq!(
            resistance.resolve(20, 2.0),
            HurtResolution::Apply {
                amount: 2.0,
                fresh_hurt: true,
            }
        );
        assert_eq!(resistance.resolve(21, f32::NAN), HurtResolution::Rejected);
        assert_eq!(resistance.resolve(21, 0.0), HurtResolution::Rejected);
    }

    #[test]
    fn knockback_rejects_near_zero_direction_and_preserves_ground_vertical_impulse() {
        assert_eq!(
            melee_knockback(1.0, 2.0, true, Vec3::new(1.0, 9.0, 2.0)),
            None
        );
        let knockback = melee_knockback(0.0, 0.0, true, Vec3::new(1.0, 0.0, 0.0)).unwrap();
        assert!((knockback.x + 0.400_000_005_960_464_5).abs() < 1e-12);
        assert_eq!(knockback.y, 0.4);
        assert!(knockback.z.abs() < f64::EPSILON);
    }

    #[test]
    fn shield_front_arc_requires_activation_delay_and_a_front_source() {
        let player = Vec3::new(0.0, 64.0, 0.0);
        let front = Some(Vec3::new(0.0, 64.0, 4.0));
        let back = Some(Vec3::new(0.0, 64.0, -4.0));
        assert!(!shield_blocks_damage_since(player, 0.0, front, 4, 0));
        assert!(shield_blocks_damage_since(player, 0.0, front, 5, 0));
        assert!(!shield_blocks_damage_since(player, 0.0, back, 5, 0));
        assert!(!shield_blocks_damage_since(player, 0.0, None, 5, 0));
        assert!(!shield_blocks_damage_since(player, 0.0, Some(player), 5, 0,));
    }

    #[test]
    fn shield_numeric_rules_fail_closed_and_match_vanilla_thresholds() {
        assert_eq!(shield_durability_damage(2.99), 0);
        assert_eq!(shield_durability_damage(3.0), 4);
        assert_eq!(shield_durability_damage(f32::INFINITY), i32::MAX);
        assert_eq!(shield_disable_ticks(5.0, 1.0), Some(100));
        assert_eq!(shield_disable_ticks(f32::NAN, 1.0), None);
        assert_eq!(shield_disable_ticks(5.0, 0.0), None);
    }

    #[test]
    fn horizontal_look_direction_matches_minecraft_yaw() {
        let forward = horizontal_look_direction(0.0);
        assert!(forward.x.abs() < f64::EPSILON);
        assert!((forward.z - 1.0).abs() < f64::EPSILON);
        let east = horizontal_look_direction(-90.0);
        assert!((east.x - 1.0).abs() < 1e-12);
        assert!(east.z.abs() < 1e-12);
    }
}
