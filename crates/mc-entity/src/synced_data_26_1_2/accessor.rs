use std::marker::PhantomData;

pub const MAX_ACCESSOR_ID: u16 = 254;
pub const MAX_DATA_ITEMS: usize = MAX_ACCESSOR_ID as usize + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccessorId(u8);

impl AccessorId {
    pub const ZERO: Self = Self(0);

    pub fn new(id: u16) -> Result<Self, AccessorIdError> {
        if id > MAX_ACCESSOR_ID {
            return Err(AccessorIdError::OutOfRange {
                requested: id,
                max: MAX_ACCESSOR_ID,
            });
        }
        Ok(Self(
            u8::try_from(id).expect("validated accessor ID fits u8"),
        ))
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessorIdError {
    OutOfRange { requested: u16, max: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SerializerId(u32);

impl SerializerId {
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SerializerIdentity(u64);

impl SerializerIdentity {
    /// Creates the stable identity of one registered serializer policy.
    ///
    /// The caller must not reuse a value for a different equality/copy policy.
    #[must_use]
    pub const fn new(identity: u64) -> Self {
        Self(identity)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SerializerKey {
    pub id: SerializerId,
    pub identity: SerializerIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaId(u64);

impl SchemaId {
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn accessor<T: 'static>(
        self,
        id: AccessorId,
        serializer: Serializer<T>,
    ) -> Accessor<T> {
        Accessor {
            schema: self,
            id,
            serializer,
        }
    }
}

pub struct Serializer<T: 'static> {
    key: SerializerKey,
    equals: fn(&T, &T) -> bool,
    copy: fn(&T) -> T,
    copy_from: fn(&T, &mut T),
    marker: PhantomData<fn() -> T>,
}

impl<T: 'static> Serializer<T> {
    #[must_use]
    pub const fn new(
        id: SerializerId,
        identity: SerializerIdentity,
        equals: fn(&T, &T) -> bool,
        copy: fn(&T) -> T,
        copy_from: fn(&T, &mut T),
    ) -> Self {
        Self {
            key: SerializerKey { id, identity },
            equals,
            copy,
            copy_from,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn cloned(
        id: SerializerId,
        identity: SerializerIdentity,
        equals: fn(&T, &T) -> bool,
    ) -> Self
    where
        T: Clone,
    {
        Self::new(
            id,
            identity,
            equals,
            clone_value::<T>,
            clone_value_from::<T>,
        )
    }

    #[must_use]
    pub const fn id(self) -> SerializerId {
        self.key.id
    }

    #[must_use]
    pub const fn identity(self) -> SerializerIdentity {
        self.key.identity
    }

    #[must_use]
    pub const fn key(self) -> SerializerKey {
        self.key
    }

    pub(crate) fn values_equal(self, left: &T, right: &T) -> bool {
        (self.equals)(left, right)
    }

    pub(crate) fn copy_value(self, value: &T) -> T {
        (self.copy)(value)
    }

    pub(crate) fn copy_value_from(self, source: &T, target: &mut T) {
        (self.copy_from)(source, target);
    }
}

impl<T: 'static> Clone for Serializer<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Serializer<T> {}

impl<T: 'static> std::fmt::Debug for Serializer<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Serializer")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

fn clone_value<T: Clone>(value: &T) -> T {
    value.clone()
}

fn clone_value_from<T: Clone>(source: &T, target: &mut T) {
    target.clone_from(source);
}

pub struct Accessor<T: 'static> {
    schema: SchemaId,
    id: AccessorId,
    serializer: Serializer<T>,
}

impl<T: 'static> Accessor<T> {
    #[must_use]
    pub const fn id(self) -> AccessorId {
        self.id
    }

    #[must_use]
    pub const fn serializer_id(self) -> SerializerId {
        self.serializer.id()
    }

    #[must_use]
    pub const fn serializer_key(self) -> SerializerKey {
        self.serializer.key()
    }

    #[must_use]
    pub const fn schema(self) -> SchemaId {
        self.schema
    }

    pub(crate) const fn serializer(self) -> Serializer<T> {
        self.serializer
    }
}

impl<T: 'static> Clone for Accessor<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Accessor<T> {}

impl<T: 'static, U: 'static> PartialEq<Accessor<U>> for Accessor<T> {
    fn eq(&self, other: &Accessor<U>) -> bool {
        self.id == other.id
    }
}

impl<T: 'static> Eq for Accessor<T> {}

impl<T: 'static> std::hash::Hash for Accessor<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T: 'static> std::fmt::Debug for Accessor<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Accessor")
            .field("schema", &self.schema)
            .field("id", &self.id)
            .field("serializer", &self.serializer.id())
            .finish()
    }
}

/// Java `Float.equals` semantics used by vanilla's change/default checks.
#[must_use]
pub fn java_f32_equals(left: &f32, right: &f32) -> bool {
    (left.is_nan() && right.is_nan()) || left.to_bits() == right.to_bits()
}
