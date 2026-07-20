use std::any::TypeId;
use std::array;
use std::rc::Rc;

use super::accessor::{
    Accessor, AccessorId, MAX_DATA_ITEMS, SchemaId, SerializerId, SerializerKey,
};
use super::value::{
    CapacityError, DataValue, DataValueBuffer, ErasedValue, PackError, stored_value,
};

struct DataItem {
    id: AccessorId,
    serializer: SerializerKey,
    value: Rc<dyn ErasedValue>,
    initial_value: Rc<dyn ErasedValue>,
    dirty: bool,
}

impl DataItem {
    fn is_default(&self) -> bool {
        self.value.equals_any(self.initial_value.as_any())
    }
}

pub struct SynchedEntityDataBuilder {
    schema: SchemaId,
    capacity: usize,
    items: [Option<DataItem>; MAX_DATA_ITEMS],
    defined: usize,
}

impl SynchedEntityDataBuilder {
    pub fn new(schema: SchemaId, capacity: usize) -> Result<Self, CapacityError> {
        if capacity > MAX_DATA_ITEMS {
            return Err(CapacityError::TooLarge {
                requested: capacity,
                max: MAX_DATA_ITEMS,
            });
        }
        Ok(Self {
            schema,
            capacity,
            items: array::from_fn(|_| None),
            defined: 0,
        })
    }

    pub fn define<T: 'static>(
        &mut self,
        accessor: Accessor<T>,
        initial_value: T,
    ) -> Result<(), DefineError> {
        let id = accessor.id().get();
        if accessor.schema() != self.schema {
            return Err(DefineError::StaleAccessor { id });
        }
        let index = usize::from(id);
        if index >= self.capacity {
            return Err(DefineError::OutsideCapacity {
                id,
                capacity: self.capacity,
            });
        }
        if self.items[index].is_some() {
            return Err(DefineError::Duplicate { id });
        }

        let serializer = accessor.serializer();
        let value = stored_value(serializer, initial_value);
        self.items[index] = Some(DataItem {
            id: accessor.id(),
            serializer: serializer.key(),
            initial_value: Rc::clone(&value),
            value,
            dirty: false,
        });
        self.defined += 1;
        Ok(())
    }

    pub fn build(self) -> Result<SynchedEntityData, BuildError> {
        if self.defined != self.capacity {
            for (index, item) in self.items[..self.capacity].iter().enumerate() {
                if item.is_none() {
                    return Err(BuildError::MissingDefinition {
                        id: u8::try_from(index).expect("capacity is bounded by vanilla IDs"),
                    });
                }
            }
        }
        Ok(SynchedEntityData {
            schema: self.schema,
            capacity: self.capacity,
            items: self.items,
            dirty: false,
        })
    }
}

impl std::fmt::Debug for SynchedEntityDataBuilder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SynchedEntityDataBuilder")
            .field("schema", &self.schema)
            .field("capacity", &self.capacity)
            .field("defined", &self.defined)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefineError {
    StaleAccessor { id: u8 },
    OutsideCapacity { id: u8, capacity: usize },
    Duplicate { id: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
    MissingDefinition { id: u8 },
}

pub struct SynchedEntityData {
    schema: SchemaId,
    capacity: usize,
    items: [Option<DataItem>; MAX_DATA_ITEMS],
    dirty: bool,
}

impl SynchedEntityData {
    #[must_use]
    pub const fn schema(&self) -> SchemaId {
        self.schema
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.capacity == 0
    }

    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn get<T: 'static>(&self, accessor: Accessor<T>) -> Result<&T, AccessError> {
        let index = self.validate_accessor(accessor)?;
        self.items[index]
            .as_ref()
            .expect("built data has contiguous definitions")
            .value
            .as_any()
            .downcast_ref::<T>()
            .ok_or(AccessError::ValueTypeMismatch {
                id: accessor.id().get(),
                serializer: accessor.serializer_id(),
            })
    }

    pub fn set<T: 'static>(
        &mut self,
        accessor: Accessor<T>,
        value: T,
    ) -> Result<SetOutcome, AccessError> {
        self.set_with_force(accessor, value, false)
    }

    pub fn set_with_force<T: 'static>(
        &mut self,
        accessor: Accessor<T>,
        value: T,
        force_dirty: bool,
    ) -> Result<SetOutcome, AccessError> {
        let index = self.validate_accessor(accessor)?;
        let item = self.items[index]
            .as_mut()
            .expect("built data has contiguous definitions");
        if !force_dirty && item.value.equals_any(&value) {
            return Ok(SetOutcome::Unchanged);
        }
        item.value = stored_value(accessor.serializer(), value);
        item.dirty = true;
        self.dirty = true;
        Ok(SetOutcome::Changed {
            update: AccessorUpdateFact {
                schema: self.schema,
                id: item.id,
                serializer: item.serializer,
            },
        })
    }

    pub fn pack_dirty<'a>(
        &mut self,
        output: &'a mut DataValueBuffer,
    ) -> Result<Option<&'a DataValueBuffer>, PackError> {
        if !self.dirty {
            output.clear_batch();
            return Ok(None);
        }

        let required = self.items[..self.capacity]
            .iter()
            .filter(|item| item.as_ref().is_some_and(|item| item.dirty))
            .count();
        output.prepare(required)?;
        for item in self.items[..self.capacity].iter_mut().flatten() {
            if item.dirty {
                output.push_copy(item.id, item.serializer, item.value.as_ref());
                item.dirty = false;
            }
        }
        self.dirty = false;
        Ok(Some(output))
    }

    pub fn pack_all<'a>(
        &self,
        output: &'a mut DataValueBuffer,
    ) -> Result<&'a DataValueBuffer, PackError> {
        output.prepare(self.capacity)?;
        for item in self.items[..self.capacity].iter().flatten() {
            output.push_copy(item.id, item.serializer, item.value.as_ref());
        }
        Ok(output)
    }

    pub fn non_default_values<'a>(
        &self,
        output: &'a mut DataValueBuffer,
    ) -> Result<Option<&'a DataValueBuffer>, PackError> {
        let required = self.items[..self.capacity]
            .iter()
            .filter(|item| item.as_ref().is_some_and(|item| !item.is_default()))
            .count();
        if required == 0 {
            output.clear_batch();
            return Ok(None);
        }
        output.prepare(required)?;
        for item in self.items[..self.capacity].iter().flatten() {
            if !item.is_default() {
                output.push_copy(item.id, item.serializer, item.value.as_ref());
            }
        }
        Ok(Some(output))
    }

    pub fn assign_values<'a>(
        &mut self,
        incoming: &'a [DataValue],
    ) -> Result<AssignOutcome<'a>, AssignValuesError<'a>> {
        let schema = self.schema;
        let mut applied = 0;
        for (input_index, incoming_value) in incoming.iter().enumerate() {
            let index = usize::from(incoming_value.id().get());
            if index >= self.capacity {
                return Err(AssignValuesError {
                    input_index,
                    applied,
                    kind: AssignValueError::OutsideCapacity {
                        id: incoming_value.id().get(),
                        capacity: self.capacity,
                    },
                    updates: AssignUpdateFacts::partial(schema, &incoming[..applied]),
                });
            }
            let item = self.items[index]
                .as_mut()
                .expect("built data has contiguous definitions");
            if incoming_value.serializer_key() != item.serializer {
                return Err(AssignValuesError {
                    input_index,
                    applied,
                    kind: AssignValueError::SerializerMismatch {
                        id: incoming_value.id().get(),
                        expected: item.serializer,
                        incoming: incoming_value.serializer_key(),
                    },
                    updates: AssignUpdateFacts::partial(schema, &incoming[..applied]),
                });
            }
            if item.value.as_any().type_id() != incoming_value.erased().as_any().type_id() {
                return Err(AssignValuesError {
                    input_index,
                    applied,
                    kind: AssignValueError::ValueTypeMismatch {
                        id: incoming_value.id().get(),
                        serializer: item.serializer.id,
                    },
                    updates: AssignUpdateFacts::partial(schema, &incoming[..applied]),
                });
            }
            item.value = incoming_value.shared_value();
            applied += 1;
        }
        Ok(AssignOutcome {
            applied,
            updates: AssignUpdateFacts::complete(schema, incoming),
        })
    }

    fn validate_accessor<T: 'static>(&self, accessor: Accessor<T>) -> Result<usize, AccessError> {
        let id = accessor.id().get();
        if accessor.schema() != self.schema {
            return Err(AccessError::StaleAccessor { id });
        }
        let index = usize::from(id);
        if index >= self.capacity {
            return Err(AccessError::OutsideCapacity {
                id,
                capacity: self.capacity,
            });
        }
        let item = self.items[index]
            .as_ref()
            .expect("built data has contiguous definitions");
        if accessor.serializer_key() != item.serializer {
            return Err(AccessError::SerializerMismatch {
                id,
                expected: item.serializer,
                incoming: accessor.serializer_key(),
            });
        }
        if item.value.as_any().type_id() != TypeId::of::<T>() {
            return Err(AccessError::ValueTypeMismatch {
                id,
                serializer: item.serializer.id,
            });
        }
        Ok(index)
    }
}

impl std::fmt::Debug for SynchedEntityData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SynchedEntityData")
            .field("schema", &self.schema)
            .field("capacity", &self.capacity)
            .field("dirty", &self.dirty)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    StaleAccessor {
        id: u8,
    },
    OutsideCapacity {
        id: u8,
        capacity: usize,
    },
    SerializerMismatch {
        id: u8,
        expected: SerializerKey,
        incoming: SerializerKey,
    },
    ValueTypeMismatch {
        id: u8,
        serializer: SerializerId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetOutcome {
    Unchanged,
    Changed { update: AccessorUpdateFact },
}

impl SetOutcome {
    #[must_use]
    pub const fn accessor_update(self) -> Option<AccessorUpdateFact> {
        match self {
            Self::Unchanged => None,
            Self::Changed { update } => Some(update),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessorUpdateFact {
    pub schema: SchemaId,
    pub id: AccessorId,
    pub serializer: SerializerKey,
}

#[derive(Debug, Clone, Copy)]
pub struct BatchUpdateFact<'a> {
    values: &'a [DataValue],
}

impl<'a> BatchUpdateFact<'a> {
    #[must_use]
    pub const fn values(self) -> &'a [DataValue] {
        self.values
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AssignUpdateFacts<'a> {
    schema: SchemaId,
    per_accessor: &'a [DataValue],
    batch: Option<&'a [DataValue]>,
}

impl<'a> AssignUpdateFacts<'a> {
    const fn complete(schema: SchemaId, values: &'a [DataValue]) -> Self {
        Self {
            schema,
            per_accessor: values,
            batch: Some(values),
        }
    }

    const fn partial(schema: SchemaId, values: &'a [DataValue]) -> Self {
        Self {
            schema,
            per_accessor: values,
            batch: None,
        }
    }

    pub fn accessor_updates(self) -> impl ExactSizeIterator<Item = AccessorUpdateFact> + 'a {
        self.per_accessor
            .iter()
            .map(move |value| AccessorUpdateFact {
                schema: self.schema,
                id: value.id(),
                serializer: value.serializer_key(),
            })
    }

    #[must_use]
    pub const fn batch_update(self) -> Option<BatchUpdateFact<'a>> {
        match self.batch {
            Some(values) => Some(BatchUpdateFact { values }),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AssignOutcome<'a> {
    pub applied: usize,
    updates: AssignUpdateFacts<'a>,
}

impl<'a> AssignOutcome<'a> {
    #[must_use]
    pub const fn updates(self) -> AssignUpdateFacts<'a> {
        self.updates
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AssignValuesError<'a> {
    pub input_index: usize,
    pub applied: usize,
    pub kind: AssignValueError,
    updates: AssignUpdateFacts<'a>,
}

impl<'a> AssignValuesError<'a> {
    #[must_use]
    pub const fn updates(self) -> AssignUpdateFacts<'a> {
        self.updates
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignValueError {
    OutsideCapacity {
        id: u8,
        capacity: usize,
    },
    SerializerMismatch {
        id: u8,
        expected: SerializerKey,
        incoming: SerializerKey,
    },
    ValueTypeMismatch {
        id: u8,
        serializer: SerializerId,
    },
}
