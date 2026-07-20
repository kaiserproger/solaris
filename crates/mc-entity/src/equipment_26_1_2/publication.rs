use super::{EquipmentSlot, EquipmentState, ItemStackState, PersistenceEntries, PersistenceEntry};

pub const HAND_SWAP_EVENT_ID: u8 = 55;
pub const MAX_PUBLICATION_ACTIONS: usize = super::EQUIPMENT_SLOT_COUNT * 2 + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicationToken(u64);

impl PublicationToken {
    pub const fn new(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EquipmentPublicationAction {
    RemoveLocationEffects {
        slot: EquipmentSlot,
        previous: ItemStackState,
    },
    ApplyLocationEffects {
        slot: EquipmentSlot,
        current: ItemStackState,
    },
    HandSwapEvent {
        event_id: u8,
    },
    EquipmentPacket(PersistenceEntries<ItemStackState>),
}

#[derive(Debug, PartialEq, Eq)]
pub struct EquipmentPublicationBatch {
    token: PublicationToken,
    actions: Vec<EquipmentPublicationAction>,
}

impl EquipmentPublicationBatch {
    pub const fn token(&self) -> PublicationToken {
        self.token
    }

    pub fn actions(&self) -> &[EquipmentPublicationAction] {
        &self.actions
    }

    pub fn len(&self) -> usize {
        self.actions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub const fn capacity(&self) -> usize {
        MAX_PUBLICATION_ACTIONS
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PublicationPrepareOutcome {
    NoChanges,
    Prepared(EquipmentPublicationBatch),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationPrepareError {
    AwaitingAdmission { token: PublicationToken },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationAdmissionError {
    NoPendingAdmission,
    StaleToken {
        expected: PublicationToken,
        actual: PublicationToken,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PublicationAdmissionCandidate {
    token: PublicationToken,
    baseline: PersistenceEntries<ItemStackState>,
}

#[derive(Debug)]
struct EquipmentChange {
    slot: EquipmentSlot,
    previous: ItemStackState,
    current: ItemStackState,
    remove_previous: bool,
    apply_current: bool,
}

impl EquipmentState {
    /// Establishes the baseline after a caller has admitted a complete
    /// spawn/load snapshot into its reliable outbound queue.
    pub fn initialize_publication_baseline(&mut self) -> Result<(), PublicationPrepareError> {
        if let Some(candidate) = &self.pending_publication {
            return Err(PublicationPrepareError::AwaitingAdmission {
                token: candidate.token,
            });
        }
        self.published_items = core::array::from_fn(|index| self.items[index].clone());
        Ok(())
    }

    /// Builds one bounded ordered work item for caller-owned reliable queue
    /// admission. This kernel does not retain or replay the returned actions.
    pub fn prepare_equipment_publication(
        &mut self,
    ) -> Result<PublicationPrepareOutcome, PublicationPrepareError> {
        if let Some(candidate) = &self.pending_publication {
            return Err(PublicationPrepareError::AwaitingAdmission {
                token: candidate.token,
            });
        }

        let mut changes = Vec::with_capacity(super::EQUIPMENT_SLOT_COUNT);
        let mut changed_slots = [false; super::EQUIPMENT_SLOT_COUNT];
        for slot in EquipmentSlot::VALUES {
            let previous = &self.published_items[slot.ordinal()];
            let current = &self.items[slot.ordinal()];
            if !previous.matches(current) {
                changed_slots[slot.ordinal()] = true;
                changes.push(EquipmentChange {
                    slot,
                    previous: previous.clone(),
                    current: current.clone(),
                    remove_previous: !previous.is_empty(),
                    apply_current: !current.is_empty() && !current.is_broken(),
                });
            }
        }
        if changes.is_empty() {
            return Ok(PublicationPrepareOutcome::NoChanges);
        }

        let main = EquipmentSlot::MainHand;
        let off = EquipmentSlot::OffHand;
        let hand_swap = changed_slots[main.ordinal()]
            && changed_slots[off.ordinal()]
            && self.items[main.ordinal()].matches(&self.published_items[off.ordinal()])
            && self.items[off.ordinal()].matches(&self.published_items[main.ordinal()]);

        let mut actions = Vec::with_capacity(MAX_PUBLICATION_ACTIONS);
        for change in &changes {
            if change.remove_previous {
                actions.push(EquipmentPublicationAction::RemoveLocationEffects {
                    slot: change.slot,
                    previous: change.previous.clone(),
                });
            }
        }
        for change in &changes {
            if change.apply_current {
                actions.push(EquipmentPublicationAction::ApplyLocationEffects {
                    slot: change.slot,
                    current: change.current.clone(),
                });
            }
        }
        if hand_swap {
            actions.push(EquipmentPublicationAction::HandSwapEvent {
                event_id: HAND_SWAP_EVENT_ID,
            });
        }

        let mut equipment_packet = PersistenceEntries::new();
        for change in &changes {
            if !(hand_swap
                && matches!(
                    change.slot,
                    EquipmentSlot::MainHand | EquipmentSlot::OffHand
                ))
            {
                equipment_packet.push(PersistenceEntry {
                    slot: change.slot,
                    value: change.current.clone(),
                });
            }
        }
        if !equipment_packet.is_empty() {
            actions.push(EquipmentPublicationAction::EquipmentPacket(
                equipment_packet,
            ));
        }
        debug_assert!(actions.len() <= MAX_PUBLICATION_ACTIONS);

        let token = PublicationToken(self.next_publication_token);
        self.next_publication_token = self.next_publication_token.wrapping_add(1).max(1);
        let mut baseline = PersistenceEntries::new();
        for change in changes {
            baseline.push(PersistenceEntry {
                slot: change.slot,
                value: change.current,
            });
        }
        self.pending_publication = Some(PublicationAdmissionCandidate { token, baseline });

        Ok(PublicationPrepareOutcome::Prepared(
            EquipmentPublicationBatch { token, actions },
        ))
    }

    /// Advances the baseline only after the caller reports successful reliable
    /// queue admission. Packet delivery and reconnect recovery remain caller-owned.
    pub fn confirm_publication_admitted(
        &mut self,
        token: PublicationToken,
    ) -> Result<(), PublicationAdmissionError> {
        self.check_publication_token(token)?;
        let candidate = self
            .pending_publication
            .take()
            .expect("checked pending admission");
        for entry in candidate.baseline.into_iter() {
            self.published_items[entry.slot.ordinal()] = entry.value;
        }
        Ok(())
    }

    /// Clears a batch returned by a failed queue-admission attempt. Consuming
    /// the batch prevents it from being enqueued after the candidate is reset.
    pub fn abort_publication_admission(
        &mut self,
        batch: EquipmentPublicationBatch,
    ) -> Result<(), PublicationAdmissionError> {
        let token = batch.token;
        self.check_publication_token(token)?;
        self.pending_publication = None;
        Ok(())
    }

    fn check_publication_token(
        &self,
        token: PublicationToken,
    ) -> Result<(), PublicationAdmissionError> {
        let Some(candidate) = &self.pending_publication else {
            return Err(PublicationAdmissionError::NoPendingAdmission);
        };
        if candidate.token == token {
            Ok(())
        } else {
            Err(PublicationAdmissionError::StaleToken {
                expected: candidate.token,
                actual: token,
            })
        }
    }
}
