//! Bounded Java Edition 26.1.2 living/effect tick composition.
//!
//! This module stages one scalar entity row and one caller-owned
//! [`ActiveEffects`] component. It does not own world queries, registry
//! resolution, callback execution, packets, or an entity store.
//!
//! The staged phase order follows the local 26.1.2 sources: caller-resolved
//! damage from the early base tick, hurt/invulnerability clocks, the guarded
//! death clock, then `LivingEntity.tickEffects`. Effect iteration order is an
//! explicit caller input because vanilla's active map uses holder identity;
//! this module does not invent an ordering parity claim.

#![forbid(unsafe_code)]

mod state;
mod transaction;

pub use crate::effects_26_1_2::{
    ActiveEffects, EffectAction, EffectApplication, EffectId, EffectInstance, PendingEffectTick,
    TargetEffectContext, TickPlanError,
};
pub use crate::living_26_1_2::{
    DamageApplied, DamageContext, DamageEvent, DamageOutcome, DamageRejection, DamageSource,
    InvulnerabilityClock, LivingState,
};
pub use state::{RemovalReason, RuntimeState, RuntimeStateError, StateRevision};
pub use transaction::{
    AppliedEffectAction, AppliedTick, ApplyError, DamageOrigin, EffectActionApplyError,
    EffectResolutionError, MAX_DAMAGE_INPUTS_PER_TICK, PrepareError, PreparedEffectAction,
    PreparedTick, PublicationFact, ResolvedDamage, RuntimeDamageSource, RuntimeScratch,
    RuntimeScratchCapacities, RuntimeScratchError, TargetKind, TickInput, TickMode,
    apply_effect_action, apply_tick, prepare_tick,
};

#[cfg(test)]
mod tests;
