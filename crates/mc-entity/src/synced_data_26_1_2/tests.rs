use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use super::*;

const BYTE_ID: SerializerId = SerializerId::new(0);
const INT_ID: SerializerId = SerializerId::new(1);
const STRING_ID: SerializerId = SerializerId::new(4);
const BYTE_IDENTITY: SerializerIdentity = SerializerIdentity::new(1);
const INT_IDENTITY: SerializerIdentity = SerializerIdentity::new(2);
const STRING_IDENTITY: SerializerIdentity = SerializerIdentity::new(3);
const DISTINCT_BYTE_IDENTITY: SerializerIdentity = SerializerIdentity::new(4);
const SHARED_IDENTITY: SerializerIdentity = SerializerIdentity::new(5);
const BYTE_KEY: SerializerKey = SerializerKey {
    id: BYTE_ID,
    identity: BYTE_IDENTITY,
};
const INT_KEY: SerializerKey = SerializerKey {
    id: INT_ID,
    identity: INT_IDENTITY,
};

fn byte_serializer() -> Serializer<u8> {
    Serializer::cloned(BYTE_ID, BYTE_IDENTITY, PartialEq::eq)
}

fn never_equal(_: &u8, _: &u8) -> bool {
    false
}

fn distinct_byte_policy() -> Serializer<u8> {
    Serializer::cloned(BYTE_ID, DISTINCT_BYTE_IDENTITY, never_equal)
}

fn int_serializer() -> Serializer<i32> {
    Serializer::cloned(INT_ID, INT_IDENTITY, PartialEq::eq)
}

fn string_serializer() -> Serializer<String> {
    Serializer::cloned(STRING_ID, STRING_IDENTITY, PartialEq::eq)
}

fn accessor<T: 'static>(schema: SchemaId, id: u8, serializer: Serializer<T>) -> Accessor<T> {
    schema.accessor(AccessorId::new(u16::from(id)).unwrap(), serializer)
}

fn two_item_data() -> (SynchedEntityData, Accessor<u8>, Accessor<i32>) {
    let schema = SchemaId::new(10);
    let flags = accessor(schema, 0, byte_serializer());
    let air = accessor(schema, 1, int_serializer());
    let mut builder = SynchedEntityDataBuilder::new(schema, 2).unwrap();
    builder.define(flags, 0).unwrap();
    builder.define(air, 300).unwrap();
    (builder.build().unwrap(), flags, air)
}

fn packed_rows(buffer: &DataValueBuffer) -> Vec<(u8, SerializerId)> {
    buffer
        .iter()
        .map(|value| (value.id().get(), value.serializer_id()))
        .collect()
}

#[test]
fn ids_and_accessors_retain_their_explicit_identity() {
    assert_eq!(
        AccessorId::new(255).unwrap_err(),
        AccessorIdError::OutOfRange {
            requested: 255,
            max: MAX_ACCESSOR_ID,
        }
    );

    let schema = SchemaId::new(77);
    let field = accessor(schema, 12, string_serializer());
    assert_eq!(field.id().get(), 12);
    assert_eq!(field.serializer_id(), STRING_ID);
    assert_eq!(field.schema(), schema);
}

fn accessor_hash<T: 'static>(accessor: Accessor<T>) -> u64 {
    let mut hasher = DefaultHasher::new();
    accessor.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn accessor_equality_and_hashing_use_only_the_vanilla_numeric_id() {
    let first = accessor(SchemaId::new(1), 0, byte_serializer());
    let same_id_other_schema_and_policy = accessor(SchemaId::new(2), 0, distinct_byte_policy());
    let same_id_other_type = accessor(SchemaId::new(3), 0, int_serializer());
    let different_id = accessor(SchemaId::new(1), 1, byte_serializer());

    assert_eq!(first, same_id_other_schema_and_policy);
    assert!(first == same_id_other_type);
    assert_ne!(first, different_id);
    assert_eq!(
        accessor_hash(first),
        accessor_hash(same_id_other_schema_and_policy)
    );
    assert_eq!(accessor_hash(first), accessor_hash(same_id_other_type));
}

#[test]
fn builder_rejects_invalid_capacity_stale_outside_and_duplicate_definitions() {
    assert_eq!(
        SynchedEntityDataBuilder::new(SchemaId::new(1), MAX_DATA_ITEMS + 1).unwrap_err(),
        CapacityError::TooLarge {
            requested: MAX_DATA_ITEMS + 1,
            max: MAX_DATA_ITEMS,
        }
    );

    let schema = SchemaId::new(1);
    let stale_schema = SchemaId::new(2);
    let first = accessor(schema, 0, byte_serializer());
    let stale = accessor(stale_schema, 0, byte_serializer());
    let outside = accessor(schema, 1, byte_serializer());
    let mut builder = SynchedEntityDataBuilder::new(schema, 1).unwrap();

    assert_eq!(
        builder.define(stale, 1).unwrap_err(),
        DefineError::StaleAccessor { id: 0 }
    );
    assert_eq!(
        builder.define(outside, 1).unwrap_err(),
        DefineError::OutsideCapacity { id: 1, capacity: 1 }
    );
    builder.define(first, 2).unwrap();
    assert_eq!(
        builder.define(first, 3).unwrap_err(),
        DefineError::Duplicate { id: 0 }
    );

    let data = builder.build().unwrap();
    assert_eq!(*data.get(first).unwrap(), 2);
}

#[test]
fn build_reports_the_first_missing_contiguous_definition() {
    let schema = SchemaId::new(3);
    let second = accessor(schema, 1, int_serializer());
    let mut builder = SynchedEntityDataBuilder::new(schema, 2).unwrap();
    builder.define(second, 20).unwrap();

    assert_eq!(
        builder.build().unwrap_err(),
        BuildError::MissingDefinition { id: 0 }
    );
}

#[test]
fn zero_capacity_data_is_valid_and_packs_an_empty_all_batch() {
    let schema = SchemaId::new(4);
    let mut data = SynchedEntityDataBuilder::new(schema, 0)
        .unwrap()
        .build()
        .unwrap();
    let mut output = DataValueBuffer::new(0).unwrap();

    assert!(data.pack_dirty(&mut output).unwrap().is_none());
    assert!(data.non_default_values(&mut output).unwrap().is_none());
    assert!(data.pack_all(&mut output).unwrap().is_empty());
}

#[test]
fn get_and_set_reject_every_accessor_mismatch_without_mutation() {
    let (mut data, flags, _) = two_item_data();
    let stale = accessor(SchemaId::new(11), 0, byte_serializer());
    let outside = accessor(SchemaId::new(10), 2, byte_serializer());
    let wrong_serializer = accessor(
        SchemaId::new(10),
        0,
        Serializer::cloned(INT_ID, INT_IDENTITY, PartialEq::eq),
    );
    let wrong_type = accessor(
        SchemaId::new(10),
        0,
        Serializer::cloned(BYTE_ID, BYTE_IDENTITY, PartialEq::eq),
    );

    assert_eq!(
        data.set(stale, 9).unwrap_err(),
        AccessError::StaleAccessor { id: 0 }
    );
    assert_eq!(
        data.set(outside, 9).unwrap_err(),
        AccessError::OutsideCapacity { id: 2, capacity: 2 }
    );
    assert_eq!(
        data.set(wrong_serializer, 9).unwrap_err(),
        AccessError::SerializerMismatch {
            id: 0,
            expected: BYTE_KEY,
            incoming: INT_KEY,
        }
    );
    assert_eq!(
        data.set(wrong_type, 9_i8).unwrap_err(),
        AccessError::ValueTypeMismatch {
            id: 0,
            serializer: BYTE_ID,
        }
    );
    assert_eq!(*data.get(flags).unwrap(), 0);
    assert!(!data.is_dirty());

    assert_eq!(
        data.get(stale).unwrap_err(),
        AccessError::StaleAccessor { id: 0 }
    );
    assert_eq!(
        data.get(wrong_serializer).unwrap_err(),
        AccessError::SerializerMismatch {
            id: 0,
            expected: BYTE_KEY,
            incoming: INT_KEY,
        }
    );
    assert_eq!(
        data.get(wrong_type).unwrap_err(),
        AccessError::ValueTypeMismatch {
            id: 0,
            serializer: BYTE_ID,
        }
    );
}

#[test]
fn serializer_matching_rejects_distinct_policies_that_share_a_wire_id() {
    let (mut data, flags, _) = two_item_data();
    let mismatched_accessor = accessor(SchemaId::new(10), 0, distinct_byte_policy());

    assert!(matches!(
        data.set(mismatched_accessor, 1),
        Err(AccessError::SerializerMismatch { .. })
    ));
    assert_eq!(*data.get(flags).unwrap(), 0);

    let incoming = DataValue::new(flags.id(), distinct_byte_policy(), 2);
    assert!(matches!(
        data.assign_values(std::slice::from_ref(&incoming)),
        Err(AssignValuesError {
            kind: AssignValueError::SerializerMismatch { .. },
            ..
        })
    ));
    assert!(matches!(
        incoming.read(byte_serializer()),
        Err(DataValueReadError::SerializerMismatch { .. })
    ));
}

#[test]
fn set_dirties_only_real_or_forced_changes_and_packs_the_latest_value_once() {
    let (mut data, flags, air) = two_item_data();
    let mut output = DataValueBuffer::new(2).unwrap();

    assert_eq!(data.set(flags, 0).unwrap(), SetOutcome::Unchanged);
    assert!(!data.is_dirty());
    assert!(data.pack_dirty(&mut output).unwrap().is_none());

    assert!(matches!(
        data.set(air, 299).unwrap(),
        SetOutcome::Changed { .. }
    ));
    assert!(matches!(
        data.set(flags, 1).unwrap(),
        SetOutcome::Changed { .. }
    ));
    assert!(matches!(
        data.set(air, 298).unwrap(),
        SetOutcome::Changed { .. }
    ));
    assert!(data.is_dirty());

    let packed = data.pack_dirty(&mut output).unwrap().unwrap();
    assert_eq!(packed_rows(packed), vec![(0, BYTE_ID), (1, INT_ID)]);
    assert_eq!(*packed.get(0).unwrap().read(byte_serializer()).unwrap(), 1);
    assert_eq!(*packed.get(1).unwrap().read(int_serializer()).unwrap(), 298);
    assert!(!data.is_dirty());
    assert!(data.pack_dirty(&mut output).unwrap().is_none());

    assert!(matches!(
        data.set_with_force(flags, 1, true).unwrap(),
        SetOutcome::Changed { .. }
    ));
    assert!(data.is_dirty());
}

#[test]
fn set_emits_only_the_vanilla_per_accessor_callback_fact() {
    let (mut data, flags, _) = two_item_data();
    assert!(data.set(flags, 0).unwrap().accessor_update().is_none());

    assert_eq!(
        data.set(flags, 1).unwrap().accessor_update(),
        Some(AccessorUpdateFact {
            schema: SchemaId::new(10),
            id: flags.id(),
            serializer: BYTE_KEY,
        })
    );
}

#[test]
fn java_float_equality_treats_nan_as_equal_and_signed_zero_as_different() {
    let schema = SchemaId::new(20);
    let float_id = SerializerId::new(3);
    let serializer = Serializer::cloned(float_id, SerializerIdentity::new(6), java_f32_equals);
    let value = accessor(schema, 0, serializer);
    let mut builder = SynchedEntityDataBuilder::new(schema, 1).unwrap();
    builder.define(value, f32::from_bits(0x7fc0_0001)).unwrap();
    let mut data = builder.build().unwrap();

    assert_eq!(
        data.set(value, f32::from_bits(0x7fc0_1234)).unwrap(),
        SetOutcome::Unchanged
    );
    data.set_with_force(value, 0.0, true).unwrap();
    let mut output = DataValueBuffer::new(1).unwrap();
    data.pack_dirty(&mut output).unwrap();
    assert!(matches!(
        data.set(value, -0.0).unwrap(),
        SetOutcome::Changed { .. }
    ));
}

#[test]
fn failed_dirty_pack_preserves_dirty_bits_and_the_previous_output_batch() {
    let (mut data, flags, air) = two_item_data();
    let mut output = DataValueBuffer::new(1).unwrap();
    data.set(flags, 1).unwrap();
    let previous = data.pack_dirty(&mut output).unwrap().unwrap();
    assert_eq!(previous.get(0).unwrap().id().get(), 0);

    data.set(flags, 2).unwrap();
    data.set(air, 200).unwrap();
    assert_eq!(
        data.pack_dirty(&mut output).unwrap_err(),
        PackError::OutputCapacityExceeded {
            required: 2,
            capacity: 1,
        }
    );
    assert!(data.is_dirty());
    assert_eq!(output.len(), 1);
    assert_eq!(output.get(0).unwrap().id().get(), 0);

    let mut enough = DataValueBuffer::new(2).unwrap();
    assert_eq!(data.pack_dirty(&mut enough).unwrap().unwrap().len(), 2);
    assert!(!data.is_dirty());
}

#[test]
fn non_default_values_are_ordered_optional_and_do_not_consume_dirty_state() {
    let (mut data, flags, air) = two_item_data();
    let mut output = DataValueBuffer::new(2).unwrap();
    assert!(data.non_default_values(&mut output).unwrap().is_none());

    data.set(air, 299).unwrap();
    data.set(flags, 1).unwrap();
    let values = data.non_default_values(&mut output).unwrap().unwrap();
    assert_eq!(packed_rows(values), vec![(0, BYTE_ID), (1, INT_ID)]);
    assert!(data.is_dirty());

    data.set(flags, 0).unwrap();
    let values = data.non_default_values(&mut output).unwrap().unwrap();
    assert_eq!(packed_rows(values), vec![(1, INT_ID)]);

    data.set(air, 300).unwrap();
    assert!(data.non_default_values(&mut output).unwrap().is_none());
    assert!(data.is_dirty());
}

#[test]
fn non_default_and_pack_all_report_output_capacity_without_changing_the_buffer() {
    let (mut data, flags, _) = two_item_data();
    let mut output = DataValueBuffer::new(1).unwrap();
    data.set(flags, 1).unwrap();
    data.non_default_values(&mut output).unwrap();
    assert_eq!(output.len(), 1);

    assert_eq!(
        data.pack_all(&mut output).unwrap_err(),
        PackError::OutputCapacityExceeded {
            required: 2,
            capacity: 1,
        }
    );
    assert_eq!(output.len(), 1);

    data.set(accessor(SchemaId::new(10), 1, int_serializer()), 299)
        .unwrap();
    assert_eq!(
        data.non_default_values(&mut output).unwrap_err(),
        PackError::OutputCapacityExceeded {
            required: 2,
            capacity: 1,
        }
    );
    assert_eq!(output.len(), 1);
}

#[test]
fn pack_all_orders_every_value_and_does_not_clear_dirty_bits() {
    let (mut data, flags, _) = two_item_data();
    let mut output = DataValueBuffer::new(2).unwrap();
    data.set(flags, 7).unwrap();

    let all = data.pack_all(&mut output).unwrap();
    assert_eq!(packed_rows(all), vec![(0, BYTE_ID), (1, INT_ID)]);
    assert_eq!(*all.get(0).unwrap().read(byte_serializer()).unwrap(), 7);
    assert!(data.is_dirty());

    let dirty = data.pack_dirty(&mut output).unwrap().unwrap();
    assert_eq!(packed_rows(dirty), vec![(0, BYTE_ID)]);
}

#[test]
fn assign_values_applies_every_entry_in_order_without_creating_dirty_state() {
    let (mut data, flags, air) = two_item_data();
    let incoming = [
        DataValue::new(flags.id(), byte_serializer(), 1),
        DataValue::new(flags.id(), byte_serializer(), 2),
        DataValue::new(air.id(), int_serializer(), 250),
    ];

    assert_eq!(data.assign_values(&incoming).unwrap().applied, 3);
    assert_eq!(*data.get(flags).unwrap(), 2);
    assert_eq!(*data.get(air).unwrap(), 250);
    assert!(!data.is_dirty());
    assert_eq!(data.assign_values(&[]).unwrap().applied, 0);

    data.set(flags, 3).unwrap();
    data.assign_values(&[DataValue::new(flags.id(), byte_serializer(), 4)])
        .unwrap();
    assert!(
        data.is_dirty(),
        "assignment must preserve an existing dirty bit"
    );
    let mut output = DataValueBuffer::new(2).unwrap();
    let packed = data.pack_dirty(&mut output).unwrap().unwrap();
    assert_eq!(*packed.get(0).unwrap().read(byte_serializer()).unwrap(), 4);
}

#[test]
fn assign_values_emits_accessor_facts_then_one_batch_fact_over_the_exact_input() {
    let (mut data, flags, air) = two_item_data();
    let incoming = [
        DataValue::new(flags.id(), byte_serializer(), 1),
        DataValue::new(flags.id(), byte_serializer(), 2),
        DataValue::new(air.id(), int_serializer(), 250),
    ];

    let outcome = data.assign_values(&incoming).unwrap();
    let updates: Vec<_> = outcome.updates().accessor_updates().collect();
    assert_eq!(
        updates,
        vec![
            AccessorUpdateFact {
                schema: SchemaId::new(10),
                id: flags.id(),
                serializer: BYTE_KEY,
            },
            AccessorUpdateFact {
                schema: SchemaId::new(10),
                id: flags.id(),
                serializer: BYTE_KEY,
            },
            AccessorUpdateFact {
                schema: SchemaId::new(10),
                id: air.id(),
                serializer: INT_KEY,
            },
        ]
    );
    let batch = outcome.updates().batch_update().unwrap();
    assert!(std::ptr::eq(batch.values(), incoming.as_slice()));

    let empty = data.assign_values(&[]).unwrap();
    assert_eq!(empty.updates().accessor_updates().len(), 0);
    assert!(empty.updates().batch_update().is_some());
}

#[test]
fn assign_values_replaces_current_with_the_incoming_value_reference() {
    let schema = SchemaId::new(32);
    let field = accessor(schema, 0, shared_serializer());
    let mut builder = SynchedEntityDataBuilder::new(schema, 1).unwrap();
    builder.define(field, SharedNumber::new(1)).unwrap();
    let mut data = builder.build().unwrap();
    let incoming = DataValue::new(field.id(), shared_serializer(), SharedNumber::new(2));
    let incoming_allocation = incoming.read(shared_serializer()).unwrap().allocation();

    data.assign_values(std::slice::from_ref(&incoming)).unwrap();
    assert_eq!(data.get(field).unwrap().allocation(), incoming_allocation);

    incoming.read(shared_serializer()).unwrap().set(3);
    assert_eq!(data.get(field).unwrap().get(), 3);
}

#[test]
fn assign_serializer_mismatch_reports_and_keeps_the_vanilla_applied_prefix() {
    let (mut data, flags, air) = two_item_data();
    let incoming = [
        DataValue::new(flags.id(), byte_serializer(), 9),
        DataValue::new(
            air.id(),
            Serializer::cloned(BYTE_ID, BYTE_IDENTITY, PartialEq::eq),
            8_u8,
        ),
    ];

    let error = data.assign_values(&incoming).unwrap_err();
    assert_eq!(error.input_index, 1);
    assert_eq!(error.applied, 1);
    assert_eq!(
        error.kind,
        AssignValueError::SerializerMismatch {
            id: 1,
            expected: INT_KEY,
            incoming: BYTE_KEY,
        }
    );
    assert_eq!(
        error.updates().accessor_updates().collect::<Vec<_>>(),
        vec![AccessorUpdateFact {
            schema: SchemaId::new(10),
            id: flags.id(),
            serializer: BYTE_KEY,
        }]
    );
    assert!(error.updates().batch_update().is_none());
    assert_eq!(*data.get(flags).unwrap(), 9);
    assert_eq!(*data.get(air).unwrap(), 300);
    assert!(!data.is_dirty());
}

#[test]
fn assign_type_mismatch_and_outside_capacity_are_typed_and_local() {
    let (mut data, flags, air) = two_item_data();
    let wrong_type = DataValue::new(
        flags.id(),
        Serializer::cloned(BYTE_ID, BYTE_IDENTITY, PartialEq::eq),
        1_i8,
    );
    let wrong_type_values = [wrong_type];
    let error = data.assign_values(&wrong_type_values).unwrap_err();
    assert_eq!(error.input_index, 0);
    assert_eq!(error.applied, 0);
    assert_eq!(
        error.kind,
        AssignValueError::ValueTypeMismatch {
            id: 0,
            serializer: BYTE_ID,
        }
    );
    assert_eq!(error.updates().accessor_updates().len(), 0);
    assert!(error.updates().batch_update().is_none());
    assert_eq!(*data.get(flags).unwrap(), 0);

    let outside = DataValue::new(AccessorId::new(2).unwrap(), int_serializer(), 100);
    let outside_values = [DataValue::new(air.id(), int_serializer(), 299), outside];
    let error = data.assign_values(&outside_values).unwrap_err();
    assert_eq!(error.input_index, 1);
    assert_eq!(error.applied, 1);
    assert_eq!(
        error.kind,
        AssignValueError::OutsideCapacity { id: 2, capacity: 2 }
    );
    assert_eq!(
        error.updates().accessor_updates().collect::<Vec<_>>(),
        vec![AccessorUpdateFact {
            schema: SchemaId::new(10),
            id: air.id(),
            serializer: INT_KEY,
        }]
    );
    assert!(error.updates().batch_update().is_none());
    assert_eq!(*data.get(air).unwrap(), 299);
}

#[test]
fn data_value_reads_validate_serializer_and_concrete_type() {
    let value = DataValue::new(AccessorId::new(0).unwrap(), byte_serializer(), 3_u8);
    assert_eq!(*value.read(byte_serializer()).unwrap(), 3);
    assert_eq!(
        value.read(int_serializer()).unwrap_err(),
        DataValueReadError::SerializerMismatch {
            expected: BYTE_KEY,
            requested: INT_KEY,
        }
    );
    assert_eq!(
        value
            .read(Serializer::<i8>::cloned(
                BYTE_ID,
                BYTE_IDENTITY,
                PartialEq::eq,
            ))
            .unwrap_err(),
        DataValueReadError::ValueTypeMismatch {
            serializer: BYTE_ID,
        }
    );
}

#[derive(Debug, Clone)]
struct SharedNumber(Rc<Cell<i32>>);

impl SharedNumber {
    fn new(value: i32) -> Self {
        Self(Rc::new(Cell::new(value)))
    }

    fn get(&self) -> i32 {
        self.0.get()
    }

    fn set(&self, value: i32) {
        self.0.set(value);
    }

    fn allocation(&self) -> *const Cell<i32> {
        Rc::as_ptr(&self.0)
    }
}

fn shared_equal(left: &SharedNumber, right: &SharedNumber) -> bool {
    left.get() == right.get()
}

fn shared_copy(value: &SharedNumber) -> SharedNumber {
    SharedNumber::new(value.get())
}

fn shared_copy_from(source: &SharedNumber, target: &mut SharedNumber) {
    target.set(source.get());
}

fn shared_serializer() -> Serializer<SharedNumber> {
    Serializer::new(
        SerializerId::new(40),
        SHARED_IDENTITY,
        shared_equal,
        shared_copy,
        shared_copy_from,
    )
}

#[test]
fn initial_and_current_share_one_reference_until_set_replaces_current() {
    let schema = SchemaId::new(30);
    let field = accessor(schema, 0, shared_serializer());
    let external = SharedNumber::new(7);
    let mut builder = SynchedEntityDataBuilder::new(schema, 1).unwrap();
    builder.define(field, external.clone()).unwrap();
    let mut data = builder.build().unwrap();
    let mut output = DataValueBuffer::new(1).unwrap();

    external.set(8);
    assert_eq!(data.get(field).unwrap().get(), 8);
    assert!(data.non_default_values(&mut output).unwrap().is_none());

    data.set(field, SharedNumber::new(9)).unwrap();
    assert_eq!(
        data.non_default_values(&mut output).unwrap().unwrap().len(),
        1
    );

    external.set(9);
    assert!(data.non_default_values(&mut output).unwrap().is_none());
}

#[test]
fn serializer_copy_policy_isolates_packed_snapshots_and_reuses_unshared_warm_slots() {
    let schema = SchemaId::new(31);
    let field = accessor(schema, 0, shared_serializer());
    let external = SharedNumber::new(7);
    let mut builder = SynchedEntityDataBuilder::new(schema, 1).unwrap();
    builder.define(field, external.clone()).unwrap();
    let data = builder.build().unwrap();
    let mut output = DataValueBuffer::new(1).unwrap();

    let packed = data.pack_all(&mut output).unwrap();
    let snapshot = packed.get(0).unwrap().read(shared_serializer()).unwrap();
    assert_eq!(snapshot.get(), 7);
    assert_ne!(snapshot.allocation(), external.allocation());
    let warm_allocation = snapshot.allocation();

    external.set(8);
    let repacked = data.pack_all(&mut output).unwrap();
    let refreshed = repacked.get(0).unwrap().read(shared_serializer()).unwrap();
    assert_eq!(refreshed.get(), 8);
    assert_eq!(refreshed.allocation(), warm_allocation);
    assert_eq!(output.warmed_slots(), 1);
}

#[test]
fn data_value_buffer_rejects_oversized_capacity() {
    assert_eq!(
        DataValueBuffer::new(MAX_DATA_ITEMS + 1).unwrap_err(),
        CapacityError::TooLarge {
            requested: MAX_DATA_ITEMS + 1,
            max: MAX_DATA_ITEMS,
        }
    );
}
