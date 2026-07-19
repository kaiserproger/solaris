use std::sync::Arc;

use mc_data::items::ItemRegistry;
use mc_entity::Vec3;
use mc_protocol::codec::Identifier;
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundBlockEntityData, ClientboundContainerSetSlot,
    ClientboundOpenSignEditor, Direction, GameMode, InteractionHand, ItemStack,
    ServerboundSignUpdate, ServerboundUseItemOn, pack_block_pos, unpack_block_pos,
};
use mc_world::{BlockRegistry, ChunkPos, SECTION_DIM};
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

use crate::connection::write_packet;
use crate::error::ConnectionError;

#[cfg(test)]
use super::block_edit_commit::apply_opaque_block_entity_to_storage_conditionally;
use super::block_edit_commit::{
    finalize_visible_block_edit_outcome, send_loaded_block_edit_resyncs,
};
use super::block_placement::{
    PlannedBlockPlacement, placed_sign_edit, placement_snapshot_positions, plan_block_placement,
    sign_block_entity_persistent_nbt, sign_block_entity_update_nbt,
};
use super::bucket_interactions::{handle_bucket_use_on, handle_cauldron_bucket_use_on};
use super::campfire_adapter::handle_campfire_use_on;
use super::explosions::{TNT_ENTITY_TYPE_NAME, TntIgnitionPlan};
use super::inventory::PlayerInventory;
use super::persistence::XpState;
use super::plants::{bonemeal_growth_edits, sweet_berry_harvest};
use super::scheduled_blocks::placed_hopper_ticks;
use super::session::{dispatch_visibility_commands, within_block_reach};
use super::simulation::{
    SurvivalBreakDrop, SurvivalBreakHeldItem, SurvivalBreakPlan, SurvivalPlacementHeldItem,
    SurvivalPlacementPlan,
};
use super::survival::{
    SurvivalState, entity_item_stack, item_entity_type_id, max_tool_damage_for_path,
};
use super::{
    BlockEdit, BlockEditPrecondition, HoeTillingPlan, InteractionState, PlayerPose,
    SIGN_BLOCK_ENTITY_TYPE_ID, air_state_id, block_break_loot_seed, clear_shield_use,
    hand_inventory_slot, interact_with_bed, interact_with_toggle_block, open_chest_container,
    open_crafting_table_container, open_enchanting_table_container, open_furnace_container,
    open_stonecutter_container, published_block_precondition,
    reject_unsupported_survival_station_use, schedule_fluid_ticks_for_interaction, splitmix64,
    start_falling_blocks_after_edits, write_block_ack, write_block_resync_then_ack,
    write_inventory_slot_updates,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UseItemOnOutcome {
    Handled,
    NoOp { reason: UseItemOnNoOpReason },
    PlaceBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UseItemOnNoOpReason {
    DeadPlayer,
    UnsupportedGameMode,
    WorldBorderHit,
    OutOfReach,
    EmptyHeldItem,
    ClickedCellUnavailable,
    TargetBlockedOrUnplaceable,
    PlacementPlanRejected,
    ConcurrentMutation,
}

#[derive(Debug, Clone, Copy)]
struct UseItemOnTarget {
    clicked_pos: mc_world::BlockPos,
    coords: (i32, i32, i32),
}

#[derive(Debug, Clone, Copy)]
struct BlockPlacementValidation {
    placed_state: mc_world::BlockStateId,
    clicked_state: mc_world::BlockStateId,
    clicked_token: mc_world::BlockMutationToken,
    target_token: mc_world::BlockMutationToken,
}

#[derive(Debug, Clone, Copy)]
enum UseItemOnHeldResync {
    None,
    BucketOnly,
    HeldItem,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct UseItemOnResyncOptions {
    held_resync: UseItemOnHeldResync,
}

impl UseItemOnResyncOptions {
    const WITH_BUCKET: Self = Self {
        held_resync: UseItemOnHeldResync::BucketOnly,
    };

    pub(super) const WITH_HELD_ITEM: Self = Self {
        held_resync: UseItemOnHeldResync::HeldItem,
    };

    const BLOCKS_ONLY: Self = Self {
        held_resync: UseItemOnHeldResync::None,
    };
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_use_item_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    survival_state: SurvivalState,
    xp_state: &XpState,
    player_pose: PlayerPose,
    respawn_pose: &mut PlayerPose,
    action: ServerboundUseItemOn,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    state.pending_use = None;
    clear_shield_use(state);

    let (clicked_x, clicked_y, clicked_z) = unpack_block_pos(action.position);
    let held = state.inventory.held(state.selected_hotbar_slot);
    debug!(
        sequence = action.sequence,
        clicked_x,
        clicked_y,
        clicked_z,
        direction = ?action.direction,
        selected_slot = state.selected_hotbar_slot,
        held_item = held.item_id,
        held_count = held.count,
        "UseItemOn received"
    );

    let preflight = classify_use_item_on_preflight(game_mode, survival_state, player_pose, &action);
    if let UseItemOnOutcome::NoOp { reason } = preflight
        && !matches!(
            reason,
            UseItemOnNoOpReason::OutOfReach | UseItemOnNoOpReason::WorldBorderHit
        )
    {
        return ack_use_item_on_noop(writer, state.compression, action.sequence, reason).await;
    }

    let (cx, cy, cz) = unpack_block_pos(action.position);
    let clicked_pos = mc_world::BlockPos {
        x: cx,
        y: cy,
        z: cz,
    };
    let target = UseItemOnTarget {
        clicked_pos,
        coords: (cx, cy, cz),
    };

    if let UseItemOnOutcome::NoOp {
        reason: UseItemOnNoOpReason::OutOfReach,
    } = preflight
    {
        let (dx, dy, dz) = action.direction.normal();
        return reject_use_item_on_with_resync(
            state,
            writer,
            action.sequence,
            target.clicked_pos,
            mc_world::BlockPos {
                x: cx + dx,
                y: cy + dy,
                z: cz + dz,
            },
            UseItemOnNoOpReason::OutOfReach,
            UseItemOnResyncOptions::BLOCKS_ONLY,
        )
        .await;
    }
    if let UseItemOnOutcome::NoOp {
        reason: UseItemOnNoOpReason::WorldBorderHit,
    } = preflight
    {
        let (dx, dy, dz) = action.direction.normal();
        return reject_use_item_on_with_resync(
            state,
            writer,
            action.sequence,
            target.clicked_pos,
            mc_world::BlockPos {
                x: cx + dx,
                y: cy + dy,
                z: cz + dz,
            },
            UseItemOnNoOpReason::WorldBorderHit,
            UseItemOnResyncOptions::BLOCKS_ONLY,
        )
        .await;
    }

    match handle_use_item_on_interactions(
        state,
        writer,
        game_mode,
        xp_state,
        player_pose,
        respawn_pose,
        &action,
        target,
    )
    .await?
    {
        UseItemOnOutcome::Handled => Ok(()),
        UseItemOnOutcome::NoOp { reason } => {
            ack_use_item_on_noop(writer, state.compression, action.sequence, reason).await
        }
        UseItemOnOutcome::PlaceBlock => {
            handle_block_item_placement(
                state,
                writer,
                game_mode,
                player_pose,
                target.clicked_pos,
                &action,
                target.coords,
            )
            .await
        }
    }
}

pub(super) fn classify_use_item_on_preflight(
    game_mode: GameMode,
    survival_state: SurvivalState,
    player_pose: PlayerPose,
    action: &ServerboundUseItemOn,
) -> UseItemOnOutcome {
    if game_mode == GameMode::Survival && survival_state.is_dead() {
        debug!(
            sequence = action.sequence,
            "survival block placement ignored for dead player"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::DeadPlayer,
        };
    }

    if !matches!(game_mode, GameMode::Creative | GameMode::Survival) {
        debug!(
            mode = ?game_mode,
            sequence = action.sequence,
            "block placement denied outside creative/survival"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::UnsupportedGameMode,
        };
    }

    if action.world_border_hit {
        debug!(
            sequence = action.sequence,
            "block placement ignored: client reported world-border hit"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::WorldBorderHit,
        };
    }

    if !within_block_reach(player_pose, action.position, game_mode) {
        debug!(
            sequence = action.sequence,
            "block placement ignored: target out of reach"
        );
        return UseItemOnOutcome::NoOp {
            reason: UseItemOnNoOpReason::OutOfReach,
        };
    }

    UseItemOnOutcome::PlaceBlock
}

#[allow(clippy::too_many_arguments)]
async fn handle_use_item_on_interactions<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    xp_state: &XpState,
    player_pose: PlayerPose,
    respawn_pose: &mut PlayerPose,
    action: &ServerboundUseItemOn,
    target: UseItemOnTarget,
) -> Result<UseItemOnOutcome, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (cx, cy, cz) = target.coords;
    if !player_pose.shifting {
        if open_crafting_table_container(state, writer, player_pose, action.sequence, cx, cy, cz)
            .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_enchanting_table_container(
            state,
            writer,
            xp_state,
            player_pose,
            action.sequence,
            mc_world::BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
        )
        .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_stonecutter_container(
            state,
            writer,
            player_pose,
            action.sequence,
            mc_world::BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
        )
        .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_furnace_container(state, writer, player_pose, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
        if open_chest_container(state, writer, player_pose, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
        if handle_cauldron_bucket_use_on(
            state,
            writer,
            game_mode,
            action.sequence,
            target.clicked_pos,
            action.hand,
        )
        .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if reject_unsupported_survival_station_use(state, writer, action.sequence, cx, cy, cz)
            .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if interact_with_bed(
            state,
            writer,
            game_mode,
            action.sequence,
            mc_world::BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
            respawn_pose,
        )
        .await?
        {
            return Ok(UseItemOnOutcome::Handled);
        }
        if interact_with_toggle_block(state, writer, action.sequence, cx, cy, cz).await? {
            return Ok(UseItemOnOutcome::Handled);
        }
    }

    if player_pose.shifting
        && handle_cauldron_bucket_use_on(
            state,
            writer,
            game_mode,
            action.sequence,
            target.clicked_pos,
            action.hand,
        )
        .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_campfire_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.hand,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_tnt_ignition(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.hand,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_bucket_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.direction,
        action.hand,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_hoe_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        target.clicked_pos,
        action.direction,
        action.hand,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }
    if handle_plant_use_on(
        state,
        writer,
        action.sequence,
        target.clicked_pos,
        player_pose,
    )
    .await?
    {
        return Ok(UseItemOnOutcome::Handled);
    }

    Ok(UseItemOnOutcome::PlaceBlock)
}

async fn handle_tnt_ignition<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    hand: InteractionHand,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if !matches!(game_mode, GameMode::Creative | GameMode::Survival) {
        return Ok(false);
    }
    let held_slot = hand_inventory_slot(state, hand);
    let held = state.inventory.slots[held_slot].clone();
    let Some(held_name) = state.items.name_of(held.item_id) else {
        return Ok(false);
    };
    if held.is_empty() || held_name.as_str() != "minecraft:flint_and_steel" {
        return Ok(false);
    }

    let snapshot = loaded_block_snapshot(state, &[clicked_pos]);
    let Some(tnt_state) = snapshot.get_cached_block(clicked_pos) else {
        return Ok(false);
    };
    if state
        .blocks
        .by_id(tnt_state)
        .is_none_or(|state| state.block.id.as_str() != TNT_ENTITY_TYPE_NAME)
    {
        return Ok(false);
    }
    let Some(air) = default_block_state(&state.blocks, "minecraft:air") else {
        return Ok(false);
    };
    let tnt_entity = Identifier::parse(TNT_ENTITY_TYPE_NAME).expect("static TNT entity id");
    let Some(tnt_entity_type_id) = state
        .entity_types
        .id_of(&tnt_entity)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return Ok(false);
    };
    let max_damage = state
        .item_facts
        .get(held_name)
        .and_then(|facts| facts.max_damage)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(64);
    let Some(expected_token) = snapshot.block_mutation_token(clicked_pos) else {
        return Ok(false);
    };
    let committed = match state
        .simulation
        .commit_tnt_ignition(TntIgnitionPlan {
            tnt: BlockEditPrecondition {
                pos: clicked_pos,
                expected_state: tnt_state,
                expected_token,
            },
            air,
            game_mode,
            held_slot,
            expected_held: held,
            flint_and_steel_max_damage: max_damage,
            tnt_entity_type_id,
        })
        .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) | Err(_) => {
            write_block_resync_then_ack(
                state,
                writer,
                pack_block_pos(clicked_pos.x, clicked_pos.y, clicked_pos.z),
                sequence,
            )
            .await?;
            return Ok(true);
        }
    };

    state.inventory = committed.inventory;
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    let outcome =
        finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    if !outcome.applied.is_empty() {
        write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    }
    Ok(true)
}

async fn handle_hoe_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    direction: Direction,
    hand: InteractionHand,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if hand != InteractionHand::MainHand || direction != Direction::Up {
        return Ok(false);
    }
    let held = state.inventory.slots[hand_inventory_slot(state, hand)].clone();
    if held.is_empty() || !item_is_hoe(&state.items, held.item_id) {
        return Ok(false);
    }
    let Some(farmland) = default_block_state(&state.blocks, "minecraft:farmland") else {
        return Ok(false);
    };
    let Some(plan) = plan_hoe_tilling(state, clicked_pos, farmland) else {
        return Ok(false);
    };
    let max_damage = if game_mode == GameMode::Survival {
        state
            .items
            .name_of(held.item_id)
            .and_then(|item| max_tool_damage_for_path(item.path()))
    } else {
        None
    };
    let committed = match state
        .simulation
        .commit_survival_break(SurvivalBreakPlan {
            edits: plan.edits,
            preconditions: plan.preconditions,
            blocks: Arc::clone(&state.blocks),
            block_facts: Arc::clone(&state.block_facts),
            falling_block_entity_type_id: None,
            held: SurvivalBreakHeldItem {
                hotbar_slot: state.selected_hotbar_slot,
                expected: held,
                max_damage,
            },
            drops: Vec::new(),
        })
        .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            write_block_resync_then_ack(
                state,
                writer,
                pack_block_pos(clicked_pos.x, clicked_pos.y, clicked_pos.z),
                sequence,
            )
            .await?;
            return Ok(true);
        }
        Err(error) => {
            debug!(?error, sequence, "simulation hoe tilling rejected");
            write_block_resync_then_ack(
                state,
                writer,
                pack_block_pos(clicked_pos.x, clicked_pos.y, clicked_pos.z),
                sequence,
            )
            .await?;
            return Ok(true);
        }
    };

    state.inventory = committed.inventory;
    let changed_slots = committed.changed_slots;
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    let outcome =
        finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    if !outcome.applied.is_empty() && !changed_slots.is_empty() {
        write_inventory_slot_updates(state, writer, changed_slots).await?;
    }
    Ok(true)
}

pub(super) fn plan_hoe_tilling(
    state: &InteractionState,
    clicked_pos: mc_world::BlockPos,
    farmland: mc_world::BlockStateId,
) -> Option<HoeTillingPlan> {
    let above_pos = mc_world::BlockPos {
        y: clicked_pos.y.checked_add(1)?,
        ..clicked_pos
    };
    let snapshot = loaded_block_snapshot(state, &[clicked_pos, above_pos]);
    let clicked = snapshot.get_cached_block(clicked_pos)?;
    let above = snapshot.get_cached_block(above_pos)?;
    if above != air_state_id(&state.blocks)
        || !state
            .blocks
            .by_id(clicked)
            .is_some_and(|state| is_tillable_block_path(state.block.id.path()))
    {
        return None;
    }
    Some(HoeTillingPlan {
        edits: vec![BlockEdit {
            pos: clicked_pos,
            new_state: farmland,
        }],
        preconditions: vec![
            BlockEditPrecondition {
                pos: clicked_pos,
                expected_state: clicked,
                expected_token: snapshot.block_mutation_token(clicked_pos)?,
            },
            BlockEditPrecondition {
                pos: above_pos,
                expected_state: above,
                expected_token: snapshot.block_mutation_token(above_pos)?,
            },
        ],
    })
}

fn item_is_hoe(items: &ItemRegistry, item_id: u32) -> bool {
    items
        .name_of(item_id)
        .is_some_and(|item| item.path().ends_with("_hoe"))
}

fn default_block_state(blocks: &BlockRegistry, id: &str) -> Option<mc_world::BlockStateId> {
    blocks
        .block(&mc_data::Identifier::parse(id).expect("static identifier"))
        .map(|block| block.default)
}

fn is_tillable_block_path(path: &str) -> bool {
    matches!(path, "dirt" | "grass_block" | "dirt_path")
}

fn loaded_block_snapshot(
    state: &InteractionState,
    positions: &[mc_world::BlockPos],
) -> mc_world::WorldReadSnapshot {
    let mut chunks = Vec::with_capacity(positions.len());
    for position in positions {
        let chunk = ChunkPos {
            x: position.x.div_euclid(SECTION_DIM as i32),
            z: position.z.div_euclid(SECTION_DIM as i32),
        };
        if !chunks.contains(&chunk) {
            chunks.push(chunk);
        }
    }
    state.world_read.snapshot_chunks(&chunks)
}

pub(super) fn plan_loaded_bonemeal_growth(
    state: &InteractionState,
    clicked_pos: mc_world::BlockPos,
    sequence: i32,
) -> Option<(Vec<BlockEdit>, Vec<BlockEditPrecondition>)> {
    let west = clicked_pos.x.checked_sub(2)?;
    let east = clicked_pos.x.checked_add(2)?;
    let north = clicked_pos.z.checked_sub(2)?;
    let south = clicked_pos.z.checked_add(2)?;
    clicked_pos.y.checked_add(7)?;
    let snapshot = loaded_block_snapshot(
        state,
        &[
            clicked_pos,
            mc_world::BlockPos {
                x: west,
                z: north,
                ..clicked_pos
            },
            mc_world::BlockPos {
                x: west,
                z: south,
                ..clicked_pos
            },
            mc_world::BlockPos {
                x: east,
                z: north,
                ..clicked_pos
            },
            mc_world::BlockPos {
                x: east,
                z: south,
                ..clicked_pos
            },
        ],
    );
    let current = snapshot.get_cached_block(clicked_pos)?;
    let current_token = snapshot.block_mutation_token(clicked_pos)?;
    let tree_seed = splitmix64(
        block_break_loot_seed(clicked_pos, current, current_token)
            ^ (sequence as i64 as u64).rotate_left(23),
    );
    let edits = bonemeal_growth_edits(&state.blocks, &snapshot, clicked_pos, current, tree_seed)?;
    let preconditions = edits
        .iter()
        .map(|edit| {
            Some(BlockEditPrecondition {
                pos: edit.pos,
                expected_state: snapshot.get_cached_block(edit.pos)?,
                expected_token: snapshot.block_mutation_token(edit.pos)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((edits, preconditions))
}

/// M6.f/M23 follow-up: resolve the placed block via the player's currently-held
/// hotbar slot through the item→block table. Drops the placement silently (still
/// acking) if the held stack is empty, if the held item has no block mapping
/// (e.g. food, tool), or if the target cell is non-air. On success decrements
/// the held stack and emits `ContainerSetSlot` so the client sees the new count.
pub(super) async fn handle_block_item_placement<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    player_pose: PlayerPose,
    clicked_pos: mc_world::BlockPos,
    action: &ServerboundUseItemOn,
    (cx, cy, cz): (i32, i32, i32),
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let sequence = action.sequence;

    let (dx, dy, dz) = action.direction.normal();
    let (tx, ty, tz) = (cx + dx, cy + dy, cz + dz);

    let air = air_state_id(&state.blocks);
    let target_pos = mc_world::BlockPos {
        x: tx,
        y: ty,
        z: tz,
    };

    // M6.f: resolve the placed block from the held item.
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot).clone();
    if held.is_empty() {
        debug!(
            sequence = action.sequence,
            held_item = held.item_id,
            held_count = held.count,
            "UseItemOn: held item is empty or not placeable; skipping"
        );
        return reject_use_item_on_with_resync(
            state,
            writer,
            sequence,
            clicked_pos,
            mc_world::BlockPos {
                x: tx,
                y: ty,
                z: tz,
            },
            UseItemOnNoOpReason::EmptyHeldItem,
            UseItemOnResyncOptions::WITH_BUCKET,
        )
        .await;
    };

    if handle_bonemeal_use_on(
        state,
        writer,
        game_mode,
        action.sequence,
        clicked_pos,
        held_slot,
        held.item_id,
    )
    .await?
    {
        return Ok(());
    }

    // Validate: target cell must currently be air. Crop items also
    // inspect the clicked block because seeds place the crop above
    // their supporting soil instead of mapping item name to block name.
    let placement_result = 'placement: {
        let snapshot = loaded_block_snapshot(state, &[clicked_pos, target_pos]);
        let Some(clicked) = snapshot.get_cached_block(clicked_pos) else {
            debug!(
                x = cx,
                y = cy,
                z = cz,
                "UseItemOn clicked cell absent; skipping placement"
            );
            break 'placement Err(UseItemOnNoOpReason::ClickedCellUnavailable);
        };
        let Some(clicked_token) = snapshot.block_mutation_token(clicked_pos) else {
            break 'placement Err(UseItemOnNoOpReason::ClickedCellUnavailable);
        };
        let target_state = snapshot.get_cached_block(target_pos);
        let Some(target_token) = snapshot.block_mutation_token(target_pos) else {
            break 'placement Err(UseItemOnNoOpReason::ClickedCellUnavailable);
        };
        if target_state != Some(air) {
            Ok(None)
        } else {
            Ok(state
                .item_to_block
                .resolve_for_use_on(
                    &state.items,
                    held.item_id,
                    clicked,
                    action.direction,
                    &state.blocks,
                )
                .map(|placed_state| BlockPlacementValidation {
                    placed_state,
                    clicked_state: clicked,
                    clicked_token,
                    target_token,
                }))
        }
    };
    let placement = match placement_result {
        Ok(Some(placement)) => placement,
        Ok(None) => {
            debug!(
                x = tx,
                y = ty,
                z = tz,
                held_item = held.item_id,
                "UseItemOn target invalid or held item not placeable; skipping placement"
            );
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                mc_world::BlockPos {
                    x: tx,
                    y: ty,
                    z: tz,
                },
                UseItemOnNoOpReason::TargetBlockedOrUnplaceable,
                UseItemOnResyncOptions::WITH_BUCKET,
            )
            .await;
        }
        Err(reason) => {
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                mc_world::BlockPos {
                    x: tx,
                    y: ty,
                    z: tz,
                },
                reason,
                UseItemOnResyncOptions::WITH_BUCKET,
            )
            .await;
        }
    };
    let placed_state = placement.placed_state;

    let Some(plan) = plan_place_block_edits(
        state,
        mc_world::BlockPos {
            x: tx,
            y: ty,
            z: tz,
        },
        placed_state,
        player_pose,
        action.direction,
        cursor_y_relative_to_target(clicked_pos.y, ty, action.cursor_y),
    )
    .await
    else {
        return reject_use_item_on_with_resync(
            state,
            writer,
            sequence,
            clicked_pos,
            mc_world::BlockPos {
                x: tx,
                y: ty,
                z: tz,
            },
            UseItemOnNoOpReason::PlacementPlanRejected,
            UseItemOnResyncOptions::WITH_HELD_ITEM,
        )
        .await;
    };
    let PlannedBlockPlacement {
        edits,
        additional_preconditions,
    } = plan;
    let scheduled_block_ticks =
        placed_hopper_ticks(&state.blocks, &edits, state.sessions.simulation_tick());
    let mut preconditions = vec![
        BlockEditPrecondition {
            pos: target_pos,
            expected_state: air,
            expected_token: placement.target_token,
        },
        BlockEditPrecondition {
            pos: clicked_pos,
            expected_state: placement.clicked_state,
            expected_token: placement.clicked_token,
        },
    ];
    preconditions.extend(additional_preconditions);
    let committed = match state
        .simulation
        .commit_survival_placement(SurvivalPlacementPlan {
            edits,
            preconditions,
            scheduled_block_ticks,
            block_facts: Arc::clone(&state.block_facts),
            held: SurvivalPlacementHeldItem {
                hotbar_slot: held_slot,
                expected: held,
            },
            expected_game_mode: game_mode,
        })
        .await
    {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                target_pos,
                UseItemOnNoOpReason::ConcurrentMutation,
                UseItemOnResyncOptions::WITH_HELD_ITEM,
            )
            .await;
        }
        Err(error) => {
            debug!(?error, sequence, "simulation survival placement rejected");
            return reject_use_item_on_with_resync(
                state,
                writer,
                sequence,
                clicked_pos,
                target_pos,
                UseItemOnNoOpReason::ConcurrentMutation,
                UseItemOnResyncOptions::WITH_HELD_ITEM,
            )
            .await;
        }
    };
    state.inventory = committed.inventory;
    let changed_slots = committed.changed_slots;
    let outcome =
        finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
    write_packet(writer, &BlockChangedAck { sequence }, state.compression).await?;
    if outcome.applied.is_empty() {
        return Ok(());
    }
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    write_inventory_slot_updates(state, writer, changed_slots).await?;
    if let Some(pending) = placed_sign_edit(&state.blocks, &outcome) {
        state.pending_sign_edit = Some(pending);
        write_packet(
            writer,
            &ClientboundOpenSignEditor {
                position: pack_block_pos(
                    pending.position.x,
                    pending.position.y,
                    pending.position.z,
                ),
                is_front_text: pending.is_front_text,
            },
            state.compression,
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn reject_use_item_on_with_resync<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    target_pos: mc_world::BlockPos,
    reason: UseItemOnNoOpReason,
    options: UseItemOnResyncOptions,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let held_slot_resync = match options.held_resync {
        UseItemOnHeldResync::None => None,
        UseItemOnHeldResync::BucketOnly => bucket_held_slot_resync(state),
        UseItemOnHeldResync::HeldItem => held_item_slot_resync(state),
    };
    let updates = [clicked_pos, target_pos]
        .into_iter()
        .filter_map(|pos| {
            state
                .world_read
                .get_cached_block(pos)
                .map(|state_id| BlockUpdate {
                    position: pack_block_pos(pos.x, pos.y, pos.z),
                    state_id: state_id.0 as i32,
                })
        })
        .collect::<Vec<_>>();
    for update in updates {
        write_packet(writer, &update, state.compression).await?;
    }
    if let Some((slot, item_stack)) = held_slot_resync {
        state.inventory_state_id = state.inventory_state_id.wrapping_add(1);
        write_packet(
            writer,
            &ClientboundContainerSetSlot {
                container_id: 0,
                state_id: state.inventory_state_id,
                slot,
                item_stack,
            },
            state.compression,
        )
        .await?;
    }
    ack_use_item_on_noop(writer, state.compression, sequence, reason).await
}

fn bucket_held_slot_resync(state: &InteractionState) -> Option<(i16, ItemStack)> {
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot);
    if held.is_empty() {
        return None;
    }
    let is_bucket = state
        .item_to_block
        .bucket_fluid_kind(held.item_id)
        .is_some()
        || Some(held.item_id) == state.item_to_block.empty_bucket_item();
    is_bucket.then(|| {
        (
            (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            held.clone(),
        )
    })
}

fn held_item_slot_resync(state: &InteractionState) -> Option<(i16, ItemStack)> {
    let held_slot = state.selected_hotbar_slot;
    let held = state.inventory.held(held_slot);
    (!held.is_empty()).then(|| {
        (
            (PlayerInventory::HOTBAR_BASE + held_slot as usize) as i16,
            held.clone(),
        )
    })
}

async fn ack_use_item_on_noop<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
    reason: UseItemOnNoOpReason,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(sequence, reason = ?reason, "UseItemOn noop acknowledged");
    write_block_ack(writer, compression, sequence).await
}

pub(super) async fn ack_use_item_noop<W>(
    writer: &mut W,
    compression: Compression,
    sequence: i32,
    reason: &'static str,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    debug!(sequence, reason, "UseItem noop acknowledged");
    write_block_ack(writer, compression, sequence).await
}

async fn handle_bonemeal_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    sequence: i32,
    clicked_pos: mc_world::BlockPos,
    held_slot: u8,
    held_item_id: u32,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let bone_meal = Identifier::parse("minecraft:bone_meal").expect("static identifier");
    if state.items.id_of(&bone_meal) != Some(held_item_id) {
        return Ok(false);
    }

    let planned = plan_loaded_bonemeal_growth(state, clicked_pos, sequence);

    let Some((edits, preconditions)) = planned else {
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(true);
    };
    let slot = PlayerInventory::HOTBAR_BASE + held_slot as usize;
    let expected_held = state.inventory.slots[slot].clone();
    let committed = state
        .simulation
        .commit_survival_placement(SurvivalPlacementPlan {
            edits: edits.clone(),
            preconditions,
            scheduled_block_ticks: Vec::new(),
            block_facts: Arc::clone(&state.block_facts),
            held: SurvivalPlacementHeldItem {
                hotbar_slot: held_slot,
                expected: expected_held,
            },
            expected_game_mode: game_mode,
        })
        .await;
    let committed = match committed {
        Ok(committed) => committed,
        Err(error) => {
            debug!(?error, sequence, "simulation bonemeal transaction rejected");
            None
        }
    };
    let Some(committed) = committed else {
        send_loaded_block_edit_resyncs(state, writer, &edits).await?;
        write_inventory_slot_updates(
            state,
            writer,
            vec![(slot, state.inventory.slots[slot].clone())],
        )
        .await?;
        write_block_ack(writer, state.compression, sequence).await?;
        return Ok(true);
    };
    state.inventory = committed.inventory;
    let outcome =
        finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
    write_block_ack(writer, state.compression, sequence).await?;
    if !outcome.applied.is_empty() {
        write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    }
    Ok(true)
}

async fn handle_plant_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    position: mc_world::BlockPos,
    _player_pose: PlayerPose,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some((edit, drop, precondition)) = plan_loaded_plant_harvest(state, position) else {
        return Ok(false);
    };
    let Some(entity_type_id) = item_entity_type_id(&state.entity_types) else {
        warn!("plant harvest rejected: item entity type unavailable");
        write_block_resync_then_ack(
            state,
            writer,
            pack_block_pos(position.x, position.y, position.z),
            sequence,
        )
        .await?;
        return Ok(true);
    };
    let outcome = match state
        .simulation
        .commit_block_drops(
            vec![edit],
            vec![precondition],
            vec![SurvivalBreakDrop {
                entity_type_id,
                position: Vec3::new(
                    f64::from(position.x) + 0.5,
                    f64::from(position.y) + 0.5,
                    f64::from(position.z) + 0.5,
                ),
                stack: entity_item_stack(drop),
            }],
        )
        .await
    {
        Ok(Some(outcome)) => outcome,
        Ok(None) => {
            write_block_resync_then_ack(
                state,
                writer,
                pack_block_pos(position.x, position.y, position.z),
                sequence,
            )
            .await?;
            return Ok(true);
        }
        Err(error) => {
            debug!(?error, "simulation plant harvest rejected");
            write_block_resync_then_ack(
                state,
                writer,
                pack_block_pos(position.x, position.y, position.z),
                sequence,
            )
            .await?;
            return Ok(true);
        }
    };
    let outcome = finalize_visible_block_edit_outcome(state, writer, outcome, false).await?;
    write_block_ack(writer, state.compression, sequence).await?;
    if !outcome.applied.is_empty() {
        schedule_fluid_ticks_for_interaction(state, &outcome.applied).await;
        start_falling_blocks_after_edits(state, writer, &outcome.applied).await?;
    }
    Ok(true)
}

pub(super) fn plan_loaded_plant_harvest(
    state: &InteractionState,
    position: mc_world::BlockPos,
) -> Option<(BlockEdit, ItemStack, BlockEditPrecondition)> {
    let precondition = published_block_precondition(state, position)?;
    let (edit, drop) = sweet_berry_harvest(
        &state.blocks,
        &state.items,
        position,
        precondition.expected_state,
    )?;
    Some((edit, drop, precondition))
}

#[cfg(test)]
pub(super) fn consume_bonemeal_after_growth(
    inventory: &mut PlayerInventory,
    held_slot: u8,
    grew: bool,
) -> Option<ItemStack> {
    if !grew {
        return None;
    }
    let held = inventory.held_mut(held_slot);
    held.count = held.count.saturating_sub(1);
    if held.count <= 0 {
        *held = ItemStack::EMPTY;
    }
    Some(inventory.held(held_slot).clone())
}

pub(super) async fn plan_place_block_edits(
    state: &InteractionState,
    pos: mc_world::BlockPos,
    placed_state: mc_world::BlockStateId,
    player_pose: PlayerPose,
    direction: Direction,
    target_relative_hit_y: f32,
) -> Option<PlannedBlockPlacement> {
    let snapshot_positions = placement_snapshot_positions(&state.blocks, placed_state, pos)?;
    let snapshot =
        (!snapshot_positions.is_empty()).then(|| loaded_block_snapshot(state, &snapshot_positions));
    plan_block_placement(
        &state.blocks,
        placed_state,
        snapshot.as_ref(),
        pos,
        player_pose,
        direction,
        target_relative_hit_y,
        air_state_id(&state.blocks),
    )
}

pub(super) fn cursor_y_relative_to_target(clicked_y: i32, target_y: i32, cursor_y: f32) -> f32 {
    cursor_y + (clicked_y - target_y) as f32
}

pub(super) async fn handle_sign_update<W>(
    state: &mut InteractionState,
    writer: &mut W,
    packet: ServerboundSignUpdate,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let (x, y, z) = unpack_block_pos(packet.position);
    let pos = mc_world::BlockPos { x, y, z };
    let Some(pending) = state.pending_sign_edit else {
        debug!(
            ?pos,
            "sign update ignored without matching open editor state"
        );
        return Ok(());
    };
    if pending.position != pos || pending.is_front_text != packet.is_front_text {
        debug!(
            ?pos,
            front = packet.is_front_text,
            "sign update ignored without matching open editor state"
        );
        return Ok(());
    }

    let update_tag = sign_block_entity_update_nbt(&packet.lines, packet.is_front_text);
    let persistent_tag = sign_block_entity_persistent_nbt(pos, &update_tag);
    let mut persistent_bytes = Vec::new();
    mc_nbt::write_network(&mut persistent_bytes, &persistent_tag)
        .map_err(mc_protocol::CodecError::from)?;
    if !commit_sign_block_entity(state, pos, pending.state, pending.token, persistent_bytes).await {
        state.pending_sign_edit = None;
        send_loaded_block_edit_resyncs(
            state,
            writer,
            &[BlockEdit {
                pos,
                new_state: pending.state,
            }],
        )
        .await?;
        return Ok(());
    }

    write_packet(
        writer,
        &ClientboundBlockEntityData {
            position: packet.position,
            block_entity_type: SIGN_BLOCK_ENTITY_TYPE_ID,
            nbt: update_tag.clone(),
        },
        state.compression,
    )
    .await?;
    dispatch_visibility_commands(state.sessions.block_entity_data_dispatches(
        pos,
        Some(state.session_id),
        SIGN_BLOCK_ENTITY_TYPE_ID,
        update_tag,
    ));
    state.pending_sign_edit = None;
    debug!(?pos, lines = ?packet.lines, front = packet.is_front_text, "sign update accepted");
    Ok(())
}

async fn commit_sign_block_entity(
    state: &InteractionState,
    position: mc_world::BlockPos,
    expected_state: mc_world::BlockStateId,
    expected_token: mc_world::BlockMutationToken,
    bytes: Vec<u8>,
) -> bool {
    #[cfg(test)]
    {
        let mut storage = state.world.lock().await;
        match apply_opaque_block_entity_to_storage_conditionally(
            &mut storage,
            position,
            expected_state,
            expected_token,
            bytes,
        ) {
            Ok(committed) => committed,
            Err(err) => {
                warn!(error = %err, ?position, "sign block entity save failed");
                false
            }
        }
    }

    #[cfg(not(test))]
    {
        match state
            .simulation
            .commit_opaque_block_entity(position, expected_state, expected_token, bytes)
            .await
        {
            Ok(committed) => committed,
            Err(error) => {
                debug!(?error, ?position, "simulation sign commit rejected");
                false
            }
        }
    }
}
