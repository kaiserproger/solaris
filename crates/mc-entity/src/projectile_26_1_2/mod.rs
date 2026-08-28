//! Pure Minecraft Java 26.1.2 projectile lifecycle policy.
//!
//! The local decompiled `Projectile`, `ThrowableProjectile`,
//! `arrow/AbstractArrow`, `ProjectileUtil`, `Entity`, `Mth`, and `AABB`
//! classes are the behavioral oracle. This module preserves the observed
//! ordering and arithmetic only for the state represented here. It does not
//! claim parity for projectile subclasses, damage/enchantment evaluation,
//! random sampling, block/entity raycasts, packets, sounds, criteria, portals,
//! block effects, or fire state.
//!
//! Raycast candidates arrive as complete mutable caller-owned working slices
//! in deterministic order. The kernel validates and reorders them in place,
//! preserving caller order for equal vanilla distance; callers that cannot
//! provide complete input must defer the transition. Complete owner
//! root-vehicle membership remains a streamed caller input. RNG, damage, and
//! deflection decisions arrive as resolved typed inputs. Prepared transitions
//! retain the complete expected state and three caller fact revisions; commit
//! rejects every stale precondition before mutation. Piercing ledgers and
//! publication batches are fixed arrays, and publication exhaustion is a typed
//! prepare failure, so a warmed tick cannot allocate or grow storage.
//!
//! Two safety boundaries intentionally differ from vanilla: non-finite AABB
//! coordinates are rejected instead of retained, and pathological finite
//! rotations are rejected when float spacing prevents bounded angle
//! normalization. Neither path enters an unbounded normalization loop.

mod arrow;
mod hit_order;
mod hurting;
mod lifecycle;
mod throwable;

pub use arrow::*;
pub use hit_order::*;
pub use hurting::*;
pub use lifecycle::*;
pub use throwable::*;

#[cfg(test)]
mod arrow_tests;
#[cfg(test)]
mod hurting_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod throwable_tests;
