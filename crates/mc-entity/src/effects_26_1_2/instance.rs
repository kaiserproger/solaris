use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct EffectId(u32);

impl EffectId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    Regeneration,
    Poison,
    Wither,
    Hunger,
    Saturation,
    InstantHealth,
    InstantDamage,
    /// Registry-resolved behavior that this narrow kernel does not guess.
    CallerOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectFlags {
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

impl Default for EffectFlags {
    fn default() -> Self {
        Self {
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }
}

/// One visible or hidden `MobEffectInstance` layer.
///
/// Amplifiers are clamped to Java's `0..=255` constructor range. Duration `-1`
/// is infinite; zero and every other negative value have no remaining duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectInstance {
    pub id: EffectId,
    pub kind: EffectKind,
    pub duration: i32,
    pub amplifier: u8,
    pub flags: EffectFlags,
}

impl EffectInstance {
    #[must_use]
    pub fn new(
        id: EffectId,
        kind: EffectKind,
        duration: i32,
        amplifier: i32,
        flags: EffectFlags,
    ) -> Self {
        Self {
            id,
            kind,
            duration,
            amplifier: amplifier.clamp(0, 255) as u8,
            flags,
        }
    }

    #[must_use]
    pub const fn is_infinite(self) -> bool {
        self.duration == -1
    }

    #[must_use]
    pub const fn has_remaining_duration(self) -> bool {
        self.is_infinite() || self.duration > 0
    }
}

pub(crate) fn is_shorter_duration_than(left: EffectInstance, right: EffectInstance) -> bool {
    !left.is_infinite() && (left.duration < right.duration || right.is_infinite())
}

pub(crate) fn decrement_duration(duration: i32) -> i32 {
    if duration == -1 || duration == 0 {
        duration
    } else {
        duration.wrapping_sub(1)
    }
}
