//! Bounded Minecraft Java 26.1.2 `SynchedEntityData` state kernel.
//!
//! The caller owns entity callbacks, serializer registration, and wire encoding.
//! This module owns accessor validation, typed values, defaults, dirtiness, and
//! serializer-copy snapshots. A [`SchemaId`] must uniquely identify one accessor
//! layout, and a [`SerializerIdentity`] must uniquely identify one registered
//! serializer policy. Reusing either identity defeats mismatch detection.
//!
//! Store and output indexing use fixed arrays bounded by vanilla's accessor ID
//! range. Initial and current values share one safe `Rc` reference until `set`
//! or assignment replaces current. Definition and each changed `set` allocate
//! an erased holder; assignment shares the incoming [`DataValue`] holder.
//! [`DataValueBuffer`] allocates an erased snapshot on first pack and whenever a
//! cached snapshot is externally shared or changes serializer identity. Otherwise
//! it asks the caller's `copy_from` policy to refresh the cached holder. Both
//! creating and cloning [`DataValue`] allocate an erased holder, and cloning
//! invokes the serializer's `copy` policy. Both `copy` and `copy_from` policies
//! may allocate internally.
//!
//! [`SetOutcome`] and [`AssignUpdateFacts`] describe callback publication without
//! embedding callbacks. Assignment facts borrow the original input: accessor
//! facts preserve mutation order, and a batch fact exists only after complete
//! success (including an empty input).

#![forbid(unsafe_code)]

mod accessor;
mod store;
mod value;

pub use accessor::{
    Accessor, AccessorId, AccessorIdError, MAX_ACCESSOR_ID, MAX_DATA_ITEMS, SchemaId, Serializer,
    SerializerId, SerializerIdentity, SerializerKey, java_f32_equals,
};
pub use store::{
    AccessError, AccessorUpdateFact, AssignOutcome, AssignUpdateFacts, AssignValueError,
    AssignValuesError, BatchUpdateFact, BuildError, DefineError, SetOutcome, SynchedEntityData,
    SynchedEntityDataBuilder,
};
pub use value::{CapacityError, DataValue, DataValueBuffer, DataValueReadError, PackError};

#[cfg(test)]
mod tests;
