use std::sync::Arc;

use super::EquipmentSlot;

/// Vanilla's default NBT quota (`NbtAccounter.DEFAULT_NBT_QUOTA`). The caller
/// supplies opaque canonical component bytes; this kernel does not parse them.
pub const MAX_STACK_COMPONENT_SERIALIZED_BYTES: usize = 2_097_152;
pub const MAX_STACK_MUTATION_OPS: usize = 24;

const COMPONENT_ENTRY_OVERHEAD: usize = size_of::<u32>() + size_of::<u32>();
const TYPED_I32_COMPONENT_SIZE: usize = size_of::<u32>() + size_of::<i32>();
const TYPED_BOOL_COMPONENT_SIZE: usize = size_of::<u32>() + size_of::<u8>();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemKey(u64);

impl ItemKey {
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentKey(u32);

impl ComponentKey {
    pub const fn new(raw: u32) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentBytes(Arc<[u8]>);

impl ComponentBytes {
    pub fn new(value: &[u8]) -> Result<Self, ComponentError> {
        check_component_size(value.len())?;
        Ok(Self(Arc::from(value)))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentEntry {
    pub key: ComponentKey,
    pub value: ComponentBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackComponents {
    damage: Option<i32>,
    max_damage: Option<i32>,
    unbreakable: bool,
    entries: Vec<ComponentEntry>,
    encoded_size: usize,
}

impl StackComponents {
    pub const fn new() -> Self {
        Self {
            damage: None,
            max_damage: None,
            unbreakable: false,
            entries: Vec::new(),
            encoded_size: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
            + usize::from(self.damage.is_some())
            + usize::from(self.max_damage.is_some())
            + usize::from(self.unbreakable)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub const fn encoded_size(&self) -> usize {
        self.encoded_size
    }

    pub fn entry(&self, index: usize) -> Option<&ComponentEntry> {
        self.entries.get(index)
    }

    pub fn get(&self, key: ComponentKey) -> Option<&ComponentBytes> {
        self.find(key).ok().map(|index| &self.entries[index].value)
    }

    pub fn set(
        &mut self,
        key: ComponentKey,
        value: ComponentBytes,
    ) -> Result<Option<ComponentBytes>, ComponentError> {
        let contribution = component_entry_size(&value);
        match self.find(key) {
            Ok(index) => {
                let previous_size = component_entry_size(&self.entries[index].value);
                let next_size = self.encoded_size - previous_size + contribution;
                check_component_size(next_size)?;
                self.encoded_size = next_size;
                let previous = std::mem::replace(&mut self.entries[index].value, value);
                Ok(Some(previous))
            }
            Err(index) => {
                let next_size = self.encoded_size.saturating_add(contribution);
                check_component_size(next_size)?;
                self.entries.insert(index, ComponentEntry { key, value });
                self.encoded_size = next_size;
                Ok(None)
            }
        }
    }

    pub fn remove(&mut self, key: ComponentKey) -> Option<ComponentBytes> {
        let index = self.find(key).ok()?;
        let entry = self.entries.remove(index);
        self.encoded_size -= component_entry_size(&entry.value);
        Some(entry.value)
    }

    pub const fn damage(&self) -> Option<i32> {
        self.damage
    }

    pub fn set_damage(&mut self, damage: Option<i32>) -> Result<(), ComponentError> {
        let normalized = damage.map(|value| match self.max_damage {
            Some(max_damage) => value.clamp(0, max_damage),
            None => value.max(0),
        });
        let next_size = replace_optional_size(
            self.encoded_size,
            self.damage.is_some(),
            normalized.is_some(),
            TYPED_I32_COMPONENT_SIZE,
        );
        check_component_size(next_size)?;
        self.damage = normalized;
        self.encoded_size = next_size;
        Ok(())
    }

    pub const fn max_damage(&self) -> Option<i32> {
        self.max_damage
    }

    pub fn set_max_damage(&mut self, max_damage: Option<i32>) -> Result<(), ComponentError> {
        if max_damage.is_some_and(|value| value <= 0) {
            return Err(ComponentError::NonPositiveMaxDamage);
        }
        let next_size = replace_optional_size(
            self.encoded_size,
            self.max_damage.is_some(),
            max_damage.is_some(),
            TYPED_I32_COMPONENT_SIZE,
        );
        check_component_size(next_size)?;
        self.max_damage = max_damage;
        if let (Some(damage), Some(max_damage)) = (self.damage, max_damage) {
            self.damage = Some(damage.clamp(0, max_damage));
        }
        self.encoded_size = next_size;
        Ok(())
    }

    pub const fn unbreakable(&self) -> bool {
        self.unbreakable
    }

    pub fn set_unbreakable(&mut self, unbreakable: bool) -> Result<(), ComponentError> {
        let next_size = replace_optional_size(
            self.encoded_size,
            self.unbreakable,
            unbreakable,
            TYPED_BOOL_COMPONENT_SIZE,
        );
        check_component_size(next_size)?;
        self.unbreakable = unbreakable;
        self.encoded_size = next_size;
        Ok(())
    }

    fn find(&self, key: ComponentKey) -> Result<usize, usize> {
        self.entries.binary_search_by_key(&key, |entry| entry.key)
    }
}

impl Default for StackComponents {
    fn default() -> Self {
        Self::new()
    }
}

fn component_entry_size(value: &ComponentBytes) -> usize {
    COMPONENT_ENTRY_OVERHEAD.saturating_add(value.len())
}

fn replace_optional_size(
    current: usize,
    was_present: bool,
    is_present: bool,
    contribution: usize,
) -> usize {
    match (was_present, is_present) {
        (false, true) => current.saturating_add(contribution),
        (true, false) => current - contribution,
        _ => current,
    }
}

fn check_component_size(encoded_size: usize) -> Result<(), ComponentError> {
    if encoded_size > MAX_STACK_COMPONENT_SERIALIZED_BYTES {
        Err(ComponentError::UnsupportedStack {
            encoded_size,
            max: MAX_STACK_COMPONENT_SERIALIZED_BYTES,
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentError {
    UnsupportedStack { encoded_size: usize, max: usize },
    NonPositiveMaxDamage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStackState {
    item: Option<ItemKey>,
    count: u32,
    components: StackComponents,
}

impl ItemStackState {
    pub const EMPTY: Self = Self {
        item: None,
        count: 0,
        components: StackComponents::new(),
    };

    pub fn occupied(
        item: ItemKey,
        count: u32,
        components: StackComponents,
    ) -> Result<Self, StackStateError> {
        if count == 0 {
            Err(StackStateError::ZeroCount)
        } else {
            Ok(Self {
                item: Some(item),
                count,
                components,
            })
        }
    }

    pub const fn is_empty(&self) -> bool {
        self.item.is_none() || self.count == 0
    }

    pub const fn item_key(&self) -> Option<ItemKey> {
        if self.is_empty() { None } else { self.item }
    }

    pub const fn count(&self) -> u32 {
        if self.is_empty() { 0 } else { self.count }
    }

    pub const fn components(&self) -> &StackComponents {
        &self.components
    }

    pub fn components_mut(&mut self) -> &mut StackComponents {
        &mut self.components
    }

    pub fn same_item_same_components(&self, other: &Self) -> bool {
        if self.is_empty() || other.is_empty() {
            return self.is_empty() && other.is_empty();
        }
        self.item == other.item && self.components == other.components
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.count() == other.count() && self.same_item_same_components(other)
    }

    pub const fn is_damageable(&self) -> bool {
        !self.is_empty()
            && self.components.max_damage.is_some()
            && self.components.damage.is_some()
            && !self.components.unbreakable
    }

    pub fn damage_value(&self) -> Option<i32> {
        if !self.is_damageable() {
            return None;
        }
        let max = self.components.max_damage.expect("damageable max damage");
        Some(
            self.components
                .damage
                .expect("damageable damage")
                .clamp(0, max),
        )
    }

    pub fn max_damage(&self) -> Option<i32> {
        self.is_damageable().then_some(self.components.max_damage?)
    }

    pub fn is_broken(&self) -> bool {
        matches!((self.damage_value(), self.max_damage()), (Some(damage), Some(max)) if damage >= max)
    }

    pub fn limit_for(self, slot: EquipmentSlot) -> StackLimit {
        if self.is_empty() {
            return StackLimit {
                equipped: Self::EMPTY,
                remainder: Self::EMPTY,
            };
        }
        let limit = slot.count_limit();
        if limit == 0 || self.count <= limit {
            return StackLimit {
                equipped: self,
                remainder: Self::EMPTY,
            };
        }
        let mut equipped = self.clone();
        equipped.count = limit;
        let mut remainder = self;
        remainder.count -= limit;
        StackLimit {
            equipped,
            remainder,
        }
    }

    pub fn shrink(&mut self, amount: u32) -> u32 {
        let removed = amount.min(self.count());
        self.count = self.count.saturating_sub(removed);
        removed
    }

    pub(super) fn set_count(&mut self, count: u32) {
        if self.item.is_some() {
            self.count = count;
        }
    }

    pub(super) fn set_damage_clamped(&mut self, damage: i32) -> Result<(), ComponentError> {
        if let Some(max) = self.components.max_damage {
            self.components.set_damage(Some(damage.clamp(0, max)))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackStateError {
    ZeroCount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackLimit {
    pub equipped: ItemStackState,
    pub remainder: ItemStackState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlotRevision(u64);

impl SlotRevision {
    pub const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackMutationOp {
    SetCount(u32),
    Shrink(u32),
    SetDamage(Option<i32>),
    SetMaxDamage(Option<i32>),
    SetUnbreakable(bool),
    SetComponent {
        key: ComponentKey,
        value: ComponentBytes,
    },
    RemoveComponent(ComponentKey),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackMutation {
    expected_revision: SlotRevision,
    operations: Vec<StackMutationOp>,
}

impl StackMutation {
    pub const fn new(expected_revision: SlotRevision) -> Self {
        Self {
            expected_revision,
            operations: Vec::new(),
        }
    }

    pub fn push(&mut self, operation: StackMutationOp) -> Result<(), StackMutationError> {
        if self.operations.len() == MAX_STACK_MUTATION_OPS {
            return Err(StackMutationError::TooManyOperations);
        }
        self.operations.push(operation);
        Ok(())
    }

    pub const fn expected_revision(&self) -> SlotRevision {
        self.expected_revision
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn operation(&self, index: usize) -> Option<&StackMutationOp> {
        self.operations.get(index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackMutationError {
    TooManyOperations,
}
