use super::{
    EquipmentPrecondition, EquipmentSlot, EquipmentState, ItemStackState, ReplacementDecision,
    SlotRevision,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropRoll(f32);

impl DropRoll {
    pub fn new(value: f32) -> Result<Self, DropRollError> {
        if value.is_finite() && (0.0..1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(DropRollError::OutsideUnitRange)
        }
    }

    pub const fn get(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropRollError {
    OutsideUnitRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PickupSourceId(u64);

impl PickupSourceId {
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PickupSourceRevision(u64);

impl PickupSourceRevision {
    pub const INITIAL: Self = Self(0);

    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickupSource {
    id: PickupSourceId,
    revision: PickupSourceRevision,
    stack: ItemStackState,
}

impl PickupSource {
    pub const fn new(id: PickupSourceId, stack: ItemStackState) -> Self {
        Self {
            id,
            revision: PickupSourceRevision::INITIAL,
            stack,
        }
    }

    pub const fn id(&self) -> PickupSourceId {
        self.id
    }

    pub const fn revision(&self) -> PickupSourceRevision {
        self.revision
    }

    pub const fn stack(&self) -> &ItemStackState {
        &self.stack
    }

    pub fn replace(&mut self, stack: ItemStackState) {
        if !self.stack.matches(&stack) {
            self.stack = stack;
            self.revision = self.revision.next();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobPickupFacts {
    pub resolved: EquipmentPrecondition,
    pub fallback_main_hand: Option<EquipmentPrecondition>,
    pub is_equippable_in_resolved_slot: bool,
    pub resolved_replacement: ReplacementDecision,
    pub can_hold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickupRandomInput {
    None,
    ReplacementDrop(DropRoll),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickupPrepareError {
    EmptyStack,
    NotEquippable,
    StaleReplacementFacts {
        slot: EquipmentSlot,
        expected_revision: SlotRevision,
        actual_revision: SlotRevision,
    },
    MissingMainHandFallback,
    WrongMainHandFallbackSlot,
    CannotReplace,
    CannotHold,
    MissingReplacementDropRoll,
    UnexpectedReplacementDropRoll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickupTransaction {
    source_id: PickupSourceId,
    source_revision: PickupSourceRevision,
    source_before: ItemStackState,
    source_after: ItemStackState,
    equipment_before: EquipmentPrecondition,
    equipped: ItemStackState,
    replaced_drop: Option<ItemStackState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickupCommitError {
    WrongSource {
        expected: PickupSourceId,
        actual: PickupSourceId,
    },
    StaleSource {
        expected_revision: PickupSourceRevision,
        actual_revision: PickupSourceRevision,
    },
    StaleEquipment {
        slot: EquipmentSlot,
        expected_revision: SlotRevision,
        actual_revision: SlotRevision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickupCommitOutcome {
    pub slot: EquipmentSlot,
    pub source_remaining: ItemStackState,
    pub equipped: ItemStackState,
    pub replaced_drop: Option<ItemStackState>,
}

impl EquipmentState {
    pub fn prepare_mob_pickup(
        &self,
        source: &PickupSource,
        facts: MobPickupFacts,
        random: PickupRandomInput,
    ) -> Result<PickupTransaction, PickupPrepareError> {
        if source.stack.is_empty() {
            return Err(PickupPrepareError::EmptyStack);
        }
        if !facts.is_equippable_in_resolved_slot {
            return Err(PickupPrepareError::NotEquippable);
        }
        self.check_replacement_precondition(&facts.resolved)?;

        let equipment_before =
            if facts.resolved.slot.is_armor() && !facts.resolved_replacement.should_replace() {
                let fallback = facts
                    .fallback_main_hand
                    .ok_or(PickupPrepareError::MissingMainHandFallback)?;
                if fallback.slot != EquipmentSlot::MainHand {
                    return Err(PickupPrepareError::WrongMainHandFallbackSlot);
                }
                self.check_replacement_precondition(&fallback)?;
                if !fallback.stack.is_empty() {
                    return Err(PickupPrepareError::CannotReplace);
                }
                fallback
            } else {
                if !facts.resolved_replacement.should_replace() {
                    return Err(PickupPrepareError::CannotReplace);
                }
                facts.resolved
            };
        if !facts.can_hold {
            return Err(PickupPrepareError::CannotHold);
        }

        let replaced_drop = match (equipment_before.stack.is_empty(), random) {
            (true, PickupRandomInput::None) => None,
            (true, PickupRandomInput::ReplacementDrop(_)) => {
                return Err(PickupPrepareError::UnexpectedReplacementDropRoll);
            }
            (false, PickupRandomInput::None) => {
                return Err(PickupPrepareError::MissingReplacementDropRoll);
            }
            (false, PickupRandomInput::ReplacementDrop(roll)) => ((roll.get() - 0.1).max(0.0)
                < self.drop_chance(equipment_before.slot))
            .then(|| equipment_before.stack.clone()),
        };
        let limited = source.stack.clone().limit_for(equipment_before.slot);
        Ok(PickupTransaction {
            source_id: source.id,
            source_revision: source.revision,
            source_before: source.stack.clone(),
            source_after: limited.remainder,
            equipment_before,
            equipped: limited.equipped,
            replaced_drop,
        })
    }

    pub fn commit_mob_pickup(
        &mut self,
        source: &mut PickupSource,
        transaction: PickupTransaction,
    ) -> Result<PickupCommitOutcome, PickupCommitError> {
        if source.id != transaction.source_id {
            return Err(PickupCommitError::WrongSource {
                expected: transaction.source_id,
                actual: source.id,
            });
        }
        if source.revision != transaction.source_revision
            || !source.stack.matches(&transaction.source_before)
        {
            return Err(PickupCommitError::StaleSource {
                expected_revision: transaction.source_revision,
                actual_revision: source.revision,
            });
        }
        if !self.matches_precondition(&transaction.equipment_before) {
            return Err(PickupCommitError::StaleEquipment {
                slot: transaction.equipment_before.slot,
                expected_revision: transaction.equipment_before.revision,
                actual_revision: self.revision(transaction.equipment_before.slot),
            });
        }

        let slot = transaction.equipment_before.slot;
        source.stack = transaction.source_after.clone();
        source.revision = source.revision.next();
        self.set(slot, transaction.equipped.clone());
        self.set_guaranteed_drop(slot);
        self.set_persistence_required(true);

        Ok(PickupCommitOutcome {
            slot,
            source_remaining: transaction.source_after,
            equipped: transaction.equipped,
            replaced_drop: transaction.replaced_drop,
        })
    }

    fn check_replacement_precondition(
        &self,
        expected: &EquipmentPrecondition,
    ) -> Result<(), PickupPrepareError> {
        if self.matches_precondition(expected) {
            Ok(())
        } else {
            Err(PickupPrepareError::StaleReplacementFacts {
                slot: expected.slot,
                expected_revision: expected.revision,
                actual_revision: self.revision(expected.slot),
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeathDropFacts {
    pub original_chance: f32,
    pub processed_chance: f32,
    pub killed_by_player: bool,
    pub prevents_equipment_drop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathDropSkipReason {
    ZeroChance,
    EmptyStack,
    PreventedByEnchantment,
    NotPlayerKilledOrPreserved,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeathDropPlan {
    Skipped(DeathDropSkipReason),
    Ready {
        expected: EquipmentPrecondition,
        chance: f32,
        preserved: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextIntSample {
    bound: i32,
    value: i32,
}

impl NextIntSample {
    pub const fn new(bound: i32, value: i32) -> Result<Self, NextIntError> {
        if bound <= 0 {
            return Err(NextIntError::NonPositiveBound);
        }
        if value < 0 || value >= bound {
            return Err(NextIntError::ValueOutsideBound);
        }
        Ok(Self { bound, value })
    }

    pub const fn bound(self) -> i32 {
        self.bound
    }

    pub const fn value(self) -> i32 {
        self.value
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextIntError {
    NonPositiveBound,
    ValueOutsideBound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeathDropDurabilityRandom {
    pub inner: NextIntSample,
    pub outer: NextIntSample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathDropRandomError {
    MissingDropRoll,
    UnexpectedDropRoll,
    MissingDurabilityRandom,
    UnexpectedDurabilityRandom,
    InnerBound { expected: i32, actual: i32 },
    OuterBound { expected: i32, actual: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeathDropOutcome {
    Skipped(DeathDropSkipReason),
    NotDropped,
    Dropped {
        slot: EquipmentSlot,
        spawn: ItemStackState,
        previous_revision: SlotRevision,
        current_revision: SlotRevision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeathDropCommitError {
    StaleEquipment {
        slot: EquipmentSlot,
        expected_revision: SlotRevision,
        actual_revision: SlotRevision,
    },
    Random(DeathDropRandomError),
    Component(super::ComponentError),
}

impl EquipmentState {
    pub fn plan_death_drop(&self, slot: EquipmentSlot, facts: DeathDropFacts) -> DeathDropPlan {
        if facts.original_chance == 0.0 {
            return DeathDropPlan::Skipped(DeathDropSkipReason::ZeroChance);
        }
        let preserved = facts.original_chance > 1.0;
        if self.get(slot).is_empty() {
            return DeathDropPlan::Skipped(DeathDropSkipReason::EmptyStack);
        }
        if facts.prevents_equipment_drop {
            return DeathDropPlan::Skipped(DeathDropSkipReason::PreventedByEnchantment);
        }
        if !facts.killed_by_player && !preserved {
            return DeathDropPlan::Skipped(DeathDropSkipReason::NotPlayerKilledOrPreserved);
        }
        DeathDropPlan::Ready {
            expected: self.precondition(slot),
            chance: facts.processed_chance,
            preserved,
        }
    }

    pub fn commit_death_drop(
        &mut self,
        plan: DeathDropPlan,
        roll: Option<DropRoll>,
        durability_random: Option<DeathDropDurabilityRandom>,
    ) -> Result<DeathDropOutcome, DeathDropCommitError> {
        let (expected, preserved) = match (plan, roll) {
            (DeathDropPlan::Skipped(reason), None) => {
                if durability_random.is_some() {
                    return Err(DeathDropCommitError::Random(
                        DeathDropRandomError::UnexpectedDurabilityRandom,
                    ));
                }
                return Ok(DeathDropOutcome::Skipped(reason));
            }
            (DeathDropPlan::Skipped(_), Some(_)) => {
                return Err(DeathDropCommitError::Random(
                    DeathDropRandomError::UnexpectedDropRoll,
                ));
            }
            (DeathDropPlan::Ready { .. }, None) => {
                return Err(DeathDropCommitError::Random(
                    DeathDropRandomError::MissingDropRoll,
                ));
            }
            (
                DeathDropPlan::Ready {
                    expected,
                    chance,
                    preserved,
                },
                Some(roll),
            ) => {
                if !self.matches_precondition(&expected) {
                    return Err(DeathDropCommitError::StaleEquipment {
                        slot: expected.slot,
                        expected_revision: expected.revision,
                        actual_revision: self.revision(expected.slot),
                    });
                }
                if !matches!(
                    roll.get().partial_cmp(&chance),
                    Some(core::cmp::Ordering::Less)
                ) {
                    if durability_random.is_some() {
                        return Err(DeathDropCommitError::Random(
                            DeathDropRandomError::UnexpectedDurabilityRandom,
                        ));
                    }
                    return Ok(DeathDropOutcome::NotDropped);
                }
                (expected, preserved)
            }
        };

        let mut spawn = expected.stack.clone();
        if !preserved && spawn.is_damageable() {
            let samples = durability_random.ok_or(DeathDropCommitError::Random(
                DeathDropRandomError::MissingDurabilityRandom,
            ))?;
            let max_damage = spawn.max_damage().expect("damageable drop has max damage");
            let inner_bound = (max_damage - 3).max(1);
            if samples.inner.bound() != inner_bound {
                return Err(DeathDropCommitError::Random(
                    DeathDropRandomError::InnerBound {
                        expected: inner_bound,
                        actual: samples.inner.bound(),
                    },
                ));
            }
            let outer_bound = 1 + samples.inner.value();
            if samples.outer.bound() != outer_bound {
                return Err(DeathDropCommitError::Random(
                    DeathDropRandomError::OuterBound {
                        expected: outer_bound,
                        actual: samples.outer.bound(),
                    },
                ));
            }
            spawn
                .set_damage_clamped(max_damage - samples.outer.value())
                .map_err(DeathDropCommitError::Component)?;
        } else if durability_random.is_some() {
            return Err(DeathDropCommitError::Random(
                DeathDropRandomError::UnexpectedDurabilityRandom,
            ));
        }

        let slot = expected.slot;
        let previous_revision = expected.revision;
        self.set(slot, ItemStackState::EMPTY);
        Ok(DeathDropOutcome::Dropped {
            slot,
            spawn,
            previous_revision,
            current_revision: self.revision(slot),
        })
    }
}
