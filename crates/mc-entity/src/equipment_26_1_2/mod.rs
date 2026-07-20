//! Pure, bounded Minecraft Java 26.1.2 living/mob equipment state.
//!
//! Registry interpretation, reliable outbound delivery, cross-authority item
//! spawning, and random-number generation remain caller-owned.

#![forbid(unsafe_code)]

mod drops;
mod durability;
mod policy;
mod publication;
mod slot;
mod stack;
mod state;

pub use drops::{
    DeathDropCommitError, DeathDropDurabilityRandom, DeathDropFacts, DeathDropOutcome,
    DeathDropPlan, DeathDropRandomError, DeathDropSkipReason, DropRoll, DropRollError,
    MobPickupFacts, NextIntError, NextIntSample, PickupCommitError, PickupCommitOutcome,
    PickupPrepareError, PickupRandomInput, PickupSource, PickupSourceId, PickupSourceRevision,
    PickupTransaction,
};
pub use durability::{
    DurabilityOutcome, DurabilityUnchangedReason, ProcessedDurabilityChange,
    durability_damage_from_hurt,
};
pub use policy::{
    ArmorReplacementFacts, CurrentItemFacts, EqualItemFacts, EquipGameEvent,
    EquipTransitionContext, EquipTransitionFacts, EquipmentSlotMask, ItemSlotFacts,
    PreferredWeaponFacts, ReplacementComparison, ReplacementDecision, ReplacementPolicyError,
    ReplacementReason, StackEquipKind, WeaponReplacementFacts, decide_replacement,
    equipment_slot_for_item, is_equippable_in_slot, plan_equip_transition,
    resolve_equipment_table_slot, slot_access_accepts,
};
pub use publication::{
    EquipmentPublicationAction, EquipmentPublicationBatch, HAND_SWAP_EVENT_ID,
    MAX_PUBLICATION_ACTIONS, PublicationAdmissionError, PublicationPrepareError,
    PublicationPrepareOutcome, PublicationToken,
};
pub use slot::{
    EQUIPMENT_SLOT_COUNT, EquipmentSlot, EquipmentSlotGroup, EquipmentSlotGroupNameError,
    EquipmentSlotNameError, EquipmentSlotType,
};
pub use stack::{
    ComponentBytes, ComponentEntry, ComponentError, ComponentKey, ItemKey, ItemStackState,
    MAX_STACK_COMPONENT_SERIALIZED_BYTES, MAX_STACK_MUTATION_OPS, SlotRevision, StackComponents,
    StackLimit, StackMutation, StackMutationError, StackMutationOp, StackStateError,
};
pub use state::{
    DEFAULT_EQUIPMENT_DROP_CHANCE, DropChanceError, DropChanceUpdate, EquipmentMutationError,
    EquipmentPersistenceProjection, EquipmentPrecondition, EquipmentState,
    GUARANTEED_EQUIPMENT_DROP_CHANCE, HandSwapOutcome, HandSwapPrecondition, MutationApplyOutcome,
    PRESERVED_EQUIPMENT_DROP_CHANCE_THRESHOLD, PersistenceEntries, PersistenceEntry,
    ReplaceCommand, ReplaceOutcome,
};

#[cfg(test)]
mod tests;
