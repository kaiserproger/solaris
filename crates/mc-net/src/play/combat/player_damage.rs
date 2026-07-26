use mc_entity::Vec3;
use mc_entity::living_26_1_2::{DamageFlags, DamageSource, DamageSourceKind};
use mc_protocol::packets::play::ItemStack;

const PLAYER_HURT_RESISTANCE_TICKS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum PlayerDamageKind {
    MobAttack,
    PlayerAttack,
    Projectile,
    Fall,
    Campfire,
    Fire,
    Lava,
    Drowning,
    Suffocation,
    Starvation,
    Generic,
    GenericKill,
    Explosion,
    #[cfg(test)]
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct PlayerDamageRequest {
    pub(in crate::play) kind: PlayerDamageKind,
    pub(in crate::play) amount: f32,
    pub(in crate::play) source_origin: Option<Vec3>,
}

impl PlayerDamageKind {
    pub(in crate::play) const fn source(self) -> DamageSource {
        match self {
            Self::MobAttack | Self::PlayerAttack => DamageSource::vanilla(DamageSourceKind::Melee),
            Self::Projectile => DamageSource::vanilla(DamageSourceKind::Projectile),
            Self::Fall => DamageSource::vanilla(DamageSourceKind::Fall),
            Self::Campfire | Self::Fire => DamageSource::vanilla(DamageSourceKind::Fire),
            Self::Lava => DamageSource::vanilla(DamageSourceKind::Lava),
            Self::Drowning => DamageSource::vanilla(DamageSourceKind::Drowning),
            Self::Suffocation => DamageSource::vanilla(DamageSourceKind::Suffocation),
            Self::Starvation => DamageSource::vanilla(DamageSourceKind::Starvation),
            Self::Generic => DamageSource::vanilla(DamageSourceKind::Generic),
            Self::GenericKill => DamageSource::with_flags(
                DamageSourceKind::Generic,
                DamageFlags::BYPASSES_ARMOR
                    .union(DamageFlags::BYPASSES_INVULNERABILITY)
                    .union(DamageFlags::BYPASSES_RESISTANCE)
                    .union(DamageFlags::NO_KNOCKBACK),
            ),
            Self::Explosion => {
                DamageSource::with_flags(DamageSourceKind::Generic, DamageFlags::NO_KNOCKBACK)
            }
            #[cfg(test)]
            Self::Unsupported => DamageSource::vanilla(DamageSourceKind::Unsupported),
        }
    }

    pub(in crate::play) const fn is_supported(self) -> bool {
        #[cfg(test)]
        if matches!(self, Self::Unsupported) {
            return false;
        }
        true
    }

    pub(in crate::play) const fn uses_armor(self) -> bool {
        !self.source().flags().contains(DamageFlags::BYPASSES_ARMOR)
    }

    pub(in crate::play) const fn uses_protection(self) -> bool {
        !self
            .source()
            .flags()
            .contains(DamageFlags::BYPASSES_ENCHANTMENTS)
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

#[cfg(test)]
mod source_policy_tests {
    use super::{DamageFlags, DamageSourceKind, PlayerDamageKind};

    #[test]
    fn common_player_damage_sources_match_vanilla_flags_and_reductions() {
        for (kind, source_kind, uses_armor, shieldable) in [
            (
                PlayerDamageKind::MobAttack,
                DamageSourceKind::Melee,
                true,
                true,
            ),
            (
                PlayerDamageKind::PlayerAttack,
                DamageSourceKind::Melee,
                true,
                true,
            ),
            (
                PlayerDamageKind::Projectile,
                DamageSourceKind::Projectile,
                true,
                true,
            ),
            (PlayerDamageKind::Fall, DamageSourceKind::Fall, false, false),
            (
                PlayerDamageKind::Campfire,
                DamageSourceKind::Fire,
                true,
                false,
            ),
            (PlayerDamageKind::Fire, DamageSourceKind::Fire, true, false),
            (PlayerDamageKind::Lava, DamageSourceKind::Lava, true, false),
            (
                PlayerDamageKind::Drowning,
                DamageSourceKind::Drowning,
                false,
                false,
            ),
            (
                PlayerDamageKind::Suffocation,
                DamageSourceKind::Suffocation,
                false,
                false,
            ),
            (
                PlayerDamageKind::Starvation,
                DamageSourceKind::Starvation,
                false,
                false,
            ),
            (
                PlayerDamageKind::Generic,
                DamageSourceKind::Generic,
                false,
                false,
            ),
            (
                PlayerDamageKind::GenericKill,
                DamageSourceKind::Generic,
                false,
                false,
            ),
            (
                PlayerDamageKind::Explosion,
                DamageSourceKind::Generic,
                true,
                false,
            ),
        ] {
            assert_eq!(kind.source().kind(), source_kind, "{kind:?}");
            assert_eq!(kind.uses_armor(), uses_armor, "{kind:?}");
            assert_eq!(kind.damages_armor(), uses_armor, "{kind:?}");
            assert_eq!(kind.can_be_blocked_by_shield(), shieldable, "{kind:?}");
            assert!(kind.uses_protection(), "{kind:?}");
            assert!(kind.is_supported(), "{kind:?}");
        }

        assert!(
            PlayerDamageKind::Starvation
                .source()
                .flags()
                .contains(DamageFlags::BYPASSES_EFFECTS)
        );
        assert!(
            PlayerDamageKind::GenericKill
                .source()
                .flags()
                .contains(DamageFlags::BYPASSES_INVULNERABILITY)
        );
        assert!(!PlayerDamageKind::Unsupported.is_supported());
        assert_eq!(
            PlayerDamageKind::Unsupported.source().kind(),
            DamageSourceKind::Unsupported
        );
    }
}
