use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::{
    BlockPos, ChestBlockEntity, ResidentBlockEditBatchResult, ResidentBlockEntityChange,
    ResidentBlockPrecondition, ResidentChunkTransaction, ResidentCrossRegionStagedChunk,
    WorldMutationView, chunk_pos_of, region_of,
};

impl WorldMutationView {
    /// Stages canonical chest after-images without publishing or persisting.
    /// Every participant requires its current block token as well as contents;
    /// the returned transaction rechecks all source chunks at durable commit.
    /// `revision` is the reserved world-journal decision id.
    pub fn prepare_chest_inventory_transaction(
        &self,
        changes: &[ResidentBlockEntityChange<ChestBlockEntity>],
        preconditions: &[ResidentBlockPrecondition],
        revision: u64,
    ) -> Result<ResidentChunkTransaction, ResidentBlockEditBatchResult> {
        if changes.is_empty()
            || changes.len() != preconditions.len()
            || revision == 0
            || revision > i64::MAX as u64
        {
            return Err(ResidentBlockEditBatchResult::Stale);
        }
        let mut positions = BTreeSet::new();
        let mut by_chunk = BTreeMap::new();
        for (change, precondition) in changes.iter().zip(preconditions) {
            if change.position != precondition.pos
                || !positions.insert(position_key(change.position))
            {
                return Err(ResidentBlockEditBatchResult::Stale);
            }
            let chunk = chunk_pos_of(change.position);
            by_chunk
                .entry((region_of(chunk), chunk.x, chunk.z))
                .or_insert_with(Vec::new)
                .push((change, precondition));
        }
        let publication = self.resident.read_view.publication_state();
        let _admission = publication.mutation();
        let mut chunks = Vec::with_capacity(by_chunk.len());
        let mut touched = Vec::with_capacity(by_chunk.len());
        for entries in by_chunk.into_values() {
            let position = chunk_pos_of(entries[0].0.position);
            let region = self
                .resident
                .region(position)
                .ok_or(ResidentBlockEditBatchResult::Missing)?;
            let region = region
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let source = region
                .chunks
                .get(&position)
                .ok_or(ResidentBlockEditBatchResult::Missing)?;
            if region.pending_journal_lsn.contains_key(&position)
                || source.world_journal_lsn() >= revision
            {
                return Err(ResidentBlockEditBatchResult::Stale);
            }
            for (change, precondition) in &entries {
                let pos = change.position;
                let x = pos.x.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
                let z = pos.z.rem_euclid(crate::section::SECTION_DIM as i32) as u8;
                let chest_block = self
                    .resident
                    .registry
                    .by_id(precondition.expected_state)
                    .is_some_and(|state| matches!(state.block.id.path(), "chest" | "barrel"));
                let contents_match = source.chests.get(&pos).map_or_else(
                    || change.expected == ChestBlockEntity::default(),
                    |current| current == &change.expected,
                );
                if source.get_block(x, pos.y, z) != Some(precondition.expected_state)
                    || source.block_mutation_token(x, pos.y, z) != Some(precondition.expected_token)
                    || !chest_block
                    || !contents_match
                {
                    return Err(ResidentBlockEditBatchResult::Stale);
                }
            }
            let mut staged = source.as_ref().clone();
            for (change, _) in entries {
                staged
                    .chests
                    .insert(change.position, change.updated.clone());
            }
            staged.set_world_journal_lsn(revision);
            chunks.push(ResidentCrossRegionStagedChunk {
                position,
                expected: Some(Arc::clone(source)),
                staged: Some(Arc::new(staged)),
            });
            touched.push(position);
        }
        Ok(ResidentChunkTransaction {
            resident: self.resident.clone(),
            chunks,
            applied: Vec::new(),
            touched,
            #[cfg(test)]
            publish_hook: None,
        })
    }
}

fn position_key(position: BlockPos) -> (i32, i32, i32) {
    (position.x, position.y, position.z)
}
