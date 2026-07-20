use super::publication::PublicationAdmissionCandidate;
use super::{
    ComponentError, EQUIPMENT_SLOT_COUNT, EquipmentSlot, ItemStackState, SlotRevision,
    StackMutation, StackMutationOp,
};

pub const DEFAULT_EQUIPMENT_DROP_CHANCE: f32 = 0.085;
pub const GUARANTEED_EQUIPMENT_DROP_CHANCE: f32 = 2.0;
pub const PRESERVED_EQUIPMENT_DROP_CHANCE_THRESHOLD: f32 = 1.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceCommand {
    pub slot: EquipmentSlot,
    pub expected_revision: SlotRevision,
    pub replacement: ItemStackState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceOutcome {
    Unchanged {
        revision: SlotRevision,
    },
    Replaced {
        previous: ItemStackState,
        current: ItemStackState,
        revision: SlotRevision,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandSwapPrecondition {
    pub main_hand: SlotRevision,
    pub off_hand: SlotRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandSwapOutcome {
    Unchanged,
    Swapped {
        main_hand_revision: SlotRevision,
        off_hand_revision: SlotRevision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EquipmentPrecondition {
    pub slot: EquipmentSlot,
    pub revision: SlotRevision,
    pub stack: ItemStackState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquipmentMutationError {
    StaleRevision {
        slot: EquipmentSlot,
        expected: SlotRevision,
        actual: SlotRevision,
    },
    EmptyStack {
        slot: EquipmentSlot,
    },
    Component(ComponentError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationApplyOutcome {
    pub previous_revision: SlotRevision,
    pub current_revision: SlotRevision,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DropChanceUpdate {
    Unchanged { current: f32 },
    Changed { previous: f32, current: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropChanceError {
    Negative { slot: EquipmentSlot },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistenceEntry<T> {
    pub slot: EquipmentSlot,
    pub value: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistenceEntries<T> {
    entries: Vec<PersistenceEntry<T>>,
}

impl<T> PersistenceEntries<T> {
    pub(super) const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(super) fn push(&mut self, entry: PersistenceEntry<T>) {
        assert!(
            self.entries.len() < EQUIPMENT_SLOT_COUNT,
            "bounded slot entry overflow"
        );
        self.entries.push(entry);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub const fn capacity(&self) -> usize {
        EQUIPMENT_SLOT_COUNT
    }

    pub fn get(&self, index: usize) -> Option<&PersistenceEntry<T>> {
        self.entries.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &PersistenceEntry<T>> {
        self.entries.iter()
    }

    pub(super) fn into_iter(self) -> impl Iterator<Item = PersistenceEntry<T>> {
        self.entries.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentPersistenceProjection {
    equipment: PersistenceEntries<ItemStackState>,
    drop_chances: PersistenceEntries<f32>,
    persistence_required: bool,
}

impl EquipmentPersistenceProjection {
    pub const fn equipment(&self) -> &PersistenceEntries<ItemStackState> {
        &self.equipment
    }

    pub const fn drop_chances(&self) -> &PersistenceEntries<f32> {
        &self.drop_chances
    }

    pub fn writes_equipment(&self) -> bool {
        !self.equipment.is_empty()
    }

    pub fn writes_drop_chances(&self) -> bool {
        !self.drop_chances.is_empty()
    }

    pub const fn persistence_required(&self) -> bool {
        self.persistence_required
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EquipmentState {
    pub(super) items: [ItemStackState; EQUIPMENT_SLOT_COUNT],
    pub(super) revisions: [SlotRevision; EQUIPMENT_SLOT_COUNT],
    pub(super) published_items: [ItemStackState; EQUIPMENT_SLOT_COUNT],
    drop_chances: [f32; EQUIPMENT_SLOT_COUNT],
    persistence_required: bool,
    pub(super) pending_publication: Option<PublicationAdmissionCandidate>,
    pub(super) next_publication_token: u64,
}

impl EquipmentState {
    pub fn new() -> Self {
        Self {
            items: core::array::from_fn(|_| ItemStackState::EMPTY),
            revisions: [SlotRevision::INITIAL; EQUIPMENT_SLOT_COUNT],
            published_items: core::array::from_fn(|_| ItemStackState::EMPTY),
            drop_chances: [DEFAULT_EQUIPMENT_DROP_CHANCE; EQUIPMENT_SLOT_COUNT],
            persistence_required: false,
            pending_publication: None,
            next_publication_token: 1,
        }
    }

    pub const fn capacity(&self) -> usize {
        EQUIPMENT_SLOT_COUNT
    }

    pub const fn get(&self, slot: EquipmentSlot) -> &ItemStackState {
        &self.items[slot.ordinal()]
    }

    pub const fn revision(&self, slot: EquipmentSlot) -> SlotRevision {
        self.revisions[slot.ordinal()]
    }

    pub const fn published(&self, slot: EquipmentSlot) -> &ItemStackState {
        &self.published_items[slot.ordinal()]
    }

    pub fn precondition(&self, slot: EquipmentSlot) -> EquipmentPrecondition {
        EquipmentPrecondition {
            slot,
            revision: self.revision(slot),
            stack: self.get(slot).clone(),
        }
    }

    pub fn set(&mut self, slot: EquipmentSlot, stack: ItemStackState) -> ItemStackState {
        let previous = self.get(slot).clone();
        if !previous.matches(&stack) {
            self.items[slot.ordinal()] = stack;
            self.revisions[slot.ordinal()] = self.revision(slot).next();
        }
        previous
    }

    pub const fn main_hand(&self) -> &ItemStackState {
        self.get(EquipmentSlot::MainHand)
    }

    pub const fn off_hand(&self) -> &ItemStackState {
        self.get(EquipmentSlot::OffHand)
    }

    pub const fn hand_items(&self) -> [&ItemStackState; 2] {
        [self.main_hand(), self.off_hand()]
    }

    pub const fn humanoid_armor(&self) -> [&ItemStackState; 4] {
        [
            self.get(EquipmentSlot::Feet),
            self.get(EquipmentSlot::Legs),
            self.get(EquipmentSlot::Chest),
            self.get(EquipmentSlot::Head),
        ]
    }

    pub const fn body(&self) -> &ItemStackState {
        self.get(EquipmentSlot::Body)
    }

    pub const fn saddle(&self) -> &ItemStackState {
        self.get(EquipmentSlot::Saddle)
    }

    pub fn replace_if(
        &mut self,
        command: ReplaceCommand,
    ) -> Result<ReplaceOutcome, EquipmentMutationError> {
        self.check_revision(command.slot, command.expected_revision)?;
        let previous = self.get(command.slot).clone();
        if previous.matches(&command.replacement) {
            return Ok(ReplaceOutcome::Unchanged {
                revision: command.expected_revision,
            });
        }
        self.set(command.slot, command.replacement.clone());
        Ok(ReplaceOutcome::Replaced {
            previous,
            current: command.replacement,
            revision: self.revision(command.slot),
        })
    }

    pub fn swap_hands(
        &mut self,
        expected: HandSwapPrecondition,
    ) -> Result<HandSwapOutcome, EquipmentMutationError> {
        self.check_revision(EquipmentSlot::MainHand, expected.main_hand)?;
        self.check_revision(EquipmentSlot::OffHand, expected.off_hand)?;
        let main = self.main_hand().clone();
        let off = self.off_hand().clone();
        if main.matches(&off) {
            return Ok(HandSwapOutcome::Unchanged);
        }
        self.items[EquipmentSlot::MainHand.ordinal()] = off;
        self.items[EquipmentSlot::OffHand.ordinal()] = main;
        self.revisions[EquipmentSlot::MainHand.ordinal()] = expected.main_hand.next();
        self.revisions[EquipmentSlot::OffHand.ordinal()] = expected.off_hand.next();
        Ok(HandSwapOutcome::Swapped {
            main_hand_revision: expected.main_hand.next(),
            off_hand_revision: expected.off_hand.next(),
        })
    }

    pub fn apply_inventory_tick_result(
        &mut self,
        slot: EquipmentSlot,
        mutation: StackMutation,
    ) -> Result<MutationApplyOutcome, EquipmentMutationError> {
        self.check_revision(slot, mutation.expected_revision())?;
        let mut next = self.get(slot).clone();
        if next.is_empty() {
            return Err(EquipmentMutationError::EmptyStack { slot });
        }
        for operation in (0..mutation.len()).filter_map(|index| mutation.operation(index)) {
            match operation {
                StackMutationOp::SetCount(count) => next.set_count(*count),
                StackMutationOp::Shrink(amount) => {
                    next.shrink(*amount);
                }
                StackMutationOp::SetDamage(damage) => next
                    .components_mut()
                    .set_damage(*damage)
                    .map_err(EquipmentMutationError::Component)?,
                StackMutationOp::SetMaxDamage(max) => next
                    .components_mut()
                    .set_max_damage(*max)
                    .map_err(EquipmentMutationError::Component)?,
                StackMutationOp::SetUnbreakable(value) => next
                    .components_mut()
                    .set_unbreakable(*value)
                    .map_err(EquipmentMutationError::Component)?,
                StackMutationOp::SetComponent { key, value } => {
                    next.components_mut()
                        .set(*key, value.clone())
                        .map_err(EquipmentMutationError::Component)?;
                }
                StackMutationOp::RemoveComponent(key) => {
                    next.components_mut().remove(*key);
                }
            }
        }
        let previous_revision = self.revision(slot);
        self.set(slot, next);
        Ok(MutationApplyOutcome {
            previous_revision,
            current_revision: self.revision(slot),
        })
    }

    pub const fn drop_chance(&self, slot: EquipmentSlot) -> f32 {
        self.drop_chances[slot.ordinal()]
    }

    pub fn set_drop_chance(
        &mut self,
        slot: EquipmentSlot,
        chance: f32,
    ) -> Result<DropChanceUpdate, DropChanceError> {
        if chance < 0.0 {
            return Err(DropChanceError::Negative { slot });
        }
        let previous = self.drop_chance(slot);
        if previous == chance {
            return Ok(DropChanceUpdate::Unchanged { current: previous });
        }
        self.drop_chances[slot.ordinal()] = chance;
        Ok(DropChanceUpdate::Changed {
            previous,
            current: chance,
        })
    }

    pub fn set_guaranteed_drop(&mut self, slot: EquipmentSlot) -> DropChanceUpdate {
        self.set_drop_chance(slot, GUARANTEED_EQUIPMENT_DROP_CHANCE)
            .expect("guaranteed chance is non-negative")
    }

    pub const fn is_preserved(&self, slot: EquipmentSlot) -> bool {
        self.drop_chance(slot) > PRESERVED_EQUIPMENT_DROP_CHANCE_THRESHOLD
    }

    pub const fn persistence_required(&self) -> bool {
        self.persistence_required
    }

    pub fn set_persistence_required(&mut self, required: bool) {
        self.persistence_required = required;
    }

    pub fn persistence_projection(&self) -> EquipmentPersistenceProjection {
        let mut equipment = PersistenceEntries::new();
        let mut drop_chances = PersistenceEntries::new();
        for slot in EquipmentSlot::VALUES {
            let stack = self.get(slot);
            if !stack.is_empty() {
                equipment.push(PersistenceEntry {
                    slot,
                    value: stack.clone(),
                });
            }
            let chance = self.drop_chance(slot);
            if chance != DEFAULT_EQUIPMENT_DROP_CHANCE {
                drop_chances.push(PersistenceEntry {
                    slot,
                    value: chance,
                });
            }
        }
        EquipmentPersistenceProjection {
            equipment,
            drop_chances,
            persistence_required: self.persistence_required,
        }
    }

    pub(super) fn matches_precondition(&self, expected: &EquipmentPrecondition) -> bool {
        self.revision(expected.slot) == expected.revision
            && self.get(expected.slot).matches(&expected.stack)
    }

    pub(super) fn check_revision(
        &self,
        slot: EquipmentSlot,
        expected: SlotRevision,
    ) -> Result<(), EquipmentMutationError> {
        let actual = self.revision(slot);
        if actual == expected {
            Ok(())
        } else {
            Err(EquipmentMutationError::StaleRevision {
                slot,
                expected,
                actual,
            })
        }
    }
}

impl Default for EquipmentState {
    fn default() -> Self {
        Self::new()
    }
}
