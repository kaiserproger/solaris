//! Headless authoritative block-edit commits on [`WorldStorage`].
//!
//! Prepares every required chunk before applying edits, then funnels
//! both single-region and cross-region batches through the resident commit
//! kernel. Cross-region batches reuse the staged scheduled-block transaction
//! with a non-durable commit; there is no new queue, owner, retry or
//! durability path here.

use crate::{BlockMutationToken, BlockPos, BlockStateId};
use mc_data::block_light::BlockLightTable;

use crate::chunk::{ChunkPos, ScheduledBlockTick};
use crate::resident::{
    ResidentBlockEdit, ResidentBlockEditBatchResult, ResidentBlockPrecondition,
    ResidentCrossRegionBlockEditPlan, ResidentCrossRegionScheduledBlockTickPrepareResult,
    ResidentOpaqueBlockEntityCommitResult,
};

use super::{WorldError, WorldStorage, chunk_pos_of};

#[cfg(test)]
#[path = "block_edits_tests.rs"]
mod tests;

impl WorldStorage {
    /// Write opaque block-entity data only if state and token still match.
    ///
    /// `bytes` is a network-format NBT compound (nameless root), matching the
    /// opaque representation retained by [`crate::Chunk::block_entities`].
    ///
    /// Loads the chunk before entering the resident commit. Validation and
    /// publication share its region lock. Matching, unchanged data is accepted;
    /// a missing position or failed precondition returns `false` without a write.
    pub fn commit_opaque_block_entity_conditionally(
        &mut self,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> Result<bool, WorldError> {
        if self.ensure_chunk(chunk_pos_of(position))?.is_none() {
            return Ok(false);
        }
        Ok(matches!(
            self.mutation_view()
                .commit_opaque_block_entity_conditionally(
                    position,
                    expected_state,
                    expected_token,
                    bytes,
                ),
            ResidentOpaqueBlockEntityCommitResult::Applied
        ))
    }

    /// Apply conditional block edits headlessly, loading required chunks first.
    ///
    /// Prepares every edit, precondition and newly scheduled tick position
    /// before applying any edits. Loading errors surface as [`WorldError`];
    /// absent or out-of-bounds positions reject the whole batch as
    /// [`ResidentBlockEditBatchResult::Missing`]. Failed preconditions return
    /// [`ResidentBlockEditBatchResult::Stale`] without partial mutation.
    /// Single-region batches use the resident fast path; cross-region batches
    /// reuse the staged transaction with a non-durable commit.
    pub fn apply_block_edits_conditionally(
        &mut self,
        edits: &[ResidentBlockEdit],
        preconditions: &[ResidentBlockPrecondition],
        scheduled_block_ticks: &[ScheduledBlockTick],
        light_table: Option<&BlockLightTable>,
        leaf_trigger_tick: Option<u64>,
    ) -> Result<ResidentBlockEditBatchResult, WorldError> {
        let mut positions = edits
            .iter()
            .map(|edit| edit.pos)
            .chain(preconditions.iter().map(|precondition| precondition.pos))
            .chain(scheduled_block_ticks.iter().map(|tick| tick.pos));
        let mut required = positions
            .clone()
            .map(chunk_pos_of)
            .collect::<Vec<ChunkPos>>();
        required.sort_unstable_by_key(|position| (position.x, position.z));
        required.dedup();
        for position in required {
            if self.ensure_chunk(position)?.is_none() {
                return Ok(ResidentBlockEditBatchResult::Missing);
            }
        }
        if positions.any(|position| self.get_cached_block(position).is_none()) {
            return Ok(ResidentBlockEditBatchResult::Missing);
        }

        let result = self.mutation_view().apply_block_edits_conditionally(
            edits,
            preconditions,
            scheduled_block_ticks,
            light_table,
            leaf_trigger_tick,
        );
        if !matches!(result, ResidentBlockEditBatchResult::CrossRegion) {
            return Ok(result);
        }
        let plan = ResidentCrossRegionBlockEditPlan {
            edits,
            preconditions,
            scheduled_block_ticks,
            light_table,
            leaf_trigger_tick,
        };
        match self
            .resident
            .mutation_view()
            .prepare_cross_region_block_edit_transaction(&plan)
        {
            ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) => {
                Ok(transaction.commit_nondurably())
            }
            ResidentCrossRegionScheduledBlockTickPrepareResult::Missing => {
                Ok(ResidentBlockEditBatchResult::Missing)
            }
            ResidentCrossRegionScheduledBlockTickPrepareResult::Stale => {
                Ok(ResidentBlockEditBatchResult::Stale)
            }
        }
    }
}
