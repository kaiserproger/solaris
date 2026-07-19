use std::collections::HashSet;
use std::sync::Arc;

use mc_data::block_light::BlockLightTable;
use mc_protocol::packets::play::BlockChangedAck;
use mc_world::light::ChunkLight;
use mc_world::{ChunkPos, ScheduledBlockTick};
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
    block_edit_changes_light, dispatch_campfire_block_entity_update,
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
    let outcome =
        apply_block_edit_batch_to_storage_conditionally(storage, table, edits, preconditions)?;
    let applied_positions = outcome
        .applied
        .iter()
        .map(|edit| edit.pos)
        .collect::<HashSet<_>>();
    for tick in scheduled_block_ticks {
        if applied_positions.contains(&tick.pos)
            && let Err(error) = storage.schedule_block_tick(tick.clone())
        {
            warn!(%error, pos = ?tick.pos, "simulation block tick scheduling failed");
        }
    }
    Some(outcome)
}

pub(super) fn apply_block_edit_batch_to_storage_conditionally(
    storage: &mut mc_world::WorldStorage,
    table: Option<&BlockLightTable>,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
) -> Option<BlockEditBatchOutcome> {
    let mut outcome = BlockEditBatchOutcome::default();
    for precondition in preconditions {
        let current = match storage.get_block(precondition.pos) {
            Ok(current) => current,
            Err(err) => {
                warn!(
                    error = %err,
                    x = precondition.pos.x,
                    y = precondition.pos.y,
                    z = precondition.pos.z,
                    "conditional block edit precondition read failed"
                );
                return None;
            }
        };
        if current != Some(precondition.expected_state)
            || storage.block_mutation_token(precondition.pos) != Some(precondition.expected_token)
        {
            return None;
        }
    }
    for edit in edits {
        apply_block_edit_to_storage(storage, table, edit, &mut outcome);
    }
    Some(outcome)
}

pub(super) fn apply_opaque_block_entity_to_storage_conditionally(
    storage: &mut mc_world::WorldStorage,
    position: mc_world::BlockPos,
    expected_state: mc_world::BlockStateId,
    expected_token: mc_world::BlockMutationToken,
    bytes: Vec<u8>,
) -> Result<bool, mc_world::WorldError> {
    if storage.get_block(position)? != Some(expected_state)
        || storage.block_mutation_token(position) != Some(expected_token)
    {
        return Ok(false);
    }
    storage.set_opaque_block_entity(position, bytes)
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
    let pos = edit.pos;
    let chunk_pos = ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    let preserves_baked_light = table.is_some_and(|table| {
        storage
            .get_cached_block(pos)
            .is_some_and(|previous| !block_edit_changes_light(table, previous, edit.new_state))
    });
    let previous_light = if table.is_some() && !preserves_baked_light {
        match storage.get_chunk(chunk_pos) {
            Ok(Some(chunk)) => ChunkLight::from_chunk(chunk),
            Ok(None) => None,
            Err(err) => {
                warn!(error = %err, cx = chunk_pos.x, cz = chunk_pos.z, "pre-edit baked light read failed");
                None
            }
        }
    } else {
        None
    };
    let edit_result = if preserves_baked_light {
        storage.set_block_at_preserving_light(pos, edit.new_state)
    } else {
        storage.set_block_at(pos, edit.new_state)
    };
    match edit_result {
        Ok(Some(previous)) if previous != edit.new_state => {
            let changes_light = table
                .is_some_and(|table| block_edit_changes_light(table, previous, edit.new_state));
            if changes_light
                && let Some(table) = table
                && let Err(err) = storage.update_highest_opaque_at(pos, table)
            {
                warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "highest-opaque heightmap update failed");
            }
            outcome.applied.push(AppliedBlockEdit {
                pos,
                previous,
                new_state: edit.new_state,
            });
            if let Some(token) = storage.block_mutation_token(pos) {
                outcome.resulting_tokens.insert(pos, token);
            }
            outcome.deltas.push(BlockDelta {
                x: pos.x,
                y: pos.y,
                z: pos.z,
                state_id: edit.new_state,
            });
            let chunk = (pos.x.div_euclid(16), pos.z.div_euclid(16));
            outcome.edit_chunks.insert(chunk);
            if changes_light {
                outcome.light_edit_chunks.insert(chunk);
                if let Some(light) = previous_light {
                    outcome.previous_light_chunks.entry(chunk).or_insert(light);
                }
            } else if let Some(light) = previous_light {
                match storage.set_baked_light(chunk_pos, &light) {
                    Ok(_) => {}
                    Err(err) => {
                        warn!(error = %err, cx = chunk_pos.x, cz = chunk_pos.z, "light-inert edit baked light restore failed");
                    }
                }
            }
        }
        Ok(Some(_)) | Ok(None) => {}
        Err(err) => {
            warn!(error = %err, x = pos.x, y = pos.y, z = pos.z, "set_block_at failed; skipping edit");
        }
    }
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
    send_block_deltas(writer, state.compression, &deltas).await
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
    send_block_deltas(writer, state.compression, &outcome.deltas).await?;
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

pub(super) async fn apply_player_block_edit_batch<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    edits: &[BlockEdit],
) -> Result<BlockEditBatchOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    apply_player_block_edit_batch_conditionally(state, writer, sequence, edits, &[], &[]).await
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
