use mc_world::ChunkSnapshot;

use crate::script::storage::PreparedStorageBatch;

use super::{WorldChunkDecision, WorldChunkJournal, WorldChunkJournalError, append_decisions};

pub(super) const INVENTORY_FRAME_MAGIC: &[u8; 4] = b"WIF1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InventoryDecision {
    // Runtime acknowledgement reconstructed by durable participant replay before
    // startup admits saves. Delivery of a plugin result is not projection durability.
    pub(super) payload: Vec<u8>,
    pub(super) projected: bool,
}

impl WorldChunkDecision {
    pub(crate) fn inventory_batch(
        &self,
    ) -> Result<Option<PreparedStorageBatch>, WorldChunkJournalError> {
        self.inventory
            .as_ref()
            .map(|inventory| {
                PreparedStorageBatch::decode_world_inventory(&inventory.payload)
                    .map_err(|error| WorldChunkJournalError::InventoryDecision(error.to_string()))
            })
            .transpose()
    }

    pub(super) fn checkpoint_ready(&self) -> bool {
        self.inventory
            .as_ref()
            .is_none_or(|inventory| inventory.projected)
    }
}

impl WorldChunkJournal {
    pub(crate) fn has_inventory_decisions(&self) -> bool {
        self.shared
            .lock_state()
            .pending
            .iter()
            .any(|decision| decision.inventory.is_some())
    }

    /// The reserved append turn must be obtained before taking participant locks.
    /// A successful return acknowledges the WAL sync, not participant publication.
    pub(crate) fn record_reserved_inventory_decision(
        &self,
        current_tick: u64,
        id: u64,
        snapshots: Vec<ChunkSnapshot>,
        batch: &PreparedStorageBatch,
    ) -> Result<(), WorldChunkJournalError> {
        let payload = batch
            .encode_world_inventory()
            .map_err(WorldChunkJournalError::InventoryEncoding)?;
        for snapshot in &snapshots {
            let actual = snapshot.world_journal_lsn();
            if actual != id {
                return Err(WorldChunkJournalError::SnapshotLsnMismatch {
                    decision_id: id,
                    position: snapshot.pos,
                    actual,
                });
            }
        }
        let decision = WorldChunkDecision {
            id,
            current_tick,
            images: self.encode_images(current_tick, snapshots)?,
            inventory: Some(InventoryDecision {
                payload,
                projected: false,
            }),
        };
        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        if id != state.next_append_id || id > state.next_id {
            return Err(WorldChunkJournalError::InvalidReservation);
        }
        if let Err(error) = append_decisions(&mut state, vec![decision]) {
            drop(state);
            self.shared.append_advanced.notify_waiters();
            return Err(error);
        }
        state.next_append_id = id
            .checked_add(1)
            .ok_or(WorldChunkJournalError::RecordIdExhausted)?;
        drop(state);
        self.shared.append_advanced.notify_waiters();
        self.writer.flush().map_err(
            |source| WorldChunkJournalError::InventorySyncOutcomeUnknown {
                source: Box::new(source),
            },
        )
    }

    /// Call only after durable player/storage projection and participant publication.
    /// Save cutoffs taken before this acknowledgement must retain the decision.
    pub(crate) fn mark_inventory_projected(&self, id: u64) -> Result<(), WorldChunkJournalError> {
        let mut state = self.shared.lock_state();
        let decision = state
            .pending
            .iter_mut()
            .find(|decision| decision.id == id)
            .and_then(|decision| decision.inventory.as_mut())
            .ok_or(WorldChunkJournalError::InvalidReservation)?;
        decision.projected = true;
        Ok(())
    }

    /// Startup only, after chunk replay and before runtime/save admission.
    pub(crate) fn recover_inventory_decisions(
        &self,
        mut recover: impl FnMut(u64, PreparedStorageBatch) -> Result<(), WorldChunkJournalError>,
    ) -> Result<(), WorldChunkJournalError> {
        let mut state = self.shared.lock_state();
        for decision in &mut state.pending {
            let Some(batch) = decision.inventory_batch()? else {
                continue;
            };
            recover(decision.id, batch)?;
            decision
                .inventory
                .as_mut()
                .expect("decoded inventory decision")
                .projected = true;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "world_inventory_journal_tests.rs"]
mod tests;
