use std::collections::HashSet;
use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_protocol::packets::play::BlockChangedAck;
use mc_world::{
    ResidentBlockEdit, ResidentBlockEditBatchResult, ResidentBlockPrecondition, ScheduledBlockTick,
};
use tokio::io::AsyncWriteExt;
#[cfg(not(test))]
use tracing::debug;
use tracing::warn;

use crate::connection::write_packet;
use crate::error::ConnectionError;

use super::block_wire::{
    BlockDelta, broadcast_block_deltas, broadcast_light_updates, send_block_deltas,
    send_light_updates,
};
use super::campfire::{CampfireCookingState, is_campfire_block};
use super::lighting::collect_incremental_light_updates_for_applied_edits;
use super::{
    AppliedBlockEdit, BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition, InteractionState,
    dispatch_campfire_block_entity_update,
};

#[cfg(test)]
async fn apply_block_edit_batch_to_world_conditionally(
    state: &mut InteractionState,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    let table = state.block_light.as_ref().map(Arc::clone);
    let mut storage = state.world.lock().await;
    let outcome = apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
        &mut storage,
        table.as_deref(),
        edits,
        preconditions,
        scheduled_block_ticks,
    );
    drop(storage);
    outcome
}

pub(super) fn apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    apply_storage_block_edits(storage, table, edits, preconditions, scheduled_block_ticks)
}

pub(super) fn apply_block_edit_batch_to_storage_conditionally(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
) -> Option<BlockEditBatchOutcome> {
    apply_storage_block_edits(storage, table, edits, preconditions, &[])
}

fn apply_storage_block_edits(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Option<BlockEditBatchOutcome> {
    let resident_edits = resident_block_edits(edits);
    let resident_preconditions = resident_block_preconditions(preconditions);
    // Leaf ticks stay with their existing owners (the simulation coordinator
    // schedules them near applied edits), so the storage commit must not
    // duplicate them.
    match storage.apply_block_edits_conditionally(
        &resident_edits,
        &resident_preconditions,
        scheduled_block_ticks,
        table,
        None,
    ) {
        Ok(ResidentBlockEditBatchResult::Applied(applied)) => {
            resident_block_edit_result_outcome(ResidentBlockEditBatchResult::Applied(applied))
        }
        Ok(ResidentBlockEditBatchResult::Stale) => None,
        Ok(ResidentBlockEditBatchResult::Missing) => {
            warn!("conditional block edit rejected: position is not loaded");
            None
        }
        Ok(ResidentBlockEditBatchResult::CrossRegion) => {
            warn!("conditional block edit rejected: cross-region batch");
            None
        }
        Err(error) => {
            warn!(%error, "conditional block edit storage commit failed");
            None
        }
    }
}

fn replaced_campfire_with_non_campfire(state: &InteractionState, edit: &AppliedBlockEdit) -> bool {
    is_campfire_block(&state.blocks, edit.previous)
        && !is_campfire_block(&state.blocks, edit.new_state)
}

pub(super) fn apply_block_edit_to_storage(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edit: &BlockEdit,
    outcome: &mut BlockEditBatchOutcome,
) {
    let resident_edit = [ResidentBlockEdit {
        pos: edit.pos,
        new_state: edit.new_state,
        // The resident kernel derives light preservation from the actual
        // previous state plus the light table; no precondition scan needed.
        preserve_light: false,
    }];
    let applied = match storage.apply_block_edits_conditionally(
        &resident_edit,
        &[],
        &[],
        table,
        None,
    ) {
        Ok(ResidentBlockEditBatchResult::Applied(applied)) => applied,
        Ok(
            ResidentBlockEditBatchResult::Stale
            | ResidentBlockEditBatchResult::Missing
            | ResidentBlockEditBatchResult::CrossRegion,
        ) => return,
        Err(error) => {
            warn!(error = %error, x = edit.pos.x, y = edit.pos.y, z = edit.pos.z, "set_block_at failed; skipping edit");
            return;
        }
    };
    let Some(additional) =
        resident_block_edit_result_outcome(ResidentBlockEditBatchResult::Applied(applied))
    else {
        return;
    };
    outcome.applied.extend(additional.applied);
    outcome.resulting_tokens.extend(additional.resulting_tokens);
    outcome.deltas.extend(additional.deltas);
    outcome.edit_chunks.extend(additional.edit_chunks);
    outcome
        .light_edit_chunks
        .extend(additional.light_edit_chunks);
    for (chunk, light) in additional.previous_light_chunks {
        outcome.previous_light_chunks.entry(chunk).or_insert(light);
    }
    debug_assert!(additional.cleared_campfires.is_empty());
    debug_assert!(additional.precomputed_light_updates.is_none());
    debug_assert!(additional.pending_light_sources.is_none());
}

pub(super) fn resident_block_edits(edits: &[BlockEdit]) -> Vec<ResidentBlockEdit> {
    edits
        .iter()
        .map(|edit| ResidentBlockEdit {
            pos: edit.pos,
            new_state: edit.new_state,
            // The resident kernel derives light preservation from the actual
            // previous state plus the light table; precondition scans here
            // would only duplicate that work with potentially stale state.
            preserve_light: false,
        })
        .collect()
}

pub(super) fn resident_block_preconditions(
    preconditions: &[BlockEditPrecondition],
) -> Vec<ResidentBlockPrecondition> {
    preconditions
        .iter()
        .map(|precondition| ResidentBlockPrecondition {
            pos: precondition.pos,
            expected_state: precondition.expected_state,
            expected_token: precondition.expected_token,
        })
        .collect()
}

pub(super) fn resident_block_edit_result_outcome(
    result: ResidentBlockEditBatchResult,
) -> Option<BlockEditBatchOutcome> {
    let ResidentBlockEditBatchResult::Applied(applied) = result else {
        return None;
    };
    let mut outcome = BlockEditBatchOutcome::default();
    for edit in applied {
        let chunk = (edit.pos.x.div_euclid(16), edit.pos.z.div_euclid(16));
        let changes_light = edit.changes_light;
        if let Some(previous_light) = edit.previous_light {
            outcome
                .previous_light_chunks
                .entry(chunk)
                .or_insert(previous_light);
        }
        outcome.applied.push(AppliedBlockEdit {
            pos: edit.pos,
            previous: edit.previous,
            new_state: edit.new_state,
        });
        outcome
            .resulting_tokens
            .insert(edit.pos, edit.resulting_token);
        outcome.deltas.push(BlockDelta {
            x: edit.pos.x,
            y: edit.pos.y,
            z: edit.pos.z,
            state_id: edit.new_state,
        });
        outcome.edit_chunks.insert(chunk);
        if changes_light {
            outcome.light_edit_chunks.insert(chunk);
        }
    }
    Some(outcome)
}

pub(super) async fn send_loaded_block_edit_resyncs<W>(
    state: &InteractionState,
    writer: &mut W,
    edits: &[BlockEdit],
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut seen = HashSet::with_capacity(edits.len());
    let deltas = edits
        .iter()
        .filter_map(|edit| {
            if !seen.insert(edit.pos) {
                return None;
            }
            state
                .world_read
                .get_cached_block(edit.pos)
                .map(|state_id| BlockDelta {
                    x: edit.pos.x,
                    y: edit.pos.y,
                    z: edit.pos.z,
                    state_id,
                })
        })
        .collect::<Vec<_>>();
    let projection = state
        .sessions
        .loader_block_projection(state.session_id, &state.blocks);
    send_block_deltas(writer, state.compression, &deltas, projection.as_ref()).await
}

pub(super) async fn apply_visible_block_edit_batch_conditionally<W>(
    state: &mut InteractionState,
    writer: &mut W,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Result<Option<BlockEditBatchOutcome>, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let outcome = {
        #[cfg(test)]
        {
            apply_block_edit_batch_to_world_conditionally(
                state,
                edits,
                preconditions,
                scheduled_block_ticks,
            )
            .await
        }
        #[cfg(not(test))]
        {
            match state
                .simulation
                .apply_block_edits_with_scheduled_ticks(
                    edits.to_vec(),
                    preconditions.to_vec(),
                    scheduled_block_ticks.to_vec(),
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    debug!(?error, "simulation block edit rejected");
                    None
                }
            }
        }
    };
    let Some(outcome) = outcome else {
        return Ok(None);
    };

    #[cfg(test)]
    let broadcast_peer_blocks = true;
    #[cfg(not(test))]
    let broadcast_peer_blocks = false;
    finalize_visible_block_edit_outcome(state, writer, outcome, broadcast_peer_blocks)
        .await
        .map(Some)
}

pub(super) async fn finalize_visible_block_edit_outcome<W>(
    state: &mut InteractionState,
    writer: &mut W,
    mut outcome: BlockEditBatchOutcome,
    broadcast_peer_blocks: bool,
) -> Result<BlockEditBatchOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let table = state.block_light.as_ref().map(Arc::clone);

    for applied in &outcome.applied {
        if !replaced_campfire_with_non_campfire(state, applied) {
            continue;
        }
        if state.sessions.clear_campfire_cooking(applied.pos) {
            outcome.cleared_campfires.push(applied.pos);
        }
    }

    if outcome.applied.is_empty() {
        return Ok(outcome);
    }

    state
        .sessions
        .invalidate_prepared_chunks(&outcome.edit_chunks);
    let projection = state
        .sessions
        .loader_block_projection(state.session_id, &state.blocks);
    send_block_deltas(
        writer,
        state.compression,
        &outcome.deltas,
        projection.as_ref(),
    )
    .await?;
    if broadcast_peer_blocks {
        broadcast_block_deltas(
            state,
            &outcome.edit_chunks,
            &outcome.deltas,
            Some(state.session_id),
        );
    }
    for pos in &outcome.cleared_campfires {
        dispatch_campfire_block_entity_update(
            &state.items,
            &state.sessions,
            None,
            *pos,
            &CampfireCookingState::default(),
        );
    }

    if let Some(table) = table {
        let light_updates = if let Some(updates) = outcome.precomputed_light_updates.take() {
            updates
        } else {
            let mut storage = state.world.lock().await;
            collect_incremental_light_updates_for_applied_edits(&mut storage, &table, &outcome)
        };
        let light_chunks: HashSet<_> = light_updates
            .iter()
            .map(|update| (update.pos.x, update.pos.z))
            .collect();
        state.sessions.invalidate_prepared_chunks(&light_chunks);
        send_light_updates(state, writer, &light_updates).await?;
        if broadcast_peer_blocks {
            broadcast_light_updates(state, &light_updates, Some(state.session_id));
        }
    }

    Ok(outcome)
}

pub(super) async fn apply_player_block_edit_batch_conditionally<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    scheduled_block_ticks: &[ScheduledBlockTick],
) -> Result<BlockEditBatchOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let outcome = match apply_visible_block_edit_batch_conditionally(
        state,
        writer,
        edits,
        preconditions,
        scheduled_block_ticks,
    )
    .await?
    {
        Some(outcome) => outcome,
        None => {
            send_loaded_block_edit_resyncs(state, writer, edits).await?;
            BlockEditBatchOutcome::default()
        }
    };

    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    Ok(outcome)
}
