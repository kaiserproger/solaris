use std::sync::Arc;

use mc_entity::Vec3;
use mc_protocol::packets::play::{
    Direction, GameMode, ItemStack, ServerboundPlayerAction, unpack_block_pos,
};
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::error::ConnectionError;

use super::block_edit_commit::{
    apply_player_block_edit_batch_conditionally, finalize_visible_block_edit_outcome,
};
use super::block_placement::plan_stair_state_transition;
use super::block_wire::broadcast_level_event;
use super::fluids::supported_flow_state;
use super::session::{dispatch_visibility_commands, within_block_reach};
use super::simulation::{SurvivalBlockBreakPlan, SurvivalBreakDrop, SurvivalBreakHeldItem};
use super::survival;
use super::survival::{
    BlockMutationSnapshot, SurvivalState, falling_block_entity_type_id, held_item_id,
    held_item_stack, item_entity_type_id, max_tool_damage_for_path, mining_progress_for_block,
    mining_target_for,
};
use super::{
    BlockEdit, BlockEditPrecondition, BlockPlanningRead, InteractionState, PlayerPose,
    ScriptGameplayEventPublisher, XpState, air_state_id, commit_player_survival_update,
    schedule_fluid_ticks_for_interaction, splitmix64, start_falling_blocks_after_edits,
    write_block_ack, write_block_resync, write_inventory_slot_updates,
};

const VANILLA_STOP_DESTROY_THRESHOLD: f32 = 0.7;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PendingBreak {
    pub(super) sequence: i32,
    pub(super) position: i64,
    pub(super) direction: Direction,
    pub(super) started_tick: u64,
    pub(super) started_progress_per_tick: f32,
    pub(super) held_hotbar_slot: u8,
    pub(super) held_item: Option<ItemStack>,
    pub(super) expected_target: Option<BlockMutationSnapshot>,
    pub(super) stop_received: bool,
}

#[derive(Debug, PartialEq)]
pub(super) enum StopBreakOutcome {
    Complete(BlockBreakCompletion),
    Acknowledge { delayed: bool },
}

#[derive(Debug, PartialEq)]
pub(super) enum DelayedBreakOutcome {
    Idle,
    Pending,
    Cancelled,
    Complete(BlockBreakCompletion),
}

#[derive(Debug, PartialEq)]
pub(super) struct BlockBreakCompletion {
    pub(super) pending: PendingBreak,
    pub(super) acknowledgement: BreakAcknowledgement,
}

pub(super) struct BlockBreakState<'a> {
    active: &'a mut Option<PendingBreak>,
    delayed: &'a mut Option<PendingBreak>,
}

impl<'a> BlockBreakState<'a> {
    pub(super) fn new(
        active: &'a mut Option<PendingBreak>,
        delayed: &'a mut Option<PendingBreak>,
    ) -> Self {
        Self { active, delayed }
    }

    pub(super) fn start(&mut self, pending: PendingBreak) {
        *self.active = Some(pending);
    }

    pub(super) fn stop(
        &mut self,
        action: &ServerboundPlayerAction,
        current_tick: u64,
        progress_per_tick: f32,
    ) -> StopBreakOutcome {
        let Some(pending) = self.active.as_ref() else {
            return StopBreakOutcome::Acknowledge { delayed: false };
        };
        if pending.position != action.position {
            return StopBreakOutcome::Acknowledge { delayed: false };
        }
        if pending.expected_target.is_none() {
            *self.active = None;
            return StopBreakOutcome::Acknowledge { delayed: false };
        }
        if destroy_progress(pending.started_tick, current_tick, progress_per_tick)
            >= VANILLA_STOP_DESTROY_THRESHOLD
        {
            let pending = self.active.take().expect("checked active break");
            return StopBreakOutcome::Complete(BlockBreakCompletion {
                pending,
                acknowledgement: BreakAcknowledgement::Send(action.sequence),
            });
        }
        if self.delayed.is_none() {
            let mut pending = self.active.take().expect("checked active break");
            pending.sequence = action.sequence;
            *self.delayed = Some(pending);
            return StopBreakOutcome::Acknowledge { delayed: true };
        }
        let pending = self.active.as_mut().expect("checked active break");
        pending.sequence = action.sequence;
        pending.stop_received = true;
        StopBreakOutcome::Acknowledge { delayed: true }
    }

    pub(super) fn tick_delayed(
        &mut self,
        current_tick: u64,
        progress_per_tick: f32,
    ) -> DelayedBreakOutcome {
        let Some(pending) = self.delayed.as_ref() else {
            return DelayedBreakOutcome::Idle;
        };
        if pending.expected_target.is_none() {
            *self.delayed = None;
            self.promote_stopped_active();
            return DelayedBreakOutcome::Cancelled;
        }
        if destroy_progress(pending.started_tick, current_tick, progress_per_tick) < 1.0 {
            return DelayedBreakOutcome::Pending;
        }
        let pending = self.delayed.take().expect("checked delayed break");
        self.promote_stopped_active();
        DelayedBreakOutcome::Complete(BlockBreakCompletion {
            acknowledgement: BreakAcknowledgement::AlreadySent(pending.sequence),
            pending,
        })
    }

    fn promote_stopped_active(&mut self) {
        if self
            .active
            .as_ref()
            .is_some_and(|pending| pending.stop_received)
        {
            *self.delayed = self.active.take();
        }
    }
}

fn destroy_progress(started_tick: u64, current_tick: u64, progress_per_tick: f32) -> f32 {
    let elapsed_with_start = current_tick.saturating_sub(started_tick).saturating_add(1);
    progress_per_tick * elapsed_with_start as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BreakAcknowledgement {
    Send(i32),
    AlreadySent(i32),
}

impl BreakAcknowledgement {
    fn sequence(self) -> i32 {
        match self {
            Self::Send(sequence) | Self::AlreadySent(sequence) => sequence,
        }
    }

    pub(super) fn should_send(self) -> bool {
        matches!(self, Self::Send(_))
    }
}

#[allow(clippy::too_many_arguments)]
async fn complete_block_break<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    game_mode: GameMode,
    player_pose: PlayerPose,
    acknowledgement: BreakAcknowledgement,
    position: i64,
    drop_items: bool,
    expected_target: Option<BlockMutationSnapshot>,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let sequence = acknowledgement.sequence();
    let (x, y, z) = unpack_block_pos(position);
    let pos = mc_world::BlockPos { x, y, z };
    if script_events.is_some_and(|events| !events.block_mutation_allowed(pos)) {
        debug!(
            sequence,
            x, y, z, "survival block break denied by plugin policy"
        );
        write_block_resync(state, writer, position).await?;
        if acknowledgement.should_send() {
            write_block_ack(writer, state.compression, sequence).await?;
        }
        return Ok(false);
    }
    if let Some(expected_target) = expected_target {
        let Some(held) = state.inventory.held(state.selected_hotbar_slot).cloned() else {
            return Ok(false);
        };
        let max_damage = if held.is_empty() {
            None
        } else {
            state
                .items
                .name_of(held.item_id)
                .and_then(|item| max_tool_damage_for_path(item.path()))
        };
        let committed = match state
            .simulation
            .commit_survival_block_break(SurvivalBlockBreakPlan {
                position: pos,
                expected_target,
                blocks: Arc::clone(&state.blocks),
                block_facts: Arc::clone(&state.block_facts),
                water: state.water,
                items: Arc::clone(&state.items),
                item_facts: Arc::clone(&state.item_facts),
                loot: Arc::clone(&state.loot),
                item_entity_type_id: item_entity_type_id(&state.entity_types),
                falling_block_entity_type_id: falling_block_entity_type_id(&state.entity_types),
                held: SurvivalBreakHeldItem {
                    hotbar_slot: state.selected_hotbar_slot,
                    expected: held,
                    max_damage,
                },
                drop_items,
            })
            .await
        {
            Ok(Some(committed)) => committed,
            Ok(None) => {
                debug!(
                    sequence,
                    x,
                    y,
                    z,
                    expected_state = expected_target.state.0,
                    expected_chunk_instance = expected_target.token.chunk_instance_id,
                    expected_version = expected_target.token.version,
                    "survival block break rejected after transaction precondition changed"
                );
                write_block_resync(state, writer, position).await?;
                if acknowledgement.should_send() {
                    write_block_ack(writer, state.compression, sequence).await?;
                }
                return Ok(false);
            }
            Err(error) => {
                debug!(
                    ?error,
                    sequence, x, y, z, "simulation survival break rejected"
                );
                write_block_resync(state, writer, position).await?;
                if acknowledgement.should_send() {
                    write_block_ack(writer, state.compression, sequence).await?;
                }
                return Ok(false);
            }
        };
        let destroyed_state = committed
            .block
            .applied
            .iter()
            .find(|edit| {
                edit.pos == pos
                    && edit.previous == expected_target.state
                    && edit.previous != edit.new_state
            })
            .map(|edit| edit.previous);
        if !committed.block.applied.is_empty() {
            dispatch_visibility_commands(
                state.sessions.broadcast_player_animation(state.session_id),
            );
        }
        state.inventory = committed.inventory;
        let changed_slots = committed.changed_slots;
        if let Some(destroyed_state) = destroyed_state {
            if let Some(script_events) = script_events {
                script_events
                    .publish_block_broken(
                        &state.blocks,
                        destroyed_state,
                        pos,
                        player_pose,
                        game_mode,
                    )
                    .await;
            }
            broadcast_level_event(
                state,
                pos,
                2001,
                destroyed_state.0 as i32,
                Some(state.session_id),
            );
        }
        let outcome =
            finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
        if acknowledgement.should_send() {
            write_block_ack(writer, state.compression, sequence).await?;
        }
        if !changed_slots.is_empty() {
            write_inventory_slot_updates(state, writer, changed_slots).await?;
        }
        let changed = !outcome.applied.is_empty();
        debug!(sequence, x, y, z, changed, "survival block break committed");
        return Ok(changed);
    }

    if !acknowledgement.should_send() {
        warn!(
            sequence,
            position, "delayed block break lost its target snapshot"
        );
        return Ok(false);
    }
    let air = air_state_id(&state.blocks);
    let planned = {
        let mut storage = state.world.lock().await;
        let replacement = break_replacement_state_in_storage(
            &state.blocks,
            &state.block_facts,
            state.water,
            &*storage,
            pos,
            air,
        );
        match storage.get_block(pos) {
            Ok(Some(previous)) => {
                let edits = plan_break_block_edits(
                    &state.blocks,
                    &*storage,
                    pos,
                    previous,
                    replacement,
                    air,
                );
                storage.block_mutation_token(pos).and_then(|token| {
                    let preconditions = plan_break_edit_preconditions(
                        &state.blocks,
                        &*storage,
                        &edits,
                        pos,
                        BlockMutationSnapshot {
                            state: previous,
                            token,
                        },
                    )?;
                    Some((edits, preconditions))
                })
            }
            Ok(None) => None,
            Err(error) => {
                warn!(%error, x, y, z, "block break target read failed");
                None
            }
        }
    };
    let Some((edits, preconditions)) = planned else {
        debug!(sequence, x, y, z, "block break could not be planned");
        write_block_resync(state, writer, position).await?;
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(false);
    };
    let outcome = apply_player_block_edit_batch_conditionally(
        state,
        writer,
        sequence,
        &edits,
        &preconditions,
        &[],
    )
    .await?;
    if !outcome.applied.is_empty() {
        dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    }
    if let Some(destroyed_state) = outcome
        .applied
        .iter()
        .find(|edit| edit.pos == pos && edit.previous != edit.new_state)
        .map(|edit| edit.previous)
    {
        if let Some(script_events) = script_events {
            script_events
                .publish_block_broken(&state.blocks, destroyed_state, pos, player_pose, game_mode)
                .await;
        }
        broadcast_level_event(
            state,
            pos,
            2001,
            destroyed_state.0 as i32,
            Some(state.session_id),
        );
    }
    let changed = !outcome.applied.is_empty();
    if changed {
        schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;
        start_falling_blocks_after_edits(state, writer, &outcome.applied).await?;
    }
    Ok(changed)
}

pub(super) fn plan_break_edit_preconditions(
    blocks: &mc_world::BlockRegistry,
    storage: &impl BlockPlanningRead,
    edits: &[BlockEdit],
    root: mc_world::BlockPos,
    expected_root: BlockMutationSnapshot,
) -> Option<Vec<BlockEditPrecondition>> {
    let root_edit = edits.iter().find(|edit| edit.pos == root)?;
    let transition = plan_stair_state_transition(
        blocks,
        storage,
        root,
        expected_root.state,
        root_edit.new_state,
    )?;
    let mut preconditions = edits
        .iter()
        .map(|edit| {
            if edit.pos == root {
                return Some(BlockEditPrecondition {
                    pos: root,
                    expected_state: expected_root.state,
                    expected_token: expected_root.token,
                });
            }
            Some(BlockEditPrecondition {
                pos: edit.pos,
                expected_state: storage.get_cached_block(edit.pos)?,
                expected_token: storage.block_mutation_token(edit.pos)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    for precondition in transition.dependency_preconditions {
        if !preconditions
            .iter()
            .any(|existing| existing.pos == precondition.pos)
        {
            preconditions.push(precondition);
        }
    }
    Some(preconditions)
}

pub(super) fn plan_survival_break_drops(
    request: &SurvivalBlockBreakPlan,
    edits: &[BlockEdit],
    preconditions: &[BlockEditPrecondition],
    air: mc_world::BlockStateId,
) -> Vec<SurvivalBreakDrop> {
    let Some(entity_type_id) = request.item_entity_type_id else {
        return Vec::new();
    };
    let held_item = (!request.held.expected.is_empty()).then_some(&request.held.expected);
    edits
        .iter()
        .zip(preconditions)
        .filter(|(edit, precondition)| {
            edit.pos == request.position
                || (edit.new_state == air
                    && is_vertical_support_cascade_state(
                        &request.blocks,
                        precondition.expected_state,
                    ))
        })
        .flat_map(|(edit, precondition)| {
            let loot_seed = block_break_loot_seed(
                edit.pos,
                precondition.expected_state,
                precondition.expected_token,
            );
            survival::block_drop_stacks_with_tool_and_facts_from_seeded(
                &request.loot,
                &request.items,
                &request.item_facts,
                &request.blocks,
                precondition.expected_state,
                held_item,
                loot_seed,
            )
            .into_iter()
            .map(move |drop| SurvivalBreakDrop {
                entity_type_id,
                position: Vec3::new(
                    f64::from(edit.pos.x) + 0.5,
                    f64::from(edit.pos.y) + 0.5,
                    f64::from(edit.pos.z) + 0.5,
                ),
                stack: survival::entity_item_stack(drop),
            })
        })
        .collect()
}

pub(super) fn block_break_loot_seed(
    position: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    token: mc_world::BlockMutationToken,
) -> u64 {
    let mut seed = token.chunk_instance_id ^ token.version.rotate_left(29);
    seed = splitmix64(seed ^ u64::from(state.0));
    seed = splitmix64(seed ^ (position.x as i64 as u64).rotate_left(11));
    seed = splitmix64(seed ^ (position.y as i64 as u64).rotate_left(31));
    splitmix64(seed ^ (position.z as i64 as u64).rotate_left(47))
}

pub(super) fn plan_break_block_edits(
    blocks: &mc_world::BlockRegistry,
    storage: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    state_id: mc_world::BlockStateId,
    replacement: mc_world::BlockStateId,
    air: mc_world::BlockStateId,
) -> Vec<BlockEdit> {
    if state_id == replacement {
        return Vec::new();
    }
    let Some(transition) = plan_stair_state_transition(blocks, storage, pos, state_id, replacement)
    else {
        return Vec::new();
    };
    let mut edits = vec![BlockEdit {
        pos,
        new_state: transition.target_state,
    }];
    edits.extend(transition.neighbor_edits);
    let Some(state) = blocks.by_id(state_id) else {
        return Vec::new();
    };
    if state.block.id.path().ends_with("_door") {
        let other_y = match super::block_state_property(state, "half") {
            Some("lower") => pos.y + 1,
            Some("upper") => pos.y - 1,
            _ => return edits,
        };
        let other_pos = mc_world::BlockPos { y: other_y, ..pos };
        if let Some(other_state_id) = storage.get_cached_block(other_pos)
            && let Some(other_state) = blocks.by_id(other_state_id)
            && other_state.block.id == state.block.id
        {
            edits.push(BlockEdit {
                pos: other_pos,
                new_state: air,
            });
        }
    }
    append_vertical_support_cascade(blocks, storage, &mut edits, pos, air);
    edits
}

fn append_vertical_support_cascade(
    blocks: &mc_world::BlockRegistry,
    storage: &impl BlockPlanningRead,
    edits: &mut Vec<BlockEdit>,
    base: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) {
    let mut y = base.y + 1;
    loop {
        let pos = mc_world::BlockPos { y, ..base };
        let Some(state_id) = storage.get_cached_block(pos) else {
            break;
        };
        let Some(state) = blocks.by_id(state_id) else {
            break;
        };
        if !is_vertical_support_cascade_block(state.block.id.path()) {
            break;
        }
        edits.push(BlockEdit {
            pos,
            new_state: air,
        });
        y += 1;
    }
}

fn is_vertical_support_cascade_block(path: &str) -> bool {
    matches!(path, "sugar_cane" | "cactus" | "bamboo")
}

fn is_vertical_support_cascade_state(
    blocks: &mc_world::BlockRegistry,
    state_id: mc_world::BlockStateId,
) -> bool {
    blocks
        .by_id(state_id)
        .is_some_and(|state| is_vertical_support_cascade_block(state.block.id.path()))
}

pub(super) fn break_replacement_state_in_storage(
    blocks: &mc_world::BlockRegistry,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    water: Option<mc_world::BlockStateId>,
    storage: &impl BlockPlanningRead,
    pos: mc_world::BlockPos,
    air: mc_world::BlockStateId,
) -> mc_world::BlockStateId {
    let mc_world::BlockPos { x, y, z } = pos;
    let neighbours = [
        (x, y + 1, z),
        (x + 1, y, z),
        (x - 1, y, z),
        (x, y, z + 1),
        (x, y, z - 1),
    ];
    let neighbour_states =
        neighbours.map(|(x, y, z)| storage.get_cached_block(mc_world::BlockPos { x, y, z }));
    for fluid in neighbour_states
        .into_iter()
        .flatten()
        .filter_map(|state_id| block_facts.fluid(state_id.0))
    {
        if let Some(flow_state) = supported_flow_state(blocks, block_facts, storage, pos, fluid) {
            return flow_state;
        }
    }

    if let Some(water) = water
        && neighbour_states
            .into_iter()
            .any(|state| state == Some(water))
    {
        return water;
    }

    air
}

/// Handles only destroy actions. Other player actions remain owned by the
/// connection-level dispatcher in `play.rs`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_block_destroy_action<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    action: ServerboundPlayerAction,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        state.pending_break = None;
        state.delayed_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "survival block break ignored for dead player"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    if !within_block_reach(player_pose, action.position, game_mode) {
        state.pending_break = None;
        state.pending_use = None;
        debug!(
            sequence = action.sequence,
            "block break ignored: target out of reach"
        );
        return write_block_ack(writer, state.compression, action.sequence).await;
    }

    match game_mode {
        GameMode::Creative => {
            state.pending_break = None;
            state.delayed_break = None;
            state.pending_use = None;
            if matches!(
                action.action,
                mc_protocol::packets::play::PlayerActionKind::AbortDestroyBlock
            ) {
                return write_block_ack(writer, state.compression, action.sequence).await;
            }
            complete_block_break(
                state,
                writer,
                script_events,
                game_mode,
                player_pose,
                BreakAcknowledgement::Send(action.sequence),
                action.position,
                false,
                None,
            )
            .await
            .map(|_| ())
        }
        GameMode::Survival => match action.action {
            mc_protocol::packets::play::PlayerActionKind::StartDestroyBlock => {
                // Vanilla starts timing when START is handled. Capture before the
                // owner snapshot await so owner-queue latency counts as progress.
                let started_tick = state.sessions.simulation_tick();
                let held_hotbar_slot = state.selected_hotbar_slot;
                let held_item = held_item_stack(state).cloned();
                let (expected_target, progress_per_tick) =
                    mining_target_for(state, action.position, player_pose).await;
                if expected_target.is_some() && progress_per_tick >= 1.0 {
                    state.pending_break = None;
                    let changed = complete_block_break(
                        state,
                        writer,
                        script_events,
                        game_mode,
                        player_pose,
                        BreakAcknowledgement::Send(action.sequence),
                        action.position,
                        true,
                        expected_target,
                    )
                    .await?;
                    apply_survival_break_exhaustion(
                        state,
                        writer,
                        survival_state,
                        xp_state,
                        player_pose,
                        changed,
                    )
                    .await?;
                    return Ok(());
                }
                BlockBreakState::new(&mut state.pending_break, &mut state.delayed_break).start(
                    PendingBreak {
                        sequence: action.sequence,
                        position: action.position,
                        direction: action.direction,
                        started_tick,
                        started_progress_per_tick: progress_per_tick,
                        held_hotbar_slot,
                        held_item: held_item.clone(),
                        expected_target,
                        stop_received: false,
                    },
                );
                debug!(
                    sequence = action.sequence,
                    position = action.position,
                    direction = ?action.direction,
                    held_slot = held_hotbar_slot,
                    held_item = ?held_item.as_ref().map(|stack| stack.item_id),
                    expected_target = ?expected_target,
                    progress_per_tick,
                    "survival block break started"
                );
                write_block_ack(writer, state.compression, action.sequence).await
            }
            mc_protocol::packets::play::PlayerActionKind::AbortDestroyBlock => {
                let pending = state.pending_break.as_ref();
                debug!(
                    sequence = action.sequence,
                    position = action.position,
                    direction = ?action.direction,
                    pending_present = pending.is_some(),
                    pending_position = pending.map(|pending| pending.position),
                    pending_direction = ?pending.map(|pending| pending.direction),
                    "survival block break aborted"
                );
                state.pending_break = None;
                write_block_ack(writer, state.compression, action.sequence).await
            }
            mc_protocol::packets::play::PlayerActionKind::StopDestroyBlock => {
                let current_tick = state.sessions.simulation_tick();
                let pending_present = state.pending_break.is_some();
                let pending_position = state.pending_break.as_ref().map(|pending| pending.position);
                let pending_direction = state
                    .pending_break
                    .as_ref()
                    .map(|pending| pending.direction);
                let pending_held_slot = state
                    .pending_break
                    .as_ref()
                    .map(|pending| pending.held_hotbar_slot);
                let pending_held_item = state
                    .pending_break
                    .as_ref()
                    .and_then(|pending| pending.held_item.as_ref())
                    .map(|stack| stack.item_id);
                let pending_expected_target = state
                    .pending_break
                    .as_ref()
                    .and_then(|pending| pending.expected_target);
                let elapsed_ticks = state
                    .pending_break
                    .as_ref()
                    .map(|pending| current_tick.saturating_sub(pending.started_tick));
                let started_progress_per_tick = state
                    .pending_break
                    .as_ref()
                    .map(|pending| pending.started_progress_per_tick);
                let current_held_stack = held_item_stack(state).cloned();
                let current_progress_per_tick = pending_expected_target.map_or(0.0, |target| {
                    mining_progress_for_block(
                        &state.blocks,
                        &state.block_facts,
                        &state.items,
                        &state.item_facts,
                        &state.tags,
                        target.state,
                        current_held_stack.as_ref(),
                        player_pose,
                    )
                });
                let outcome = BlockBreakState::new(
                    &mut state.pending_break,
                    &mut state.delayed_break,
                )
                .stop(&action, current_tick, current_progress_per_tick);
                match outcome {
                    StopBreakOutcome::Complete(completion) => {
                        let changed = complete_block_break(
                            state,
                            writer,
                            script_events,
                            game_mode,
                            player_pose,
                            completion.acknowledgement,
                            completion.pending.position,
                            true,
                            completion.pending.expected_target,
                        )
                        .await?;
                        apply_survival_break_exhaustion(
                            state,
                            writer,
                            survival_state,
                            xp_state,
                            player_pose,
                            changed,
                        )
                        .await
                    }
                    StopBreakOutcome::Acknowledge { delayed } => {
                        debug!(
                            sequence = action.sequence,
                            pending_present,
                            elapsed_ticks,
                            started_progress_per_tick,
                            current_progress_per_tick,
                            action_position = action.position,
                            action_direction = ?action.direction,
                            action_held_slot = state.selected_hotbar_slot,
                            action_held_item = ?held_item_id(state),
                            pending_position,
                            pending_direction = ?pending_direction,
                            pending_held_slot,
                            pending_held_item,
                            pending_expected_target = ?pending_expected_target,
                            delayed,
                            "survival block break stopped before completion"
                        );
                        write_block_ack(writer, state.compression, action.sequence).await
                    }
                }
            }
            _ => write_block_ack(writer, state.compression, action.sequence).await,
        },
        GameMode::Adventure | GameMode::Spectator => {
            state.pending_break = None;
            state.delayed_break = None;
            state.pending_use = None;
            debug!(
                mode = ?game_mode,
                sequence = action.sequence,
                "block break denied outside survival/creative"
            );
            write_block_ack(writer, state.compression, action.sequence).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn tick_delayed_break<W>(
    state: &mut InteractionState,
    writer: &mut W,
    script_events: Option<&ScriptGameplayEventPublisher>,
    game_mode: GameMode,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    current_tick: u64,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if game_mode != GameMode::Survival || survival_state.is_dead() {
        state.delayed_break = None;
        return Ok(());
    }
    let expected_target = state
        .delayed_break
        .as_ref()
        .and_then(|pending| pending.expected_target);
    let current_held_stack = held_item_stack(state).cloned();
    let progress_per_tick = expected_target.map_or(0.0, |target| {
        mining_progress_for_block(
            &state.blocks,
            &state.block_facts,
            &state.items,
            &state.item_facts,
            &state.tags,
            target.state,
            current_held_stack.as_ref(),
            player_pose,
        )
    });
    let outcome = BlockBreakState::new(&mut state.pending_break, &mut state.delayed_break)
        .tick_delayed(current_tick, progress_per_tick);
    let DelayedBreakOutcome::Complete(completion) = outcome else {
        return Ok(());
    };
    let (x, y, z) = unpack_block_pos(completion.pending.position);
    debug!(
        sequence = completion.acknowledgement.sequence(),
        x, y, z, "delayed survival block break reached completion"
    );

    let changed = complete_block_break(
        state,
        writer,
        script_events,
        game_mode,
        player_pose,
        completion.acknowledgement,
        completion.pending.position,
        true,
        completion.pending.expected_target,
    )
    .await?;
    apply_survival_break_exhaustion(
        state,
        writer,
        survival_state,
        xp_state,
        player_pose,
        changed,
    )
    .await?;
    let held_after_break = held_item_stack(state).cloned();
    if changed
        && let Some(active) = state.pending_break.as_mut()
        && active.held_hotbar_slot == state.selected_hotbar_slot
    {
        active.held_item = held_after_break;
    }
    Ok(())
}

async fn apply_survival_break_exhaustion<W>(
    state: &mut InteractionState,
    writer: &mut W,
    survival_state: &mut SurvivalState,
    xp_state: &mut XpState,
    player_pose: PlayerPose,
    changed: bool,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let mut updated_survival = *survival_state;
    if changed && updated_survival.add_exhaustion(SurvivalState::BLOCK_BREAK_EXHAUSTION) {
        let expected_inventory = state.inventory.clone();
        commit_player_survival_update(
            state,
            writer,
            survival_state,
            xp_state,
            expected_inventory,
            updated_survival,
            xp_state.clone(),
            None,
            true,
            player_pose,
        )
        .await?;
    }
    Ok(())
}
