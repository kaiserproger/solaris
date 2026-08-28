use std::sync::Arc;

use mc_data::{ItemStack, block_facts::FluidKind};
use mc_domain::{Direction, GameMode, InteractionHand};
use mc_protocol::codec::Identifier;
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
        item_model: None,
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::BytesMut;
    use mc_protocol::Packet;
    use mc_protocol::packets::play::{BlockChangedAck, BlockUpdate, ClientboundContainerSetSlot};

    use super::*;
    use crate::play::simulation::simulation_channel;
    use crate::play::tests::{
        interaction_state_for_items_and_blocks, register_survival_test_player,
    };
    use crate::play::{SurvivalState, XpState};

    #[tokio::test]
    async fn committed_bucket_response_orders_block_ack_before_inventory_update() {
        let items = Arc::new(mc_data::items::solaris_required_items());
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let mut state =
            interaction_state_for_items_and_blocks(Arc::clone(&items), Arc::clone(&blocks));
        let water_bucket = items
            .id_of(&Identifier::parse("minecraft:water_bucket").unwrap())
            .unwrap();
        let empty_bucket = items
            .id_of(&Identifier::parse("minecraft:bucket").unwrap())
            .unwrap();
        let stone = blocks
            .block(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .default;
        let water = blocks
            .block(&Identifier::parse("minecraft:water").unwrap())
            .unwrap()
            .default;
        let air = blocks
            .block(&Identifier::parse("minecraft:air").unwrap())
            .unwrap()
            .default;
        let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
        let token = {
            let mut storage = state.world.lock().await;
            let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
            storage
                .insert_generated_chunk(
                    chunk_pos,
                    mc_world::Chunk::empty(
                        chunk_pos,
                        air,
                        Identifier::parse("minecraft:plains").unwrap(),
                    ),
                )
                .unwrap();
            storage.set_block_at(pos, stone).unwrap();
            storage.block_mutation_token(pos).unwrap()
        };
        let held_slot = PlayerInventory::HOTBAR_BASE;
        let held = ItemStack::new(water_bucket, 1);
        state.inventory.slots[held_slot] = held.clone();
        state.selected_hotbar_slot = 0;
        let (session_id, _) = register_survival_test_player(
            &mut state,
            "BucketResponse",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(session_id);
        let sessions = Arc::clone(&state.sessions);
        let world = Arc::clone(&state.world);
        let plan = BucketUsePlan {
            edit: BlockEdit {
                pos,
                new_state: water,
            },
            precondition: super::super::BlockEditPrecondition {
                pos,
                expected_state: stone,
                expected_token: token,
            },
            block_facts: Arc::clone(&state.block_facts),
            inventory: Some(BucketInventoryChange {
                held_slot,
                expected_held: held,
                replacement_item: empty_bucket,
                replacement_max_stack: 16,
            }),
            schedule_fluid_ticks: false,
        };
        let mut writer = Vec::new();
        let mut response = Box::pin(commit_bucket_use_and_respond(
            &mut state,
            &mut writer,
            31,
            plan,
        ));
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            std::future::Future::poll(response.as_mut(), &mut context),
            Poll::Pending
        ));
        assert_eq!(
            owner
                .process_tick_with_world(&sessions, Some(&world), None, 1)
                .processed,
            1
        );
        assert!(response.await.unwrap());

        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(empty_bucket, 1)
        );
        let mut buf = BytesMut::from(writer.as_slice());
        let mut block = mc_protocol::frame::try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("committed block update");
        assert_eq!(block.id, BlockUpdate::ID);
        let update = BlockUpdate::decode(&mut block.body).unwrap();
        assert_eq!(update.state_id, i32::try_from(water.0).unwrap());
        let mut ack = mc_protocol::frame::try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("block changed acknowledgement");
        assert_eq!(ack.id, BlockChangedAck::ID);
        assert_eq!(BlockChangedAck::decode(&mut ack.body).unwrap().sequence, 31);
        let mut inventory = mc_protocol::frame::try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("inventory slot update");
        assert_eq!(inventory.id, ClientboundContainerSetSlot::ID);
        assert_eq!(
            ClientboundContainerSetSlot::decode(&mut inventory.body)
                .unwrap()
                .item_stack
                .item_id,
            empty_bucket
        );
        assert!(buf.is_empty());
    }
}
