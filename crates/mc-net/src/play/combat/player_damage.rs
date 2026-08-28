use mc_data::item_stack::ItemStack;
use mc_entity::Vec3;
use mc_entity::living_26_1_2::{DamageFlags, DamageSource, DamageSourceKind};

pub(in crate::play) use mc_entity::player_combat_26_1_2::{
    HurtResistance as PlayerHurtResistance, HurtResolution as PlayerHurtResolution,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::play) enum PlayerDamageKind {
    MobAttack,
    PlayerAttack,
    Projectile,
    Fireball,
    LargeFireball,
    ShulkerBullet,
    WindCharge,
    SonicBoom,
    IndirectMagic,
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
            Self::Projectile | Self::ShulkerBullet | Self::WindCharge => {
                DamageSource::vanilla(DamageSourceKind::Projectile)
            }
            Self::Fireball | Self::LargeFireball => DamageSource::with_flags(
                DamageSourceKind::Projectile,
                DamageFlags::IS_PROJECTILE.union(DamageFlags::IS_FIRE),
            ),
            Self::SonicBoom => DamageSource::with_flags(
                DamageSourceKind::Generic,
                DamageFlags::BYPASSES_ARMOR.union(DamageFlags::BYPASSES_ENCHANTMENTS),
            ),
            Self::IndirectMagic => DamageSource::vanilla(DamageSourceKind::IndirectMagic),
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
            Self::MobAttack
                | Self::PlayerAttack
                | Self::Projectile
                | Self::Fireball
                | Self::LargeFireball
                | Self::ShulkerBullet
                | Self::WindCharge
        )
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
    mc_entity::player_combat_26_1_2::melee_knockback(target_x, target_z, target_on_ground, source)
        .map(|knockback| MeleeKnockback {
            x: knockback.x,
            y: knockback.y,
            z: knockback.z,
        })
}

pub(in crate::play) fn shield_block_knockback(
    target_x: f64,
    target_z: f64,
    target_on_ground: bool,
    source: Vec3,
) -> Option<MeleeKnockback> {
    mc_entity::player_combat_26_1_2::shield_block_knockback(
        target_x,
        target_z,
        target_on_ground,
        source,
    )
    .map(|knockback| MeleeKnockback {
        x: knockback.x,
        y: knockback.y,
        z: knockback.z,
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
            (
                PlayerDamageKind::IndirectMagic,
                DamageSourceKind::IndirectMagic,
                false,
                false,
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
