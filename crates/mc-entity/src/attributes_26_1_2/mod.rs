//! Bounded Java Edition 26.1.2 entity-attribute policy.
//!
//! The kernel owns value calculation, modifier lifetime, lazy attribute-map
//! instances, dirty publication facts, and persistence projections. Modifier
//! keys are owned, validated [`Identifier`] values used unchanged by runtime
//! and packed projections. Canonical and permanent modifiers retain vanilla's
//! array-map insertion order. Operation buckets emulate fastutil 8.5.18's
//! default open-address table, including hash mixing, probing, descending
//! value iteration, shifting, resizing, and `putAll` reinsertion.
//! Cross-attribute persistence and pending publication facts instead use
//! deterministic [`AttributeId`] order. Vanilla keys those collections by JVM
//! identity hashes, so this is a deliberate semantic-order boundary, not
//! iteration or wire-order parity. Sequential assignments retain the source's
//! first-materialization input order so a late failure preserves that prefix.
//!
//! This is not a registry, NBT/codec, or packet implementation. Vanilla 26.1.2
//! `Holder<Attribute>` resolution remains the caller's responsibility. Plain
//! identity sanitization and `RangedAttribute` sanitization are supported;
//! custom `Attribute` subclasses with other sanitizers are not. Identifiers are
//! bounded to [`MAX_IDENTIFIER_BYTES`] canonical ASCII bytes.
//!
//! Java's callback sets are represented as pending update and sync id slices on
//! [`AttributeMap`]. Reading a cached value does not clear those slices. Callers
//! clear each slice after performing the corresponding publication work.
//! Packed lists are applied in input order, like Java. Each record is atomic
//! with respect to bounded-capacity failure, while records accepted before a
//! later failure remain applied.
//!
//! [`AttributeModifier`] is a shared immutable record handle. Reusing a clone of
//! the same handle models Java reference identity and makes a transient upsert
//! idempotent; constructing a distinct value-equal record dirties and publishes.
//! Instance construction preallocates canonical, permanent, and operation-table
//! storage up to the configured bound. Warm value reads and in-bound mutations
//! do not allocate; identifier/modifier construction, lazy materialization, and
//! persistence projection use normal Rust allocation semantics.

#![forbid(unsafe_code)]

mod identifier;
mod instance;
mod map;

pub use identifier::{Identifier, IdentifierError, MAX_IDENTIFIER_BYTES};
pub use instance::{
    AttributeDefinition, AttributeDefinitionError, AttributeId, AttributeInstance,
    AttributeInstanceError, AttributeModifier, DirtyEffect, InstanceCapacities,
    MAX_MODIFIERS_PER_ATTRIBUTE, Operation, PackedAttribute,
};
pub use map::{
    ApplyReport, AttributeMap, AttributeMapError, AttributeMapInitError, AttributeMapLimits,
    AttributeTemplate, InstantiationOutcome, MAX_ATTRIBUTE_INSTANCES, ModifierPersistence,
    TemplateModifier,
};

#[cfg(test)]
mod tests;
