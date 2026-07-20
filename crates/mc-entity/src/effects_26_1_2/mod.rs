//! Bounded Java Edition 26.1.2 mob-effect lifecycle policy.
//!
//! The authoritative ECS remains the owner of entity rows, registry resolution,
//! immunity checks, attributes, health/food mutation, and publication. This
//! module owns only a caller-embedded active-effect component, vanilla merge and
//! duration transitions, and typed decisions that adapters may translate into
//! callbacks, metadata dirtiness, or effect packets. It never executes a
//! callback and never publishes a packet.
//!
//! [`ActiveEffects::plan_tick_batch`] requires the caller to provide one
//! [`EffectId`] per active effect in the order actions must be applied. The
//! kernel validates that complete unique set before producing decisions, then
//! returns plans and outcomes in the supplied order. The caller must resolve
//! every [`EffectApplication::CallerOwned`] decision before calling
//! [`ActiveEffects::commit_tick_batch`]. Storage and scratch are bounded and
//! allocate only while being constructed.
//!
//! Vanilla iterates a `HashMap<Holder<MobEffect>, _>` whose reference-holder
//! identity hash is not derivable from a registry ID. Internal numeric storage
//! order is therefore only a lookup implementation detail, never a vanilla
//! action-order claim. Reproducing an observed vanilla order belongs to the
//! caller that owns the holder/hash representation.
//!
//! Only regeneration, poison, wither, hunger, saturation, instant health, and
//! instant damage are encoded as gameplay actions. Every other effect belongs at
//! the explicit [`EffectKind::CallerOwned`] boundary. This is not a second entity
//! store or a claim of complete vanilla effect parity.
//!
//! Instant scales use Java-compatible double arithmetic and double-to-int cast
//! boundaries for every `f64`, including NaN and infinities. Saturation ignores
//! scale exactly as its inherited vanilla implementation does.

#![forbid(unsafe_code)]

mod gameplay;
mod instance;
mod store;

pub use gameplay::{
    EffectAction, EffectApplication, EffectDamageSource, InstantApplication, InstantDelivery,
    InstantPlanError, TargetEffectContext, plan_instant_application,
};
pub use instance::{EffectFlags, EffectId, EffectInstance, EffectKind};
pub use store::{
    ActiveEffectChainSnapshot, ActiveEffects, ActiveEffectsSnapshot, AddOutcome, CallerOwnedResult,
    EffectCapacities, EffectLimitError, EffectLimits, EffectStoreError, EffectTickOutcome,
    MAX_ACTIVE_EFFECTS, MAX_HIDDEN_EFFECTS, PendingEffectTick, RemoveOutcome, TickCommitError,
    TickPlanError, TickResolutionError, TickScratch, TickScratchCapacities, TickScratchError,
};

#[cfg(test)]
mod tests;
