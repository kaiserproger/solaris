use std::sync::Arc;

use mc_data::block_facts::FluidKind;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{Direction, GameMode, InteractionHand, ItemStack};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::error::ConnectionError;

use super::block_edit_commit::{
    finalize_visible_block_edit_outcome, send_loaded_block_edit_resyncs,
};
use super::inventory::PlayerInventory;
use super::session::dispatch_visibility_commands;
use super::simulation::{BucketInventoryChange, BucketUsePlan};
use super::{
    BlockEdit, InteractionState, air_state_id, block_state_property, hand_inventory_slot,
    published_block_precondition, write_block_ack, write_inventory_slot_updates,
};

pub(super) async fn handle_bucket_use_on<W>(
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
    let held_slot = hand_inventory_slot(state, hand);
    let held = state.inventory.slots[held_slot].clone();
    if held.is_empty() {
        return Ok(false);
    }

    if let Some(kind) = state.item_to_block.bucket_fluid_kind(held.item_id) {
        let Some(source_state) = state.item_to_block.fluid_source_state(kind) else {
            return Ok(false);
        };
        let Some(empty_bucket) = state.item_to_block.empty_bucket_item() else {
            return Ok(false);
        };
        let (dx, dy, dz) = direction.normal();
        let target = mc_world::BlockPos {
            x: clicked_pos.x + dx,
            y: clicked_pos.y + dy,
            z: clicked_pos.z + dz,
        };
        let air = air_state_id(&state.blocks);
        let Some(precondition) = published_block_precondition(state, target) else {
            return Ok(false);
        };
        if precondition.expected_state != air {
            return Ok(false);
        }

        commit_bucket_use_and_respond(
            state,
            writer,
            sequence,
            BucketUsePlan {
                edit: BlockEdit {
                    pos: target,
                    new_state: source_state,
                },
                precondition,
                block_facts: Arc::clone(&state.block_facts),
                inventory: (game_mode == GameMode::Survival).then_some(BucketInventoryChange {
                    held_slot,
                    expected_held: held,
                    replacement_item: empty_bucket,
                    replacement_max_stack: 16,
                }),
                schedule_fluid_ticks: true,
            },
        )
        .await?;
        return Ok(true);
    }

    if Some(held.item_id) != state.item_to_block.empty_bucket_item() {
        return Ok(false);
    }
    let Some(precondition) = published_block_precondition(state, clicked_pos) else {
        return write_block_ack(writer, state.compression, sequence)
            .await
            .map(|()| true);
    };
    let Some(fluid) = state.block_facts.fluid(precondition.expected_state.0) else {
        return Ok(false);
    };
    if !fluid.source {
        return Ok(false);
    }
    let Some(filled_bucket) = state.item_to_block.filled_bucket_item(fluid.kind) else {
        return Ok(false);
    };
    commit_bucket_use_and_respond(
        state,
        writer,
        sequence,
        BucketUsePlan {
            edit: BlockEdit {
                pos: clicked_pos,
                new_state: air_state_id(&state.blocks),
            },
            precondition,
            block_facts: Arc::clone(&state.block_facts),
            inventory: (game_mode == GameMode::Survival).then_some(BucketInventoryChange {
                held_slot,
                expected_held: held,
                replacement_item: filled_bucket,
                replacement_max_stack: 1,
            }),
            schedule_fluid_ticks: true,
        },
    )
    .await?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CauldronBucketPlan {
    new_state: mc_world::BlockStateId,
    replacement_item: u32,
    replacement_max_stack: i32,
}

pub(super) async fn handle_cauldron_bucket_use_on<W>(
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
    if game_mode != GameMode::Survival {
        return Ok(false);
    }

    let held_slot = hand_inventory_slot(state, hand);
    let held = state.inventory.slots[held_slot].clone();
    if held.is_empty() {
        return Ok(false);
    }

    let Some(precondition) = published_block_precondition(state, clicked_pos) else {
        return write_block_ack(writer, state.compression, sequence)
            .await
            .map(|()| true);
    };
    let Some(plan) = plan_cauldron_bucket_use(state, precondition.expected_state, held.item_id)
    else {
        return Ok(false);
    };

    commit_bucket_use_and_respond(
        state,
        writer,
        sequence,
        BucketUsePlan {
            edit: BlockEdit {
                pos: clicked_pos,
                new_state: plan.new_state,
            },
            precondition,
            block_facts: Arc::clone(&state.block_facts),
            inventory: Some(BucketInventoryChange {
                held_slot,
                expected_held: held,
                replacement_item: plan.replacement_item,
                replacement_max_stack: plan.replacement_max_stack,
            }),
            schedule_fluid_ticks: false,
        },
    )
    .await?;
    Ok(true)
}

async fn commit_bucket_use_and_respond<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    plan: BucketUsePlan,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let edit = plan.edit;
    let inventory_slot = plan.inventory.as_ref().map(|change| change.held_slot);
    let committed = match state.simulation.commit_bucket_use(plan).await {
        Ok(Some(committed)) => committed,
        Ok(None) => {
            send_loaded_block_edit_resyncs(state, writer, &[edit]).await?;
            if let Some(slot) = inventory_slot {
                write_inventory_slot_updates(
                    state,
                    writer,
                    vec![(slot, state.inventory.slots[slot].clone())],
                )
                .await?;
            }
            write_block_ack(writer, state.compression, sequence).await?;
            return Ok(false);
        }
        Err(error) => {
            debug!(?error, "simulation bucket use rejected");
            return Err(ConnectionError::RuntimeUnavailable {
                operation: "committing bucket use",
            });
        }
    };

    if let Some(inventory) = committed.inventory {
        state.inventory = inventory;
    }
    finalize_visible_block_edit_outcome(state, writer, committed.block, false).await?;
    write_block_ack(writer, state.compression, sequence).await?;
    write_inventory_slot_updates(state, writer, committed.changed_slots).await?;
    dispatch_visibility_commands(state.sessions.broadcast_player_animation(state.session_id));
    Ok(true)
}

fn plan_cauldron_bucket_use(
    state: &InteractionState,
    clicked_state: mc_world::BlockStateId,
    held_item: u32,
) -> Option<CauldronBucketPlan> {
    let clicked = state.blocks.by_id(clicked_state)?;
    match clicked.block.id.as_str() {
        "minecraft:cauldron" => {
            if state.item_to_block.bucket_fluid_kind(held_item) != Some(FluidKind::Water) {
                return None;
            }
            Some(CauldronBucketPlan {
                new_state: full_water_cauldron_state(&state.blocks)?,
                replacement_item: state.item_to_block.empty_bucket_item()?,
                replacement_max_stack: 16,
            })
        }
        "minecraft:water_cauldron" => {
            if block_state_property(clicked, "level") != Some("3")
                || Some(held_item) != state.item_to_block.empty_bucket_item()
            {
                return None;
            }
            Some(CauldronBucketPlan {
                new_state: empty_cauldron_state(&state.blocks)?,
                replacement_item: state.item_to_block.filled_bucket_item(FluidKind::Water)?,
                replacement_max_stack: 1,
            })
        }
        _ => None,
    }
}

fn empty_cauldron_state(blocks: &mc_world::BlockRegistry) -> Option<mc_world::BlockStateId> {
    let cauldron = Identifier::parse("minecraft:cauldron").expect("static identifier");
    blocks.block(&cauldron).map(|block| block.default)
}

fn full_water_cauldron_state(blocks: &mc_world::BlockRegistry) -> Option<mc_world::BlockStateId> {
    let water_cauldron = Identifier::parse("minecraft:water_cauldron").expect("static identifier");
    blocks.by_name_and_props(&water_cauldron, &[("level".to_string(), "3".to_string())])
}

pub(in crate::play) fn plan_bucket_replacement(
    inventory: &PlayerInventory,
    held_slot: usize,
    replacement_item: u32,
    replacement_max_stack: i32,
) -> Option<(PlayerInventory, Vec<(usize, ItemStack)>)> {
    if held_slot >= inventory.slots.len() || replacement_max_stack <= 0 {
        return None;
    }

    let mut inventory = inventory.clone();
    let mut changed = Vec::new();
    let held = &mut inventory.slots[held_slot];
    if held.is_empty() {
        return None;
    }

    let replacement = ItemStack {
        item_id: replacement_item,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
    };
    if held.count <= 1 {
        *held = replacement;
        changed.push((held_slot, held.clone()));
        return Some((inventory, changed));
    }

    held.count -= 1;
    changed.push((held_slot, held.clone()));
    let (leftover, mut merged) = inventory.merge_stack(replacement, replacement_max_stack);
    if !leftover.is_empty() {
        return None;
    }
    changed.append(&mut merged);
    Some((inventory, changed))
}
