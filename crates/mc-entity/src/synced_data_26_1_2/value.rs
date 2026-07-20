use std::any::Any;
use std::array;
use std::rc::Rc;

use super::accessor::{AccessorId, MAX_DATA_ITEMS, Serializer, SerializerId, SerializerKey};

pub(crate) trait ErasedValue {
    fn as_any(&self) -> &dyn Any;
    fn equals_any(&self, other: &dyn Any) -> bool;
    fn copy_shared(&self) -> Rc<dyn ErasedValue>;
    fn copy_from_erased(&mut self, source: &dyn ErasedValue) -> bool;
}

struct StoredValue<T: 'static> {
    serializer: Serializer<T>,
    value: T,
}

impl<T: 'static> StoredValue<T> {
    fn new(serializer: Serializer<T>, value: T) -> Self {
        Self { serializer, value }
    }
}

impl<T: 'static> ErasedValue for StoredValue<T> {
    fn as_any(&self) -> &dyn Any {
        &self.value
    }

    fn equals_any(&self, other: &dyn Any) -> bool {
        other
            .downcast_ref::<T>()
            .is_some_and(|other| self.serializer.values_equal(&self.value, other))
    }

    fn copy_shared(&self) -> Rc<dyn ErasedValue> {
        Rc::new(Self::new(
            self.serializer,
            self.serializer.copy_value(&self.value),
        ))
    }

    fn copy_from_erased(&mut self, source: &dyn ErasedValue) -> bool {
        let Some(source) = source.as_any().downcast_ref::<T>() else {
            return false;
        };
        self.serializer.copy_value_from(source, &mut self.value);
        true
    }
}

pub(crate) fn stored_value<T: 'static>(serializer: Serializer<T>, value: T) -> Rc<dyn ErasedValue> {
    Rc::new(StoredValue::new(serializer, value))
}

pub struct DataValue {
    id: AccessorId,
    serializer: SerializerKey,
    value: Rc<dyn ErasedValue>,
}

impl DataValue {
    #[must_use]
    pub fn new<T: 'static>(id: AccessorId, serializer: Serializer<T>, value: T) -> Self {
        Self {
            id,
            serializer: serializer.key(),
            value: stored_value(serializer, value),
        }
    }

    #[must_use]
    pub const fn id(&self) -> AccessorId {
        self.id
    }

    #[must_use]
    pub const fn serializer_id(&self) -> SerializerId {
        self.serializer.id
    }

    #[must_use]
    pub const fn serializer_key(&self) -> SerializerKey {
        self.serializer
    }

    pub fn read<T: 'static>(&self, serializer: Serializer<T>) -> Result<&T, DataValueReadError> {
        if serializer.key() != self.serializer {
            return Err(DataValueReadError::SerializerMismatch {
                expected: self.serializer,
                requested: serializer.key(),
            });
        }
        self.value
            .as_any()
            .downcast_ref::<T>()
            .ok_or(DataValueReadError::ValueTypeMismatch {
                serializer: self.serializer.id,
            })
    }

    pub(crate) fn erased(&self) -> &dyn ErasedValue {
        self.value.as_ref()
    }

    pub(crate) fn shared_value(&self) -> Rc<dyn ErasedValue> {
        Rc::clone(&self.value)
    }

    fn copied_from(id: AccessorId, serializer: SerializerKey, source: &dyn ErasedValue) -> Self {
        Self {
            id,
            serializer,
            value: source.copy_shared(),
        }
    }

    fn refresh_from(
        &mut self,
        id: AccessorId,
        serializer: SerializerKey,
        source: &dyn ErasedValue,
    ) {
        if self.serializer == serializer
            && let Some(target) = Rc::get_mut(&mut self.value)
            && target.copy_from_erased(source)
        {
            self.id = id;
            return;
        }
        *self = Self::copied_from(id, serializer, source);
    }
}

impl Clone for DataValue {
    fn clone(&self) -> Self {
        Self::copied_from(self.id, self.serializer, self.value.as_ref())
    }
}

impl std::fmt::Debug for DataValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataValue")
            .field("id", &self.id)
            .field("serializer", &self.serializer)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataValueReadError {
    SerializerMismatch {
        expected: SerializerKey,
        requested: SerializerKey,
    },
    ValueTypeMismatch {
        serializer: SerializerId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapacityError {
    TooLarge { requested: usize, max: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackError {
    OutputCapacityExceeded { required: usize, capacity: usize },
}

pub struct DataValueBuffer {
    capacity: usize,
    slots: [Option<DataValue>; MAX_DATA_ITEMS],
    order: [AccessorId; MAX_DATA_ITEMS],
    len: usize,
    warmed_slots: usize,
}

impl DataValueBuffer {
    pub fn new(capacity: usize) -> Result<Self, CapacityError> {
        if capacity > MAX_DATA_ITEMS {
            return Err(CapacityError::TooLarge {
                requested: capacity,
                max: MAX_DATA_ITEMS,
            });
        }
        Ok(Self {
            capacity,
            slots: array::from_fn(|_| None),
            order: [AccessorId::ZERO; MAX_DATA_ITEMS],
            len: 0,
            warmed_slots: 0,
        })
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub const fn warmed_slots(&self) -> usize {
        self.warmed_slots
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&DataValue> {
        let id = *self.order.get(index).filter(|_| index < self.len)?;
        self.slots[usize::from(id.get())].as_ref()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &DataValue> {
        self.order[..self.len].iter().map(|id| {
            self.slots[usize::from(id.get())]
                .as_ref()
                .expect("batch order only contains populated slots")
        })
    }

    pub(crate) fn prepare(&mut self, required: usize) -> Result<(), PackError> {
        if required > self.capacity {
            return Err(PackError::OutputCapacityExceeded {
                required,
                capacity: self.capacity,
            });
        }
        self.len = 0;
        Ok(())
    }

    pub(crate) fn clear_batch(&mut self) {
        self.len = 0;
    }

    pub(crate) fn push_copy(
        &mut self,
        id: AccessorId,
        serializer: SerializerKey,
        source: &dyn ErasedValue,
    ) {
        let slot = &mut self.slots[usize::from(id.get())];
        match slot {
            Some(value) => value.refresh_from(id, serializer, source),
            None => {
                *slot = Some(DataValue::copied_from(id, serializer, source));
                self.warmed_slots += 1;
            }
        }
        self.order[self.len] = id;
        self.len += 1;
    }
}

impl std::fmt::Debug for DataValueBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataValueBuffer")
            .field("capacity", &self.capacity)
            .field("len", &self.len)
            .field("warmed_slots", &self.warmed_slots)
            .finish()
    }
}
