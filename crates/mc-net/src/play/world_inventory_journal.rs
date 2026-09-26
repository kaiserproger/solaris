use mc_world::ChunkSnapshot;

use crate::script::storage::PreparedStorageBatch;

use super::{WorldChunkDecision, WorldChunkJournal, WorldChunkJournalError};

pub(super) const INVENTORY_FRAME_MAGIC: &[u8; 4] = b"WIF1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InventoryDecision {
    // Runtime acknowledgement reconstructed by durable participant replay before
    // startup admits saves. Delivery of a plugin result is not projection durability.
    pub(super) payload: Vec<u8>,
    pub(super) projected: bool,
    /// Durable native health commit linked to this physical debit. The WAM1
    /// frame follows the regional WAL sync; this is not plugin projection.
    pub(super) native_committed: bool,
}

impl WorldChunkDecision {
    pub(crate) fn inventory_batch(
        &self,
    ) -> Result<Option<PreparedStorageBatch>, WorldChunkJournalError> {
        self.inventory
            .as_ref()
            .map(|inventory| {
                PreparedStorageBatch::decode_world_decision(&inventory.payload)
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
        self.record_reserved_decisions(current_tick, vec![(id, snapshots, Some(payload))])
    }

    /// Append and sync the native-health acknowledgement after the regional
    /// journal is durable, before any participant is published.
    pub(crate) fn confirm_treatment_committed(
        &self,
        id: u64,
    ) -> Result<(), WorldChunkJournalError> {
        let mut state = self.shared.lock_state();
        if state.poisoned {
            return Err(WorldChunkJournalError::PoisonedOutcomeUnknown);
        }
        let decision = state
            .pending
            .iter()
            .find(|decision| decision.id == id)
            .ok_or(WorldChunkJournalError::InvalidReservation)?;
        if decision
            .inventory_batch()?
            .is_none_or(|batch| batch.treatment().is_none())
        {
            return Err(WorldChunkJournalError::InvalidReservation);
        }
        if decision
            .inventory
            .as_ref()
            .expect("medical decision carries inventory")
            .native_committed
        {
            return Ok(());
        }
        let bytes = super::encode_treatment_ack(id);
        if state
            .requests
            .send(super::WriterRequest::Append { bytes })
            .is_err()
        {
            state.poisoned = true;
            return Err(WorldChunkJournalError::WriterClosed {
                operation: "append treatment acknowledgement",
            });
        }
        if let Err(error) = self.writer.flush() {
            state.poisoned = true;
            return Err(WorldChunkJournalError::InventorySyncOutcomeUnknown {
                source: Box::new(error),
            });
        }
        state
            .pending
            .iter_mut()
            .find(|decision| decision.id == id)
            .and_then(|decision| decision.inventory.as_mut())
            .expect("medical decision remains retained")
            .native_committed = true;
        Ok(())
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
            let needs_treatment = batch.treatment().is_some();
            recover(decision.id, batch)?;
            if !needs_treatment {
                decision
                    .inventory
                    .as_mut()
                    .expect("decoded inventory decision")
                    .projected = true;
            }
        }
        Ok(())
    }

    /// Ordered, unacknowledged physical debits whose regional heal must replay.
    pub(crate) fn pending_treatments(
        &self,
    ) -> Result<
        Vec<(
            u64,
            u64,
            crate::script::storage::world_inventory::DurableTreatmentIntent,
            bool,
        )>,
        WorldChunkJournalError,
    > {
        let state = self.shared.lock_state();
        let mut pending = Vec::new();
        for decision in &state.pending {
            if decision
                .inventory
                .as_ref()
                .is_none_or(|inventory| inventory.projected)
            {
                continue;
            }
            if let Some(batch) = decision.inventory_batch()?
                && let Some(treatment) = batch.treatment()
            {
                pending.push((
                    decision.id,
                    batch.transaction_id(),
                    treatment.clone(),
                    decision
                        .inventory
                        .as_ref()
                        .expect("medical inventory")
                        .native_committed,
                ));
            }
        }
        Ok(pending)
    }
}

#[cfg(test)]
#[path = "world_inventory_journal_tests.rs"]
mod tests;
