//! Verified Minecraft Java 26.1.2 living-entity damage and lifecycle policy.
//!
//! This module is deliberately not a store, ECS system, or packet publisher.
//! `EntityStore::damage` and the ECS combat stage remain the production mutation
//! owners. A later adapter can copy the scalar state into this kernel,
//! apply one transition, and publish the returned deltas without cloning an
//! `EntitySnapshot`.
//!
//! The formulas here stop at oracle-provided armor/enchantment aggregates.
//! Equipment lookup, source-aware enchantment evaluation, weapon enchantment
//! armor modifiers, blocking, totems, attribution, loot, and publication stay
//! outside this allocation-free boundary.
//! Nonfinite caller input is rejected atomically. A finite input that overflows
//! verified Java float reduction arithmetic to positive infinity still follows
//! the lethal mutation path. On that pathological branch, vanilla contaminates
//! absorption bookkeeping with `NaN` through `Infinity - Infinity`; Solaris
//! deliberately consumes finite absorption to zero to preserve its finite-state
//! invariant. No exact absorption-state parity is claimed for that branch.

mod damage;
mod reduction;
mod source;
mod state;

pub use damage::{
    CreativePlayerStatus, DamageApplied, DamageContext, DamageEvent, DamageInputField,
    DamageOutcome, DamageRejection, EnchantmentImmunity, KnockbackInput, KnockbackOutcome,
    KnockbackVelocity, LifecycleTransition, TargetImmunityContext, UnsupportedRule,
    VANILLA_RANDOM_KNOCKBACK_DIRECTION_SQUARED, apply_damage,
};
pub use reduction::{
    ArmorEffectiveness, EnchantmentProtection, ReductionContext, ResistanceEffect,
};
pub use source::{DamageFlags, DamageSource, DamageSourceKind};
pub use state::{
    InvulnerabilityClock, LivingLifecycle, LivingState, StateError, TickApplied, TickOutcome,
    tick_living,
};

#[cfg(test)]
mod tests;
