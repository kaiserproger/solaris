use mc_entity::Vec3;
use mc_protocol::packets::play::ItemStack;

const PLAYER_HURT_RESISTANCE_TICKS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum PlayerDamageKind {
    MobAttack,
    PlayerAttack,
    Projectile,
    Fall,
    Campfire,
    Starvation,
    Generic,
    GenericKill,
    Explosion,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct PlayerDamageRequest {
    pub(in crate::play) kind: PlayerDamageKind,
    pub(in crate::play) amount: f32,
    pub(in crate::play) source_origin: Option<Vec3>,
}

impl PlayerDamageKind {
    pub(in crate::play) const fn uses_armor(self) -> bool {
        matches!(
            self,
            Self::MobAttack
                | Self::PlayerAttack
                | Self::Projectile
                | Self::Campfire
                | Self::Explosion
        )
    }

    pub(in crate::play) const fn uses_protection(self) -> bool {
        !matches!(self, Self::GenericKill)
    }

    pub(in crate::play) const fn damages_armor(self) -> bool {
        self.uses_armor()
    }

    pub(in crate::play) const fn can_be_blocked_by_shield(self) -> bool {
        matches!(
            self,
            Self::MobAttack | Self::PlayerAttack | Self::Projectile
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) enum PlayerHurtResolution {
    Rejected,
    Apply { amount: f32, fresh_hurt: bool },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(in crate::play) struct PlayerHurtResistance {
    last_full_hurt_tick: Option<u64>,
    last_hurt: f32,
}

impl PlayerHurtResistance {
    pub(in crate::play) fn preview(
        self,
        current_tick: u64,
        amount: f32,
    ) -> (PlayerHurtResolution, Self) {
        let mut next = self;
        let resolution = next.resolve(current_tick, amount);
        (resolution, next)
    }

    pub(in crate::play) fn resolve(
        &mut self,
        current_tick: u64,
        amount: f32,
    ) -> PlayerHurtResolution {
        if !amount.is_finite() || amount <= 0.0 {
            return PlayerHurtResolution::Rejected;
        }
        if let Some(last_tick) = self.last_full_hurt_tick
            && current_tick.saturating_sub(last_tick) < PLAYER_HURT_RESISTANCE_TICKS
        {
            if amount <= self.last_hurt {
                return PlayerHurtResolution::Rejected;
            }
            let difference = amount - self.last_hurt;
            self.last_hurt = amount;
            return PlayerHurtResolution::Apply {
                amount: difference,
                fresh_hurt: false,
            };
        }
        self.last_full_hurt_tick = Some(current_tick);
        self.last_hurt = amount;
        PlayerHurtResolution::Apply {
            amount,
            fresh_hurt: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::play) struct ActiveShield {
    pub(in crate::play) started_tick: u64,
    pub(in crate::play) slot: usize,
    pub(in crate::play) expected_stack: ItemStack,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct MeleeKnockback {
    pub(in crate::play) x: f64,
    pub(in crate::play) y: f64,
    pub(in crate::play) z: f64,
}

pub(in crate::play) fn melee_knockback(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
) -> Option<MeleeKnockback> {
    knockback_with_strength(
        target_x,
        target_z,
        target_on_ground,
        source,
        0.400_000_005_960_464_5,
    )
}

pub(in crate::play) fn shield_block_knockback(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
) -> Option<MeleeKnockback> {
    knockback_with_strength(target_x, target_z, target_on_ground, source, 0.5)
}

fn knockback_with_strength(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
    strength: f64,
) -> Option<MeleeKnockback> {
    let direction_x = source.x - target_x;
    let direction_z = source.z - target_z;
    let length_squared = direction_x * direction_x + direction_z * direction_z;
    if length_squared < 9.999_999_747_378_752e-6 {
        return None;
    }
    let scale = strength / length_squared.sqrt();
    Some(MeleeKnockback {
        x: -direction_x * scale,
        y: if target_on_ground { 0.4 } else { 0.0 },
        z: -direction_z * scale,
    })
}
