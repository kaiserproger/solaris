/// The narrow set of vanilla damage sources owned by this kernel slice.
///
/// `Fire` is specifically `minecraft:in_fire`, `Melee` is
/// `minecraft:mob_attack`, and `Projectile` is `minecraft:arrow`. Registry
/// adapters must use [`DamageSource::with_flags`] for another already-resolved
/// damage type instead of assuming category-wide tag equivalence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageSourceKind {
    Fall,
    Fire,
    Drowning,
    Void,
    Generic,
    Melee,
    Projectile,
    Unsupported,
}

/// Resolved 26.1.2 damage-type tags used by the living lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DamageFlags(u16);

impl DamageFlags {
    pub const NONE: Self = Self(0);
    pub const BYPASSES_ARMOR: Self = Self(1 << 0);
    pub const BYPASSES_INVULNERABILITY: Self = Self(1 << 1);
    pub const BYPASSES_COOLDOWN: Self = Self(1 << 2);
    pub const BYPASSES_EFFECTS: Self = Self(1 << 3);
    pub const BYPASSES_RESISTANCE: Self = Self(1 << 4);
    pub const BYPASSES_ENCHANTMENTS: Self = Self(1 << 5);
    pub const IS_FIRE: Self = Self(1 << 6);
    pub const IS_PROJECTILE: Self = Self(1 << 7);
    pub const IS_FALL: Self = Self(1 << 8);
    pub const IS_DROWNING: Self = Self(1 << 9);
    pub const NO_IMPACT: Self = Self(1 << 10);
    pub const NO_KNOCKBACK: Self = Self(1 << 11);

    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A typed source plus its already-resolved registry tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageSource {
    kind: DamageSourceKind,
    flags: DamageFlags,
}

impl DamageSource {
    /// Builds the exact representative source documented on
    /// [`DamageSourceKind`].
    #[must_use]
    pub const fn vanilla(kind: DamageSourceKind) -> Self {
        let flags = match kind {
            DamageSourceKind::Fall => DamageFlags::BYPASSES_ARMOR
                .union(DamageFlags::IS_FALL)
                .union(DamageFlags::NO_KNOCKBACK),
            DamageSourceKind::Fire => DamageFlags::IS_FIRE.union(DamageFlags::NO_KNOCKBACK),
            DamageSourceKind::Drowning => DamageFlags::BYPASSES_ARMOR
                .union(DamageFlags::IS_DROWNING)
                .union(DamageFlags::NO_IMPACT)
                .union(DamageFlags::NO_KNOCKBACK),
            DamageSourceKind::Void => DamageFlags::BYPASSES_ARMOR
                .union(DamageFlags::BYPASSES_INVULNERABILITY)
                .union(DamageFlags::BYPASSES_RESISTANCE)
                .union(DamageFlags::NO_KNOCKBACK),
            DamageSourceKind::Generic => {
                DamageFlags::BYPASSES_ARMOR.union(DamageFlags::NO_KNOCKBACK)
            }
            DamageSourceKind::Projectile => DamageFlags::IS_PROJECTILE,
            DamageSourceKind::Melee | DamageSourceKind::Unsupported => DamageFlags::NONE,
        };
        Self { kind, flags }
    }

    /// Accepts tags resolved by the vanilla-data registry adapter.
    #[must_use]
    pub const fn with_flags(kind: DamageSourceKind, flags: DamageFlags) -> Self {
        Self { kind, flags }
    }

    #[must_use]
    pub const fn kind(self) -> DamageSourceKind {
        self.kind
    }

    #[must_use]
    pub const fn flags(self) -> DamageFlags {
        self.flags
    }
}
