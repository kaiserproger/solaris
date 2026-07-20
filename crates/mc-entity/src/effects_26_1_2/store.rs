use super::gameplay::tick_application;
use super::instance::{decrement_duration, is_shorter_duration_than};
use super::{EffectApplication, EffectId, EffectInstance, EffectKind, TargetEffectContext};
use serde::{Deserialize, Serialize};

pub const MAX_ACTIVE_EFFECTS: usize = 256;
pub const MAX_HIDDEN_EFFECTS: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectLimits {
    active: usize,
    hidden: usize,
}

impl EffectLimits {
    pub fn new(active: usize, hidden: usize) -> Result<Self, EffectLimitError> {
        if active > MAX_ACTIVE_EFFECTS {
            return Err(EffectLimitError::TooManyActiveEffects);
        }
        if hidden > MAX_HIDDEN_EFFECTS {
            return Err(EffectLimitError::TooManyHiddenEffects);
        }
        Ok(Self { active, hidden })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectLimitError {
    TooManyActiveEffects,
    TooManyHiddenEffects,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectStoreError {
    AllocationFailed,
    ActiveCapacityExceeded {
        capacity: usize,
    },
    HiddenCapacityExceeded {
        capacity: usize,
    },
    KindMismatch {
        id: EffectId,
        active: EffectKind,
        incoming: EffectKind,
    },
    InvalidSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveEffectChainSnapshot {
    pub current: EffectInstance,
    pub hidden: Vec<EffectInstance>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ActiveEffectsSnapshot {
    pub chains: Vec<ActiveEffectChainSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectCapacities {
    pub active: usize,
    pub hidden_nodes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Typed `LivingEntity.addEffect` decisions.
///
/// `Added` means `onEffectAdded(current)` followed by
/// `onEffectStarted(started)`. `Updated` means
/// `onEffectUpdated(current, true)` followed by `onEffectStarted(started)`.
/// `HiddenOnly` and `Unchanged` request only `onEffectStarted(started)` and do
/// not publish an effect packet.
pub enum AddOutcome {
    Added {
        current: EffectInstance,
        started: EffectInstance,
    },
    Updated {
        current: EffectInstance,
        started: EffectInstance,
        refresh_attributes: bool,
    },
    HiddenOnly {
        current: EffectInstance,
        started: EffectInstance,
    },
    Unchanged {
        current: EffectInstance,
        started: EffectInstance,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// `Removed` requests `LivingEntity.onEffectsRemoved`; `NotPresent` is a no-op.
pub enum RemoveOutcome {
    Removed { effect: EffectInstance },
    NotPresent,
}

#[derive(Debug, Clone, Copy)]
struct ActiveRow {
    current: EffectInstance,
    hidden_head: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct HiddenNode {
    effect: Option<EffectInstance>,
    next: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct ActiveEffects {
    limits: EffectLimits,
    active: Vec<ActiveRow>,
    hidden: Vec<HiddenNode>,
    free_hidden: Option<usize>,
    hidden_in_use: usize,
    epoch: u64,
}

impl ActiveEffects {
    pub fn try_new(limits: EffectLimits) -> Result<Self, EffectStoreError> {
        let mut active = Vec::new();
        active
            .try_reserve_exact(limits.active)
            .map_err(|_| EffectStoreError::AllocationFailed)?;

        let mut hidden = Vec::new();
        hidden
            .try_reserve_exact(limits.hidden)
            .map_err(|_| EffectStoreError::AllocationFailed)?;
        for index in 0..limits.hidden {
            hidden.push(HiddenNode {
                effect: None,
                next: (index + 1 < limits.hidden).then_some(index + 1),
            });
        }

        Ok(Self {
            limits,
            active,
            hidden,
            free_hidden: (limits.hidden > 0).then_some(0),
            hidden_in_use: 0,
            epoch: 0,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    #[must_use]
    pub fn capacities(&self) -> EffectCapacities {
        EffectCapacities {
            active: self.active.capacity(),
            hidden_nodes: self.hidden.capacity(),
        }
    }

    #[must_use]
    pub const fn hidden_nodes_in_use(&self) -> usize {
        self.hidden_in_use
    }

    #[must_use]
    pub fn snapshot(&self) -> ActiveEffectsSnapshot {
        let chains = self
            .active
            .iter()
            .map(|row| {
                let mut hidden = Vec::new();
                let mut node = row.hidden_head;
                while let Some(index) = node {
                    let entry = self.hidden[index];
                    hidden.push(
                        entry
                            .effect
                            .expect("active hidden chain must contain effects"),
                    );
                    node = entry.next;
                }
                ActiveEffectChainSnapshot {
                    current: row.current,
                    hidden,
                }
            })
            .collect();
        ActiveEffectsSnapshot { chains }
    }

    pub fn try_from_snapshot(
        limits: EffectLimits,
        snapshot: &ActiveEffectsSnapshot,
    ) -> Result<Self, EffectStoreError> {
        if snapshot.chains.len() > limits.active {
            return Err(EffectStoreError::ActiveCapacityExceeded {
                capacity: limits.active,
            });
        }
        let hidden_count = snapshot
            .chains
            .iter()
            .try_fold(0_usize, |count, chain| {
                count.checked_add(chain.hidden.len())
            })
            .ok_or(EffectStoreError::HiddenCapacityExceeded {
                capacity: limits.hidden,
            })?;
        if hidden_count > limits.hidden {
            return Err(EffectStoreError::HiddenCapacityExceeded {
                capacity: limits.hidden,
            });
        }
        for (index, chain) in snapshot.chains.iter().enumerate() {
            if index > 0 && snapshot.chains[index - 1].current.id >= chain.current.id {
                return Err(EffectStoreError::InvalidSnapshot);
            }
            if chain
                .hidden
                .iter()
                .any(|hidden| hidden.id != chain.current.id || hidden.kind != chain.current.kind)
            {
                return Err(EffectStoreError::InvalidSnapshot);
            }
        }

        let mut restored = Self::try_new(limits)?;
        for chain in &snapshot.chains {
            let mut head = None;
            for hidden in chain.hidden.iter().rev().copied() {
                head = Some(restored.allocate_hidden(hidden, head));
            }
            restored.active.push(ActiveRow {
                current: chain.current,
                hidden_head: head,
            });
        }
        Ok(restored)
    }

    #[must_use]
    pub fn get(&self, id: EffectId) -> Option<EffectInstance> {
        self.row_index(id)
            .ok()
            .map(|index| self.active[index].current)
    }

    #[must_use]
    pub fn hidden_depth(&self, id: EffectId) -> usize {
        let Ok(index) = self.row_index(id) else {
            return 0;
        };
        let mut depth = 0;
        let mut node = self.active[index].hidden_head;
        while let Some(index) = node {
            depth += 1;
            node = self.hidden[index].next;
        }
        depth
    }

    #[must_use]
    pub fn hidden_at(&self, id: EffectId, depth: usize) -> Option<EffectInstance> {
        let row = self.row_index(id).ok()?;
        let mut node = self.active[row].hidden_head;
        for _ in 0..depth {
            node = self.hidden[node?].next;
        }
        self.hidden[node?].effect
    }

    pub fn add(&mut self, incoming: EffectInstance) -> Result<AddOutcome, EffectStoreError> {
        let row_index = match self.row_index(incoming.id) {
            Ok(index) => index,
            Err(index) => {
                if self.active.len() == self.limits.active {
                    return Err(EffectStoreError::ActiveCapacityExceeded {
                        capacity: self.limits.active,
                    });
                }
                self.active.insert(
                    index,
                    ActiveRow {
                        current: incoming,
                        hidden_head: None,
                    },
                );
                self.bump_epoch();
                return Ok(AddOutcome::Added {
                    current: incoming,
                    started: incoming,
                });
            }
        };

        let active_kind = self.active[row_index].current.kind;
        if active_kind != incoming.kind {
            return Err(EffectStoreError::KindMismatch {
                id: incoming.id,
                active: active_kind,
                incoming: incoming.kind,
            });
        }

        let location = Location::Active(row_index);
        let needs_hidden = self.merge_needs_hidden_node(location, incoming);
        if needs_hidden && self.free_hidden.is_none() {
            return Err(EffectStoreError::HiddenCapacityExceeded {
                capacity: self.limits.hidden,
            });
        }

        let merge = self.merge_at(location, incoming);
        let current = self.active[row_index].current;
        self.bump_epoch();
        Ok(if merge.published {
            AddOutcome::Updated {
                current,
                started: incoming,
                refresh_attributes: true,
            }
        } else if merge.mutated {
            AddOutcome::HiddenOnly {
                current,
                started: incoming,
            }
        } else {
            AddOutcome::Unchanged {
                current,
                started: incoming,
            }
        })
    }

    pub fn remove(&mut self, id: EffectId) -> RemoveOutcome {
        let Ok(index) = self.row_index(id) else {
            return RemoveOutcome::NotPresent;
        };
        let row = self.active.remove(index);
        self.free_hidden_chain(row.hidden_head);
        self.bump_epoch();
        RemoveOutcome::Removed {
            effect: row.current,
        }
    }

    /// Plans one tick in an order explicitly selected by the caller.
    ///
    /// `action_order` must contain every active ID exactly once. It is not
    /// inferred from internal storage and is not claimed to match vanilla's
    /// identity-hashed holder order. Any planning error invalidates an older
    /// batch in `scratch` but leaves this component unchanged.
    pub fn plan_tick_batch<'a>(
        &self,
        entity_tick_count: i32,
        target: TargetEffectContext,
        action_order: &[EffectId],
        scratch: &'a mut TickScratch,
    ) -> Result<&'a mut [PendingEffectTick], TickPlanError> {
        scratch.planned_epoch = None;
        if self.active.len() > scratch.capacity {
            return Err(TickPlanError::ScratchTooSmall {
                needed: self.active.len(),
                capacity: scratch.capacity,
            });
        }
        if action_order.len() != self.active.len() {
            return Err(TickPlanError::OrderLengthMismatch {
                active: self.active.len(),
                provided: action_order.len(),
            });
        }
        for (index, &id) in action_order.iter().enumerate() {
            if action_order[..index].contains(&id) {
                return Err(TickPlanError::DuplicateEffectId(id));
            }
            if self.row_index(id).is_err() {
                return Err(TickPlanError::UnknownEffectId(id));
            }
        }

        scratch.pending.clear();
        scratch.outcomes.clear();
        for &id in action_order {
            let row = &self.active[self
                .row_index(id)
                .expect("action-order preflight checked every effect id")];
            let application = if row.current.has_remaining_duration() {
                tick_application(row.current, entity_tick_count, target)
            } else {
                EffectApplication::None
            };
            scratch.pending.push(PendingEffectTick {
                id: row.current.id,
                application,
                caller_owned_result: None,
            });
        }
        scratch.planned_epoch = Some(self.epoch);
        Ok(&mut scratch.pending)
    }

    /// Commits a fully resolved batch after an allocation-free atomic preflight.
    ///
    /// Outcomes preserve the caller-supplied action order. Stale or unresolved
    /// batches return before any duration or hidden-chain mutation.
    pub fn commit_tick_batch<'a>(
        &mut self,
        scratch: &'a mut TickScratch,
    ) -> Result<&'a [EffectTickOutcome], TickCommitError> {
        let Some(planned_epoch) = scratch.planned_epoch else {
            return Err(TickCommitError::NoPlannedBatch);
        };
        if planned_epoch != self.epoch || scratch.pending.len() != self.active.len() {
            return Err(TickCommitError::StalePlan);
        }
        for pending in &scratch.pending {
            if self.row_index(pending.id).is_err() {
                return Err(TickCommitError::StalePlan);
            }
            if matches!(pending.application, EffectApplication::CallerOwned { .. })
                && pending.caller_owned_result.is_none()
            {
                return Err(TickCommitError::UnresolvedCallerOwned(pending.id));
            }
        }

        scratch.outcomes.clear();
        for pending_index in 0..scratch.pending.len() {
            let pending = scratch.pending[pending_index];
            let row_index = self
                .row_index(pending.id)
                .expect("tick preflight checked every active id");
            let current = self.active[row_index].current;
            let callback_removes = pending.caller_owned_result == Some(CallerOwnedResult::Remove);
            if !current.has_remaining_duration() || callback_removes {
                let row = self.active.remove(row_index);
                self.free_hidden_chain(row.hidden_head);
                scratch.outcomes.push(EffectTickOutcome {
                    id: pending.id,
                    restored: None,
                    refresh_attributes: false,
                    periodic_sync: None,
                    removed: Some(row.current),
                });
                continue;
            }

            self.decrement_row(row_index);
            let restored = self.restore_hidden_if_expired(row_index);
            let after = self.active[row_index].current;
            let remains = after.has_remaining_duration();
            let periodic_sync = (remains && after.duration % 600 == 0).then_some(after);
            let removed = if remains {
                None
            } else {
                let row = self.active.remove(row_index);
                self.free_hidden_chain(row.hidden_head);
                Some(row.current)
            };
            scratch.outcomes.push(EffectTickOutcome {
                id: pending.id,
                restored,
                refresh_attributes: restored.is_some(),
                periodic_sync,
                removed,
            });
        }

        if !scratch.pending.is_empty() {
            self.bump_epoch();
        }
        scratch.planned_epoch = None;
        Ok(&scratch.outcomes)
    }

    fn row_index(&self, id: EffectId) -> Result<usize, usize> {
        self.active.binary_search_by_key(&id, |row| row.current.id)
    }

    fn merge_needs_hidden_node(&self, location: Location, incoming: EffectInstance) -> bool {
        let current = self.location_effect(location);
        if incoming.amplifier > current.amplifier {
            return is_shorter_duration_than(incoming, current);
        }
        if is_shorter_duration_than(current, incoming) && incoming.amplifier != current.amplifier {
            return match self.location_next(location) {
                Some(next) => self.merge_needs_hidden_node(Location::Hidden(next), incoming),
                None => true,
            };
        }
        false
    }

    fn merge_at(&mut self, location: Location, incoming: EffectInstance) -> MergeResult {
        let mut current = self.location_effect(location);
        let mut changed = false;
        let mut mutated = false;
        if incoming.amplifier > current.amplifier {
            if is_shorter_duration_than(incoming, current) {
                let previous_hidden = self.location_next(location);
                let hidden = self.allocate_hidden(current, previous_hidden);
                self.set_location_next(location, Some(hidden));
            }
            current.amplifier = incoming.amplifier;
            current.duration = incoming.duration;
            changed = true;
            mutated = true;
        } else if is_shorter_duration_than(current, incoming) {
            if incoming.amplifier == current.amplifier {
                current.duration = incoming.duration;
                changed = true;
                mutated = true;
            } else if let Some(hidden) = self.location_next(location) {
                mutated |= self.merge_at(Location::Hidden(hidden), incoming).mutated;
            } else {
                let hidden = self.allocate_hidden(incoming, None);
                self.set_location_next(location, Some(hidden));
                mutated = true;
            }
        }

        if (!incoming.flags.ambient && current.flags.ambient) || changed {
            mutated |= current.flags.ambient != incoming.flags.ambient;
            current.flags.ambient = incoming.flags.ambient;
            changed = true;
        }
        if incoming.flags.visible != current.flags.visible {
            current.flags.visible = incoming.flags.visible;
            changed = true;
            mutated = true;
        }
        if incoming.flags.show_icon != current.flags.show_icon {
            current.flags.show_icon = incoming.flags.show_icon;
            changed = true;
            mutated = true;
        }
        self.set_location_effect(location, current);
        MergeResult {
            published: changed,
            mutated,
        }
    }

    fn location_effect(&self, location: Location) -> EffectInstance {
        match location {
            Location::Active(index) => self.active[index].current,
            Location::Hidden(index) => self.hidden[index]
                .effect
                .expect("occupied hidden location must contain an effect"),
        }
    }

    fn set_location_effect(&mut self, location: Location, effect: EffectInstance) {
        match location {
            Location::Active(index) => self.active[index].current = effect,
            Location::Hidden(index) => self.hidden[index].effect = Some(effect),
        }
    }

    fn location_next(&self, location: Location) -> Option<usize> {
        match location {
            Location::Active(index) => self.active[index].hidden_head,
            Location::Hidden(index) => self.hidden[index].next,
        }
    }

    fn set_location_next(&mut self, location: Location, next: Option<usize>) {
        match location {
            Location::Active(index) => self.active[index].hidden_head = next,
            Location::Hidden(index) => self.hidden[index].next = next,
        }
    }

    fn allocate_hidden(&mut self, effect: EffectInstance, next: Option<usize>) -> usize {
        let index = self
            .free_hidden
            .expect("merge preflight guarantees a free hidden node");
        self.free_hidden = self.hidden[index].next;
        self.hidden[index] = HiddenNode {
            effect: Some(effect),
            next,
        };
        self.hidden_in_use += 1;
        index
    }

    fn free_hidden_chain(&mut self, mut next: Option<usize>) {
        while let Some(index) = next {
            next = self.hidden[index].next;
            self.hidden[index] = HiddenNode {
                effect: None,
                next: self.free_hidden,
            };
            self.free_hidden = Some(index);
            self.hidden_in_use -= 1;
        }
    }

    fn decrement_row(&mut self, row_index: usize) {
        let mut hidden = self.active[row_index].hidden_head;
        while let Some(index) = hidden {
            let mut effect = self.hidden[index]
                .effect
                .expect("active hidden chain must contain effects");
            effect.duration = decrement_duration(effect.duration);
            self.hidden[index].effect = Some(effect);
            hidden = self.hidden[index].next;
        }
        self.active[row_index].current.duration =
            decrement_duration(self.active[row_index].current.duration);
    }

    fn restore_hidden_if_expired(&mut self, row_index: usize) -> Option<EffectInstance> {
        if self.active[row_index].current.duration != 0 {
            return None;
        }
        let hidden_index = self.active[row_index].hidden_head?;
        let node = self.hidden[hidden_index];
        let restored = node
            .effect
            .expect("active hidden head must contain an effect");
        self.active[row_index].current = restored;
        self.active[row_index].hidden_head = node.next;
        self.hidden[hidden_index] = HiddenNode {
            effect: None,
            next: self.free_hidden,
        };
        self.free_hidden = Some(hidden_index);
        self.hidden_in_use -= 1;
        Some(restored)
    }

    fn bump_epoch(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
    }
}

#[derive(Debug, Clone, Copy)]
enum Location {
    Active(usize),
    Hidden(usize),
}

#[derive(Debug, Clone, Copy)]
struct MergeResult {
    published: bool,
    mutated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerOwnedResult {
    /// `shouldApplyEffectTickThisTick` returned false.
    Skipped,
    /// The callback ran and returned true.
    Continue,
    /// The callback ran and returned false, so the active effect is removed
    /// without decrementing or restoring its hidden chain.
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingEffectTick {
    id: EffectId,
    application: EffectApplication,
    caller_owned_result: Option<CallerOwnedResult>,
}

impl PendingEffectTick {
    #[must_use]
    pub const fn id(&self) -> EffectId {
        self.id
    }

    #[must_use]
    pub const fn application(&self) -> EffectApplication {
        self.application
    }

    pub fn resolve_caller_owned(
        &mut self,
        result: CallerOwnedResult,
    ) -> Result<(), TickResolutionError> {
        if !matches!(self.application, EffectApplication::CallerOwned { .. }) {
            return Err(TickResolutionError::NotCallerOwned(self.id));
        }
        self.caller_owned_result = Some(result);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickResolutionError {
    NotCallerOwned(EffectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectTickOutcome {
    pub id: EffectId,
    /// `onEffectUpdated(effect, true, null)` before any later fields.
    pub restored: Option<EffectInstance>,
    pub refresh_attributes: bool,
    /// The 600-tick `onEffectUpdated(effect, false, null)` decision.
    pub periodic_sync: Option<EffectInstance>,
    /// `onEffectsRemoved`, ordered after restoration and periodic decisions.
    pub removed: Option<EffectInstance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickScratchError {
    CapacityExceedsHardCap,
    AllocationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickScratchCapacities {
    pub pending: usize,
    pub outcomes: usize,
}

#[derive(Debug)]
pub struct TickScratch {
    capacity: usize,
    pending: Vec<PendingEffectTick>,
    outcomes: Vec<EffectTickOutcome>,
    planned_epoch: Option<u64>,
}

impl TickScratch {
    pub fn try_new(capacity: usize) -> Result<Self, TickScratchError> {
        if capacity > MAX_ACTIVE_EFFECTS {
            return Err(TickScratchError::CapacityExceedsHardCap);
        }
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(capacity)
            .map_err(|_| TickScratchError::AllocationFailed)?;
        let mut outcomes = Vec::new();
        outcomes
            .try_reserve_exact(capacity)
            .map_err(|_| TickScratchError::AllocationFailed)?;
        Ok(Self {
            capacity,
            pending,
            outcomes,
            planned_epoch: None,
        })
    }

    #[must_use]
    pub fn capacities(&self) -> TickScratchCapacities {
        TickScratchCapacities {
            pending: self.pending.capacity(),
            outcomes: self.outcomes.capacity(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickPlanError {
    ScratchTooSmall { needed: usize, capacity: usize },
    OrderLengthMismatch { active: usize, provided: usize },
    DuplicateEffectId(EffectId),
    UnknownEffectId(EffectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickCommitError {
    NoPlannedBatch,
    StalePlan,
    UnresolvedCallerOwned(EffectId),
}
