use std::cmp::Ordering;
use std::sync::Arc;

use super::identifier::Identifier;

pub const MAX_MODIFIERS_PER_ATTRIBUTE: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AttributeId(u32);

impl AttributeId {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
enum Sanitizer {
    Identity,
    Ranged { minimum: f64, maximum: f64 },
}

#[derive(Debug, Clone, Copy)]
pub struct AttributeDefinition {
    id: AttributeId,
    default_value: f64,
    sanitizer: Sanitizer,
    syncable: bool,
}

impl AttributeDefinition {
    #[must_use]
    pub const fn unbounded(id: AttributeId, default_value: f64, syncable: bool) -> Self {
        Self {
            id,
            default_value,
            sanitizer: Sanitizer::Identity,
            syncable,
        }
    }

    pub fn ranged(
        id: AttributeId,
        default_value: f64,
        minimum: f64,
        maximum: f64,
        syncable: bool,
    ) -> Result<Self, AttributeDefinitionError> {
        if minimum > maximum {
            return Err(AttributeDefinitionError::MinimumAboveMaximum);
        }
        if default_value < minimum {
            return Err(AttributeDefinitionError::DefaultBelowMinimum);
        }
        if default_value > maximum {
            return Err(AttributeDefinitionError::DefaultAboveMaximum);
        }
        Ok(Self {
            id,
            default_value,
            sanitizer: Sanitizer::Ranged { minimum, maximum },
            syncable,
        })
    }

    #[must_use]
    pub const fn id(self) -> AttributeId {
        self.id
    }

    #[must_use]
    pub const fn default_value(self) -> f64 {
        self.default_value
    }

    #[must_use]
    pub const fn is_syncable(self) -> bool {
        self.syncable
    }

    #[must_use]
    pub fn sanitize(self, value: f64) -> f64 {
        match self.sanitizer {
            Sanitizer::Identity => value,
            Sanitizer::Ranged { minimum, maximum } => {
                if value.is_nan() || value < minimum {
                    minimum
                } else {
                    java_min(value, maximum)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeDefinitionError {
    MinimumAboveMaximum,
    DefaultBelowMinimum,
    DefaultAboveMaximum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    AddValue,
    AddMultipliedBase,
    AddMultipliedTotal,
}

impl Operation {
    const fn index(self) -> usize {
        match self {
            Self::AddValue => 0,
            Self::AddMultipliedBase => 1,
            Self::AddMultipliedTotal => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttributeModifier {
    record: Arc<ModifierRecord>,
}

impl AttributeModifier {
    #[must_use]
    pub fn new(id: Identifier, amount: f64, operation: Operation) -> Self {
        Self {
            record: Arc::new(ModifierRecord {
                id,
                amount,
                operation,
            }),
        }
    }

    #[must_use]
    pub fn id(&self) -> &Identifier {
        &self.record.id
    }

    #[must_use]
    pub fn amount(&self) -> f64 {
        self.record.amount
    }

    #[must_use]
    pub fn operation(&self) -> Operation {
        self.record.operation
    }

    #[must_use]
    pub fn is_same_record(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.record, &other.record)
    }
}

#[derive(Debug)]
struct ModifierRecord {
    id: Identifier,
    amount: f64,
    operation: Operation,
}

impl PartialEq for ModifierRecord {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && java_double_bits(self.amount) == java_double_bits(other.amount)
            && self.operation == other.operation
    }
}

impl Eq for ModifierRecord {}

impl PartialEq for AttributeModifier {
    fn eq(&self, other: &Self) -> bool {
        self.record == other.record
    }
}

impl Eq for AttributeModifier {}

#[derive(Debug, Clone)]
pub struct PackedAttribute {
    pub attribute: AttributeId,
    pub base_value: f64,
    pub modifiers: Vec<AttributeModifier>,
}

impl PartialEq for PackedAttribute {
    fn eq(&self, other: &Self) -> bool {
        self.attribute == other.attribute
            && java_double_bits(self.base_value) == java_double_bits(other.base_value)
            && self.modifiers == other.modifiers
    }
}

impl Eq for PackedAttribute {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyEffect {
    notifications: usize,
}

impl DirtyEffect {
    pub const NONE: Self = Self { notifications: 0 };
    pub const ONE: Self = Self { notifications: 1 };

    #[must_use]
    pub const fn notifications(self) -> usize {
        self.notifications
    }

    #[must_use]
    pub const fn changed(self) -> bool {
        self.notifications != 0
    }

    const fn plus(self, other: Self) -> Self {
        Self {
            notifications: self.notifications + other.notifications,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceCapacities {
    pub modifiers: usize,
    pub permanent_modifiers: usize,
    pub add_value_slots: usize,
    pub add_multiplied_base_slots: usize,
    pub add_multiplied_total_slots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeInstanceError {
    CapacityExceedsHardLimit {
        requested: usize,
        maximum: usize,
    },
    AllocationFailed,
    DuplicateModifier {
        id: Identifier,
    },
    ModifierCapacityExceeded {
        capacity: usize,
    },
    OperationCapacityExceeded {
        operation: Operation,
        capacity: usize,
    },
    PackedAttributeMismatch {
        expected: AttributeId,
        actual: AttributeId,
    },
    SourceAttributeMismatch {
        expected: AttributeId,
        actual: AttributeId,
    },
}

#[derive(Debug)]
pub struct AttributeInstance {
    definition: AttributeDefinition,
    base_value: f64,
    modifiers_by_id: Vec<AttributeModifier>,
    permanent_by_id: Vec<AttributeModifier>,
    modifiers_by_operation: [FastutilBucket; 3],
    modifier_capacity: usize,
    value_dirty: bool,
    cached_value: f64,
}

impl AttributeInstance {
    pub fn try_new(
        definition: AttributeDefinition,
        modifier_capacity: usize,
    ) -> Result<Self, AttributeInstanceError> {
        if modifier_capacity > MAX_MODIFIERS_PER_ATTRIBUTE {
            return Err(AttributeInstanceError::CapacityExceedsHardLimit {
                requested: modifier_capacity,
                maximum: MAX_MODIFIERS_PER_ATTRIBUTE,
            });
        }

        let mut modifiers_by_id = Vec::new();
        let mut permanent_by_id = Vec::new();
        let modifiers_by_operation = [
            FastutilBucket::try_new(modifier_capacity)?,
            FastutilBucket::try_new(modifier_capacity)?,
            FastutilBucket::try_new(modifier_capacity)?,
        ];
        reserve(&mut modifiers_by_id, modifier_capacity)?;
        reserve(&mut permanent_by_id, modifier_capacity)?;

        Ok(Self {
            definition,
            base_value: definition.default_value(),
            modifiers_by_id,
            permanent_by_id,
            modifiers_by_operation,
            modifier_capacity,
            value_dirty: true,
            cached_value: 0.0,
        })
    }

    #[must_use]
    pub const fn attribute(&self) -> AttributeId {
        self.definition.id()
    }

    #[must_use]
    pub const fn definition(&self) -> AttributeDefinition {
        self.definition
    }

    #[must_use]
    pub const fn base_value(&self) -> f64 {
        self.base_value
    }

    #[must_use]
    pub const fn is_value_dirty(&self) -> bool {
        self.value_dirty
    }

    #[must_use]
    pub fn capacities(&self) -> InstanceCapacities {
        InstanceCapacities {
            modifiers: self.modifiers_by_id.capacity(),
            permanent_modifiers: self.permanent_by_id.capacity(),
            add_value_slots: self.modifiers_by_operation[Operation::AddValue.index()]
                .entry_capacity(),
            add_multiplied_base_slots: self.modifiers_by_operation
                [Operation::AddMultipliedBase.index()]
            .entry_capacity(),
            add_multiplied_total_slots: self.modifiers_by_operation
                [Operation::AddMultipliedTotal.index()]
            .entry_capacity(),
        }
    }

    pub fn set_base_value(&mut self, base_value: f64) -> DirtyEffect {
        if base_value != self.base_value {
            self.base_value = base_value;
            self.set_dirty()
        } else {
            DirtyEffect::NONE
        }
    }

    pub fn value(&mut self) -> f64 {
        if self.value_dirty {
            self.cached_value = self.calculate_value();
            self.value_dirty = false;
        }
        self.cached_value
    }

    #[must_use]
    pub fn modifier(&self, id: &Identifier) -> Option<AttributeModifier> {
        locate(&self.modifiers_by_id, id)
            .ok()
            .map(|index| self.modifiers_by_id[index].clone())
    }

    #[must_use]
    pub fn has_modifier(&self, id: &Identifier) -> bool {
        locate(&self.modifiers_by_id, id).is_ok()
    }

    pub fn modifiers(&self) -> impl Iterator<Item = AttributeModifier> + '_ {
        self.modifiers_by_id.iter().cloned()
    }

    pub fn permanent_modifiers(&self) -> impl Iterator<Item = AttributeModifier> + '_ {
        self.permanent_by_id.iter().cloned()
    }

    pub fn add_transient_modifier(
        &mut self,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        self.add_modifier(modifier, false)
    }

    pub fn add_or_update_transient_modifier(
        &mut self,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        match locate(&self.modifiers_by_id, modifier.id()) {
            Ok(index) if self.modifiers_by_id[index].is_same_record(&modifier) => {
                Ok(DirtyEffect::NONE)
            }
            Ok(index) => {
                self.preflight_operation_insert(&modifier)?;
                let operation = modifier.operation();
                self.modifiers_by_id[index] = modifier.clone();
                self.modifiers_by_operation[operation.index()].put(modifier);
                Ok(self.set_dirty())
            }
            Err(_) => {
                self.preflight_new_modifier(&modifier)?;
                let operation = modifier.operation();
                self.modifiers_by_id.push(modifier.clone());
                self.modifiers_by_operation[operation.index()].put(modifier);
                Ok(self.set_dirty())
            }
        }
    }

    pub fn add_permanent_modifier(
        &mut self,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        self.add_modifier(modifier, true)
    }

    pub fn add_or_replace_permanent_modifier(
        &mut self,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        self.preflight_replacement(&modifier)?;
        let removed = self.remove_modifier(modifier.id());
        self.insert_new_modifier_unchecked(modifier, true);
        Ok(removed.plus(self.set_dirty()))
    }

    pub(crate) fn replace_transient_modifier(
        &mut self,
        modifier: AttributeModifier,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        self.preflight_replacement(&modifier)?;
        let removed = self.remove_modifier(modifier.id());
        self.insert_new_modifier_unchecked(modifier, false);
        Ok(removed.plus(self.set_dirty()))
    }

    pub fn remove_modifier(&mut self, id: &Identifier) -> DirtyEffect {
        let Ok(index) = locate(&self.modifiers_by_id, id) else {
            return DirtyEffect::NONE;
        };
        let modifier = self.modifiers_by_id.remove(index);
        self.modifiers_by_operation[modifier.operation().index()].remove(id);
        remove_array(&mut self.permanent_by_id, id);
        self.set_dirty()
    }

    pub fn remove_all_modifiers(&mut self) -> DirtyEffect {
        let mut result = DirtyEffect::NONE;
        while let Some(modifier) = self.modifiers_by_id.first() {
            let id = modifier.id().clone();
            result = result.plus(self.remove_modifier(&id));
        }
        result
    }

    pub fn replace_from(&mut self, other: &Self) -> Result<DirtyEffect, AttributeInstanceError> {
        if self.attribute() != other.attribute() {
            return Err(AttributeInstanceError::SourceAttributeMismatch {
                expected: self.attribute(),
                actual: other.attribute(),
            });
        }
        self.preflight_copy(other)?;
        self.base_value = other.base_value;
        copy_slice(&mut self.modifiers_by_id, &other.modifiers_by_id);
        copy_slice(&mut self.permanent_by_id, &other.permanent_by_id);
        for (target, source) in self
            .modifiers_by_operation
            .iter_mut()
            .zip(&other.modifiers_by_operation)
        {
            target.put_all_from(source);
        }
        Ok(self.set_dirty())
    }

    pub fn try_pack(&self) -> Result<PackedAttribute, AttributeInstanceError> {
        let mut modifiers = Vec::new();
        reserve(&mut modifiers, self.permanent_by_id.len())?;
        modifiers.extend_from_slice(&self.permanent_by_id);
        Ok(PackedAttribute {
            attribute: self.attribute(),
            base_value: self.base_value,
            modifiers,
        })
    }

    pub fn apply_packed(
        &mut self,
        packed: &PackedAttribute,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        self.preflight_packed(packed)?;
        self.base_value = packed.base_value;
        for modifier in &packed.modifiers {
            self.overlay_permanent_unchecked(modifier.clone());
        }
        Ok(self.set_dirty())
    }

    pub(crate) fn preflight_packed(
        &self,
        packed: &PackedAttribute,
    ) -> Result<(), AttributeInstanceError> {
        if packed.attribute != self.attribute() {
            return Err(AttributeInstanceError::PackedAttributeMismatch {
                expected: self.attribute(),
                actual: packed.attribute,
            });
        }

        let new_canonical = count_new_ids(&self.modifiers_by_id, &packed.modifiers, None);
        if self.modifiers_by_id.len() + new_canonical > self.modifier_capacity {
            return Err(AttributeInstanceError::ModifierCapacityExceeded {
                capacity: self.modifier_capacity,
            });
        }
        for operation in [
            Operation::AddValue,
            Operation::AddMultipliedBase,
            Operation::AddMultipliedTotal,
        ] {
            let bucket = &self.modifiers_by_operation[operation.index()];
            let new_slots = count_new_bucket_ids(bucket, &packed.modifiers, operation);
            if bucket.len() + new_slots > self.modifier_capacity {
                return Err(AttributeInstanceError::OperationCapacityExceeded {
                    operation,
                    capacity: self.modifier_capacity,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn try_clone_with_capacity(
        &self,
        capacity: usize,
    ) -> Result<Self, AttributeInstanceError> {
        let mut result = Self::try_new(self.definition, capacity)?;
        result.replace_from(self)?;
        Ok(result)
    }

    pub(crate) fn cached_template_value(&self) -> f64 {
        debug_assert!(!self.value_dirty);
        self.cached_value
    }

    fn add_modifier(
        &mut self,
        modifier: AttributeModifier,
        permanent: bool,
    ) -> Result<DirtyEffect, AttributeInstanceError> {
        if self.has_modifier(modifier.id()) {
            return Err(AttributeInstanceError::DuplicateModifier {
                id: modifier.id().clone(),
            });
        }
        self.preflight_new_modifier(&modifier)?;
        self.insert_new_modifier_unchecked(modifier, permanent);
        Ok(self.set_dirty())
    }

    fn preflight_new_modifier(
        &self,
        modifier: &AttributeModifier,
    ) -> Result<(), AttributeInstanceError> {
        if self.modifiers_by_id.len() == self.modifier_capacity {
            return Err(AttributeInstanceError::ModifierCapacityExceeded {
                capacity: self.modifier_capacity,
            });
        }
        self.preflight_operation_insert(modifier)?;
        Ok(())
    }

    fn preflight_operation_insert(
        &self,
        modifier: &AttributeModifier,
    ) -> Result<(), AttributeInstanceError> {
        let operation = modifier.operation();
        let bucket = &self.modifiers_by_operation[operation.index()];
        if !bucket.contains(modifier.id()) && bucket.len() == self.modifier_capacity {
            return Err(AttributeInstanceError::OperationCapacityExceeded {
                operation,
                capacity: self.modifier_capacity,
            });
        }
        Ok(())
    }

    fn preflight_replacement(
        &self,
        modifier: &AttributeModifier,
    ) -> Result<(), AttributeInstanceError> {
        let existing = self.modifier(modifier.id());
        if existing.is_none() {
            return self.preflight_new_modifier(modifier);
        }
        let operation = modifier.operation();
        let bucket = &self.modifiers_by_operation[operation.index()];
        let replacement_frees_target =
            existing.is_some_and(|current| current.operation() == operation);
        if !bucket.contains(modifier.id())
            && bucket.len() == self.modifier_capacity
            && !replacement_frees_target
        {
            return Err(AttributeInstanceError::OperationCapacityExceeded {
                operation,
                capacity: self.modifier_capacity,
            });
        }
        Ok(())
    }

    pub(crate) fn preflight_copy(&self, other: &Self) -> Result<(), AttributeInstanceError> {
        if other.modifiers_by_id.len() > self.modifier_capacity {
            return Err(AttributeInstanceError::ModifierCapacityExceeded {
                capacity: self.modifier_capacity,
            });
        }
        for operation in [
            Operation::AddValue,
            Operation::AddMultipliedBase,
            Operation::AddMultipliedTotal,
        ] {
            if other.modifiers_by_operation[operation.index()].len() > self.modifier_capacity {
                return Err(AttributeInstanceError::OperationCapacityExceeded {
                    operation,
                    capacity: self.modifier_capacity,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn permanent_slice(&self) -> &[AttributeModifier] {
        &self.permanent_by_id
    }

    fn insert_new_modifier_unchecked(&mut self, modifier: AttributeModifier, permanent: bool) {
        let operation = modifier.operation();
        upsert_array(&mut self.modifiers_by_id, modifier.clone());
        self.modifiers_by_operation[operation.index()].put(modifier.clone());
        if permanent {
            upsert_array(&mut self.permanent_by_id, modifier);
        }
    }

    fn overlay_permanent_unchecked(&mut self, modifier: AttributeModifier) {
        let operation = modifier.operation();
        upsert_array(&mut self.modifiers_by_id, modifier.clone());
        self.modifiers_by_operation[operation.index()].put(modifier.clone());
        upsert_array(&mut self.permanent_by_id, modifier);
    }

    fn calculate_value(&self) -> f64 {
        let mut base = self.base_value;
        for modifier in self.modifiers_by_operation[Operation::AddValue.index()].values() {
            base += modifier.amount();
        }

        let mut result = base;
        for modifier in self.modifiers_by_operation[Operation::AddMultipliedBase.index()].values() {
            result += base * modifier.amount();
        }
        for modifier in self.modifiers_by_operation[Operation::AddMultipliedTotal.index()].values()
        {
            result *= 1.0 + modifier.amount();
        }
        self.definition.sanitize(result)
    }

    fn set_dirty(&mut self) -> DirtyEffect {
        self.value_dirty = true;
        DirtyEffect::ONE
    }
}

const FASTUTIL_DEFAULT_TABLE_SIZE: usize = 32;
const FASTUTIL_LOAD_NUMERATOR: usize = 3;
const FASTUTIL_LOAD_DENOMINATOR: usize = 4;

#[derive(Debug)]
struct FastutilBucket {
    slots: Vec<Option<AttributeModifier>>,
    scratch: Vec<Option<AttributeModifier>>,
    table_size: usize,
    size: usize,
    entry_capacity: usize,
}

impl FastutilBucket {
    fn try_new(entry_capacity: usize) -> Result<Self, AttributeInstanceError> {
        let maximum_table_size =
            FASTUTIL_DEFAULT_TABLE_SIZE.max(fastutil_array_size(entry_capacity.saturating_add(1)));
        let slot_count = maximum_table_size + 1;
        let slots = empty_slots(slot_count)?;
        let scratch = empty_slots(slot_count)?;
        Ok(Self {
            slots,
            scratch,
            table_size: FASTUTIL_DEFAULT_TABLE_SIZE,
            size: 0,
            entry_capacity,
        })
    }

    const fn len(&self) -> usize {
        self.size
    }

    const fn entry_capacity(&self) -> usize {
        self.entry_capacity
    }

    fn contains(&self, id: &Identifier) -> bool {
        self.find(id).is_ok()
    }

    fn values(&self) -> impl Iterator<Item = &AttributeModifier> {
        self.slots[..self.table_size]
            .iter()
            .rev()
            .filter_map(Option::as_ref)
    }

    fn put(&mut self, modifier: AttributeModifier) {
        match self.find(modifier.id()) {
            Ok(position) => self.slots[position] = Some(modifier),
            Err(position) => {
                debug_assert!(self.size < self.entry_capacity);
                self.slots[position] = Some(modifier);
                let previous_size = self.size;
                self.size += 1;
                if previous_size >= fastutil_max_fill(self.table_size) {
                    self.rehash(fastutil_array_size(self.size + 1));
                }
            }
        }
    }

    fn remove(&mut self, id: &Identifier) -> Option<AttributeModifier> {
        let position = self.find(id).ok()?;
        let removed = self.slots[position].take();
        self.size -= 1;
        self.shift_keys(position);
        if self.table_size > FASTUTIL_DEFAULT_TABLE_SIZE
            && self.size < fastutil_max_fill(self.table_size) / 4
        {
            self.rehash(self.table_size / 2);
        }
        removed
    }

    fn put_all_from(&mut self, source: &Self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.table_size = FASTUTIL_DEFAULT_TABLE_SIZE;
        self.size = 0;
        let needed = fastutil_array_size(source.size);
        if needed > self.table_size {
            self.rehash(needed);
        }
        for modifier in source.values() {
            self.put(modifier.clone());
        }
    }

    fn find(&self, id: &Identifier) -> Result<usize, usize> {
        let mask = self.table_size - 1;
        let mut position = fastutil_mix(id.java_hash_code()) & mask;
        loop {
            match &self.slots[position] {
                Some(modifier) if modifier.id() == id => return Ok(position),
                Some(_) => position = (position + 1) & mask,
                None => return Err(position),
            }
        }
    }

    fn shift_keys(&mut self, mut last: usize) {
        let mask = self.table_size - 1;
        loop {
            let mut position = (last + 1) & mask;
            loop {
                let Some(modifier) = &self.slots[position] else {
                    self.slots[last] = None;
                    return;
                };
                let slot = fastutil_mix(modifier.id().java_hash_code()) & mask;
                let must_shift = if last <= position {
                    last >= slot || slot > position
                } else {
                    last >= slot && slot > position
                };
                if must_shift {
                    break;
                }
                position = (position + 1) & mask;
            }
            self.slots[last] = self.slots[position].take();
            last = position;
        }
    }

    fn rehash(&mut self, new_table_size: usize) {
        debug_assert!(new_table_size < self.slots.len());
        for slot in &mut self.scratch {
            *slot = None;
        }

        let mask = new_table_size - 1;
        let mut scan = self.table_size;
        for _ in 0..self.size {
            let modifier = loop {
                scan -= 1;
                if let Some(modifier) = self.slots[scan].take() {
                    break modifier;
                }
            };
            let mut position = fastutil_mix(modifier.id().java_hash_code()) & mask;
            while self.scratch[position].is_some() {
                position = (position + 1) & mask;
            }
            self.scratch[position] = Some(modifier);
        }
        std::mem::swap(&mut self.slots, &mut self.scratch);
        self.table_size = new_table_size;
    }
}

fn empty_slots(capacity: usize) -> Result<Vec<Option<AttributeModifier>>, AttributeInstanceError> {
    let mut slots = Vec::new();
    slots
        .try_reserve_exact(capacity)
        .map_err(|_| AttributeInstanceError::AllocationFailed)?;
    slots.resize_with(capacity, || None);
    Ok(slots)
}

fn fastutil_mix(hash: i32) -> usize {
    let mixed = (hash as u32).wrapping_mul(0x9e37_79b9);
    (mixed ^ (mixed >> 16)) as usize
}

fn fastutil_array_size(expected: usize) -> usize {
    let required = expected
        .saturating_mul(FASTUTIL_LOAD_DENOMINATOR)
        .div_ceil(FASTUTIL_LOAD_NUMERATOR);
    required.max(2).next_power_of_two()
}

fn fastutil_max_fill(table_size: usize) -> usize {
    table_size
        .saturating_mul(FASTUTIL_LOAD_NUMERATOR)
        .div_ceil(FASTUTIL_LOAD_DENOMINATOR)
        .min(table_size - 1)
}

fn reserve(
    values: &mut Vec<AttributeModifier>,
    capacity: usize,
) -> Result<(), AttributeInstanceError> {
    values
        .try_reserve_exact(capacity)
        .map_err(|_| AttributeInstanceError::AllocationFailed)
}

fn locate(values: &[AttributeModifier], id: &Identifier) -> Result<usize, usize> {
    values
        .iter()
        .position(|modifier| modifier.id() == id)
        .ok_or(values.len())
}

fn upsert_array(values: &mut Vec<AttributeModifier>, modifier: AttributeModifier) {
    match locate(values, modifier.id()) {
        Ok(index) => values[index] = modifier,
        Err(_) => values.push(modifier),
    }
}

fn remove_array(values: &mut Vec<AttributeModifier>, id: &Identifier) {
    if let Ok(index) = locate(values, id) {
        values.remove(index);
    }
}

fn copy_slice(target: &mut Vec<AttributeModifier>, source: &[AttributeModifier]) {
    target.clear();
    target.extend_from_slice(source);
}

fn count_new_ids(
    existing: &[AttributeModifier],
    incoming: &[AttributeModifier],
    operation: Option<Operation>,
) -> usize {
    incoming
        .iter()
        .enumerate()
        .filter(|(index, modifier)| {
            operation.is_none_or(|expected| modifier.operation() == expected)
                && locate(existing, modifier.id()).is_err()
                && !incoming[..*index].iter().any(|previous| {
                    previous.id() == modifier.id()
                        && operation.is_none_or(|expected| previous.operation() == expected)
                })
        })
        .count()
}

fn count_new_bucket_ids(
    existing: &FastutilBucket,
    incoming: &[AttributeModifier],
    operation: Operation,
) -> usize {
    incoming
        .iter()
        .enumerate()
        .filter(|(index, modifier)| {
            modifier.operation() == operation
                && !existing.contains(modifier.id())
                && !incoming[..*index].iter().any(|previous| {
                    previous.operation() == operation && previous.id() == modifier.id()
                })
        })
        .count()
}

fn java_double_bits(value: f64) -> u64 {
    if value.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        value.to_bits()
    }
}

fn java_min(left: f64, right: f64) -> f64 {
    if left.is_nan() {
        return left;
    }
    if right.is_nan() {
        return right;
    }
    if left == 0.0 && right == 0.0 {
        return if left.is_sign_negative() || right.is_sign_negative() {
            -0.0
        } else {
            0.0
        };
    }
    match left.partial_cmp(&right) {
        Some(Ordering::Less) => left,
        Some(Ordering::Equal | Ordering::Greater) | None => right,
    }
}
