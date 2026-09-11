use std::sync::Arc;

use mc_data::{ItemStack, block_facts::FluidKind, collision_shapes::vanilla_collision_shapes};
use mc_domain::{Direction, GameMode, InteractionHand};
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{ServerboundUseItem, pack_block_pos};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::error::ConnectionError;

use super::block_edit_commit::{
    finalize_visible_block_edit_outcome, send_loaded_block_edit_resyncs,
};
use super::inventory::PlayerInventory;
use super::session::{dispatch_visibility_commands, within_block_reach};
use super::simulation::{BucketInventoryChange, BucketUsePlan};
use super::use_item_on_adapter::{
    UseItemOnNoOpReason, UseItemOnResyncOptions, reject_use_item_on_with_resync,
};
use super::{
    BlockEdit, InteractionState, PlayerPose, air_state_id, block_state_property,
    hand_inventory_slot, published_block_precondition, write_block_ack,
    write_inventory_slot_updates,
};

/// Vanilla bucket raycast distance: `BucketItem` traces the player's look ray
/// up to the block-interaction range (4.5 blocks in survival).
const BUCKET_FLUID_RAYCAST_RANGE: f64 = 4.5;

/// Authoritative empty-bucket pickup for the `ServerboundUseItem` (air-use)
/// packet: raycast from the validated player eye along the validated pose
/// look vector in `SOURCE_ONLY` fluid mode, then reuse the shared
/// [`BucketUsePlan`] + simulation commit transaction. Returns `true` when the
/// held stack is an empty bucket (the sequence is always acked here).
pub(super) async fn handle_bucket_use<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    player_pose: PlayerPose,
    action: ServerboundUseItem,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let held_slot = hand_inventory_slot(state, action.hand);
    let held = state.inventory.slots[held_slot].clone();
    if held.is_empty() || Some(held.item_id) != state.item_to_block.empty_bucket_item() {
        return Ok(false);
    }

    let Some((eye, direction)) = bucket_eye_and_look(player_pose) else {
        return write_block_ack(writer, state.compression, action.sequence)
            .await
            .map(|()| true);
    };
    let Some(hit) = raycast_fluid_source(state, eye, direction) else {
        return write_block_ack(writer, state.compression, action.sequence)
            .await
            .map(|()| true);
    };
    let Some(precondition) = published_block_precondition(state, hit) else {
        return write_block_ack(writer, state.compression, action.sequence)
            .await
            .map(|()| true);
    };
    // The raycast and the precondition read the same published view, but the
    // commit only checks the token: re-validate fluid-ness here so a stale or
    // non-fluid hit can never delete an unrelated block.
    let Some(fluid) = state.block_facts.fluid(precondition.expected_state.0) else {
        return resync_bucket_use_miss(state, writer, action.sequence, held_slot, &held, hit).await;
    };
    if !fluid.source {
        return resync_bucket_use_miss(state, writer, action.sequence, held_slot, &held, hit).await;
    }
    let Some(filled_bucket) = state.item_to_block.filled_bucket_item(fluid.kind) else {
        return write_block_ack(writer, state.compression, action.sequence)
            .await
            .map(|()| true);
    };
    commit_bucket_use_and_respond(
        state,
        writer,
        action.sequence,
        BucketUsePlan {
            edit: BlockEdit {
                pos: hit,
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

/// Vanilla `UseItemOn` bucket ordering: an empty bucket aimed at a source
/// fluid picks it up first; a filled bucket places into a replaceable
/// vegetation cell or the adjacent cell. Every bucket-held outcome is
/// terminal (`true`): successes commit through the shared transaction, while
/// failures resync the authoritative blocks + held bucket and ack, so the
/// client never falls through to block placement with a stale prediction.
#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_bucket_use_on<W>(
    state: &mut InteractionState,
    writer: &mut W,
    game_mode: GameMode,
    player_pose: PlayerPose,
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

    if Some(held.item_id) == state.item_to_block.empty_bucket_item() {
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
        return Ok(true);
    }

    let Some(kind) = state.item_to_block.bucket_fluid_kind(held.item_id) else {
        return Ok(false);
    };
    let Some(source_state) = state.item_to_block.fluid_source_state(kind) else {
        return Ok(false);
    };
    let Some(empty_bucket) = state.item_to_block.empty_bucket_item() else {
        return Ok(false);
    };

    let (dx, dy, dz) = direction.normal();
    let adjacent = mc_world::BlockPos {
        x: clicked_pos.x + dx,
        y: clicked_pos.y + dy,
        z: clicked_pos.z + dz,
    };
    let Some(clicked_precondition) = published_block_precondition(state, clicked_pos) else {
        return reject_use_item_on_with_resync(
            state,
            writer,
            hand,
            sequence,
            clicked_pos,
            adjacent,
            UseItemOnNoOpReason::ClickedCellUnavailable,
            UseItemOnResyncOptions::WITH_BUCKET,
        )
        .await
        .map(|()| true);
    };
    // Vanilla replaces a replaceable plant in the clicked cell instead of
    // offsetting to the adjacent cell; the literal-air adjacent fast path
    // below only runs for solid (non-replaceable) clicked blocks.
    let target_is_clicked = bucket_replaceable_plant(state, clicked_precondition.expected_state);
    let target = if target_is_clicked {
        clicked_pos
    } else {
        adjacent
    };
    if !within_block_reach(
        player_pose,
        pack_block_pos(target.x, target.y, target.z),
        game_mode,
    ) {
        return reject_use_item_on_with_resync(
            state,
            writer,
            hand,
            sequence,
            clicked_pos,
            adjacent,
            UseItemOnNoOpReason::OutOfReach,
            UseItemOnResyncOptions::WITH_BUCKET,
        )
        .await
        .map(|()| true);
    }
    let target_precondition = if target_is_clicked {
        clicked_precondition
    } else if let Some(precondition) = published_block_precondition(state, target) {
        precondition
    } else {
        return reject_use_item_on_with_resync(
            state,
            writer,
            hand,
            sequence,
            clicked_pos,
            adjacent,
            UseItemOnNoOpReason::ClickedCellUnavailable,
            UseItemOnResyncOptions::WITH_BUCKET,
        )
        .await
        .map(|()| true);
    };
    // Solid cells and fluid sources refuse the fluid; the client predicted a
    // placement, so resync authoritatively instead of falling through.
    if !bucket_fluid_placeable(state, target_precondition.expected_state) {
        return reject_use_item_on_with_resync(
            state,
            writer,
            hand,
            sequence,
            clicked_pos,
            adjacent,
            UseItemOnNoOpReason::TargetBlockedOrUnplaceable,
            UseItemOnResyncOptions::WITH_BUCKET,
        )
        .await
        .map(|()| true);
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
            precondition: target_precondition,
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
    Ok(true)
}

/// Authoritative eye + look vector from the validated pose. `None` fails
/// closed on non-finite movement state. Look math is owned by
/// [`super::player_look_direction`]; this only validates and normalizes.
fn bucket_eye_and_look(pose: PlayerPose) -> Option<([f64; 3], [f64; 3])> {
    if ![pose.x, pose.y, pose.z]
        .iter()
        .all(|value| value.is_finite())
    {
        return None;
    }
    if !pose.yaw.is_finite() || !pose.pitch.is_finite() {
        return None;
    }
    let direction = super::player_look_direction(pose);
    let direction = [direction.x, direction.y, direction.z];
    let length =
        (direction[0] * direction[0] + direction[1] * direction[1] + direction[2] * direction[2])
            .sqrt();
    if length <= f64::EPSILON {
        return None;
    }
    Some((
        [pose.x, pose.y + pose.eye_height(), pose.z],
        [
            direction[0] / length,
            direction[1] / length,
            direction[2] / length,
        ],
    ))
}

/// Vanilla `ClipContext.Fluid.SOURCE_ONLY` trace: the first source-fluid cell
/// is a hit, solid cells occlude, and air / vegetation / flowing fluid are
/// transparent to the ray. Unloaded cells miss.
fn raycast_fluid_source(
    state: &InteractionState,
    eye: [f64; 3],
    direction: [f64; 3],
) -> Option<mc_world::BlockPos> {
    let mut cell = [
        eye[0].floor() as i32,
        eye[1].floor() as i32,
        eye[2].floor() as i32,
    ];
    let mut step = [0; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for (axis, origin) in eye.iter().enumerate() {
        if direction[axis] > 0.0 {
            step[axis] = 1;
            let boundary = cell[axis] as f64 + 1.0;
            t_max[axis] = (boundary - origin) / direction[axis];
            t_delta[axis] = 1.0 / direction[axis];
        } else if direction[axis] < 0.0 {
            step[axis] = -1;
            let boundary = cell[axis] as f64;
            t_max[axis] = (boundary - origin) / direction[axis];
            t_delta[axis] = 1.0 / -direction[axis];
        }
    }
    loop {
        let pos = mc_world::BlockPos {
            x: cell[0],
            y: cell[1],
            z: cell[2],
        };
        let current = state.world_read.get_cached_block(pos)?;
        if state
            .block_facts
            .fluid(current.0)
            .is_some_and(|fluid| fluid.source)
        {
            return Some(pos);
        }
        if !bucket_ray_passable(state, current) {
            return None;
        }
        let axis = if t_max[0] < t_max[1] {
            if t_max[0] < t_max[2] { 0 } else { 2 }
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };
        if t_max[axis] > BUCKET_FLUID_RAYCAST_RANGE {
            return None;
        }
        cell[axis] += step[axis];
        t_max[axis] += t_delta[axis];
    }
}

/// Cells the bucket ray passes through: air, replaceable vegetation, and
/// flowing (non-source) fluid. Everything else occludes.
fn bucket_ray_passable(state: &InteractionState, id: mc_world::BlockStateId) -> bool {
    if bucket_is_air(state, id) {
        return true;
    }
    if state.block_facts.fluid(id.0).is_some() {
        return true;
    }
    bucket_has_empty_collision(state, id)
}

/// Vanilla `BlockState.canBeReplaced(fluid)` for bucket placement: air,
/// flowing fluid, and replaceable vegetation accept the fluid; solid cells
/// and fluid sources refuse it.
fn bucket_fluid_placeable(state: &InteractionState, id: mc_world::BlockStateId) -> bool {
    if bucket_is_air(state, id) {
        return true;
    }
    if let Some(fluid) = state.block_facts.fluid(id.0) {
        return !fluid.source;
    }
    bucket_has_empty_collision(state, id)
}

/// Clicked cells vanilla replaces in place instead of offsetting: vegetation
/// with an empty collision shape (never air, never fluid).
fn bucket_replaceable_plant(state: &InteractionState, id: mc_world::BlockStateId) -> bool {
    !bucket_is_air(state, id)
        && state.block_facts.fluid(id.0).is_none()
        && bucket_has_empty_collision(state, id)
}

fn bucket_is_air(state: &InteractionState, id: mc_world::BlockStateId) -> bool {
    state.blocks.by_id(id).is_some_and(|block| {
        matches!(
            block.block.id.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    })
}

/// Fingerprinted empty-collision check; unknown or mismatched states fail
/// closed (solid / not replaceable).
fn bucket_has_empty_collision(state: &InteractionState, id: mc_world::BlockStateId) -> bool {
    state
        .blocks
        .by_id(id)
        .and_then(|block| {
            vanilla_collision_shapes().get_for_state(
                id.0,
                &block.block.id,
                block.properties.as_slice(),
            )
        })
        .is_some_and(|shape| shape.is_empty())
}

/// Authoritative correction for a `UseItem` raycast hit that is no longer a
/// pickup target: resync the cell plus the held bucket, then ack.
async fn resync_bucket_use_miss<W>(
    state: &mut InteractionState,
    writer: &mut W,
    sequence: i32,
    held_slot: usize,
    held: &ItemStack,
    pos: mc_world::BlockPos,
) -> Result<bool, ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if let Some(current) = state.world_read.get_cached_block(pos) {
        send_loaded_block_edit_resyncs(
            state,
            writer,
            &[BlockEdit {
                pos,
                new_state: current,
            }],
        )
        .await?;
    }
    write_inventory_slot_updates(state, writer, vec![(held_slot, held.clone())]).await?;
    write_block_ack(writer, state.compression, sequence).await?;
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
            // Rejected (stale precondition, moved held stack, or a full
            // inventory): the authoritative blocks + held slot are already
            // resent above, so this is terminal — falling through would ack
            // the same sequence a second time.
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
            return Ok(true);
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
        stew_effects: Vec::new(),
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
    struct BucketWorld {
        empty_bucket: u32,
        water_bucket: u32,
        air: mc_world::BlockStateId,
        stone: mc_world::BlockStateId,
        water: mc_world::BlockStateId,
        grass: mc_world::BlockStateId,
    }

    fn bucket_test_world() -> (InteractionState, BucketWorld) {
        let report = mc_data::blocks::solaris_required_blocks_report();
        let items = Arc::new(mc_data::items::solaris_required_items());
        let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
        let mut state =
            interaction_state_for_items_and_blocks(Arc::clone(&items), Arc::clone(&blocks));
        state.block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &report,
        ));
        let world = BucketWorld {
            empty_bucket: items
                .id_of(&Identifier::parse("minecraft:bucket").unwrap())
                .unwrap(),
            water_bucket: items
                .id_of(&Identifier::parse("minecraft:water_bucket").unwrap())
                .unwrap(),
            air: air_state_id(&state.blocks),
            stone: blocks
                .block(&Identifier::parse("minecraft:stone").unwrap())
                .unwrap()
                .default,
            water: state
                .item_to_block
                .fluid_source_state(mc_data::block_facts::FluidKind::Water)
                .unwrap(),
            grass: blocks
                .block(&Identifier::parse("minecraft:short_grass").unwrap())
                .unwrap()
                .default,
        };
        (state, world)
    }

    async fn seed_bucket_chunk(state: &InteractionState, air: mc_world::BlockStateId) {
        let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
        state
            .world
            .lock()
            .await
            .insert_generated_chunk(
                chunk_pos,
                mc_world::Chunk::empty(
                    chunk_pos,
                    air,
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
    }

    async fn set_bucket_block(
        state: &InteractionState,
        pos: mc_world::BlockPos,
        id: mc_world::BlockStateId,
    ) {
        state.world.lock().await.set_block_at(pos, id).unwrap();
    }

    async fn bucket_block_at(
        state: &InteractionState,
        pos: mc_world::BlockPos,
    ) -> Option<mc_world::BlockStateId> {
        state.world.lock().await.get_cached_block(pos)
    }

    fn spawn_bucket_owner(state: &mut InteractionState) -> tokio::task::JoinHandle<()> {
        let sessions = Arc::clone(&state.sessions);
        let world = Arc::clone(&state.world);
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(state.session_id);
        tokio::spawn(async move {
            while owner.wait_for_command().await {
                owner.process_tick_with_world(&sessions, Some(&world), None, 64);
            }
        })
    }

    fn standing_pose(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> PlayerPose {
        let mut pose = PlayerPose::new(x, y, z);
        pose.yaw = yaw;
        pose.pitch = pitch;
        pose
    }

    struct BucketWireLog {
        acks: Vec<i32>,
        slots: Vec<(i16, u32)>,
        blocks: Vec<(i64, i32)>,
    }

    fn decode_bucket_wire(state: &InteractionState, bytes: &[u8]) -> BucketWireLog {
        let mut buf = BytesMut::from(bytes);
        let mut log = BucketWireLog {
            acks: Vec::new(),
            slots: Vec::new(),
            blocks: Vec::new(),
        };
        while let Some(mut frame) =
            mc_protocol::frame::try_decode_frame(&mut buf, state.compression).unwrap()
        {
            if frame.id == BlockChangedAck::ID {
                log.acks
                    .push(BlockChangedAck::decode(&mut frame.body).unwrap().sequence);
            } else if frame.id == ClientboundContainerSetSlot::ID {
                let slot = ClientboundContainerSetSlot::decode(&mut frame.body).unwrap();
                log.slots.push((slot.slot, slot.item_stack.item_id));
            } else if frame.id == BlockUpdate::ID {
                let update = BlockUpdate::decode(&mut frame.body).unwrap();
                log.blocks.push((update.position, update.state_id));
            }
        }
        assert!(buf.is_empty(), "unexpected trailing wire bytes");
        log
    }

    #[tokio::test]
    async fn empty_bucket_use_item_picks_up_raycast_source_water() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let water_pos = mc_world::BlockPos { x: 0, y: 65, z: 3 };
        set_bucket_block(&state, water_pos, world.water).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.empty_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketRaycast",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 7,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, water_pos).await, Some(world.air));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.water_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&7));
        assert!(log.slots.contains(&(held_slot as i16, world.water_bucket)));
    }

    #[tokio::test]
    async fn empty_bucket_use_item_miss_only_acks() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.empty_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketRaycastMiss",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 7,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.empty_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert_eq!(log.acks, vec![7]);
        assert!(log.blocks.is_empty());
        assert!(log.slots.is_empty());
    }

    #[tokio::test]
    async fn empty_bucket_use_item_occluded_source_misses() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let water_pos = mc_world::BlockPos { x: 0, y: 65, z: 3 };
        set_bucket_block(
            &state,
            mc_world::BlockPos { x: 0, y: 65, z: 2 },
            world.stone,
        )
        .await;
        set_bucket_block(&state, water_pos, world.water).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.empty_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketRaycastBlocked",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: 7,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, water_pos).await, Some(world.water));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.empty_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert_eq!(log.acks, vec![7]);
        assert!(log.slots.is_empty());
    }

    #[tokio::test]
    async fn filled_bucket_use_on_places_into_adjacent_air() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let clicked = mc_world::BlockPos { x: 0, y: 64, z: 1 };
        let target = mc_world::BlockPos { x: 0, y: 65, z: 1 };
        set_bucket_block(&state, clicked, world.stone).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.water_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketPlaceAir",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            11,
            clicked,
            Direction::Up,
            InteractionHand::MainHand,
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, target).await, Some(world.water));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.empty_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&11));
    }

    #[tokio::test]
    async fn filled_bucket_use_on_replaces_clicked_vegetation() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let clicked = mc_world::BlockPos { x: 0, y: 64, z: 1 };
        let above = mc_world::BlockPos { x: 0, y: 65, z: 1 };
        set_bucket_block(&state, clicked, world.grass).await;
        set_bucket_block(&state, above, world.stone).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.water_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketPlaceGrass",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            11,
            clicked,
            Direction::Up,
            InteractionHand::MainHand,
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, clicked).await, Some(world.water));
        assert_eq!(bucket_block_at(&state, above).await, Some(world.stone));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.empty_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&11));
    }

    #[tokio::test]
    async fn filled_bucket_use_on_rejects_out_of_reach_target() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let clicked = mc_world::BlockPos { x: 5, y: 65, z: 0 };
        let target = mc_world::BlockPos { x: 6, y: 65, z: 0 };
        set_bucket_block(&state, clicked, world.stone).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        state.inventory.slots[held_slot] = ItemStack::new(world.water_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketPlaceReach",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            11,
            clicked,
            Direction::East,
            InteractionHand::MainHand,
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, clicked).await, Some(world.stone));
        assert_eq!(bucket_block_at(&state, target).await, Some(world.air));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.water_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&11));
        assert!(log.slots.contains(&(held_slot as i16, world.water_bucket)));
        assert_eq!(log.blocks.len(), 2);
    }

    #[tokio::test]
    async fn filled_bucket_use_on_full_inventory_rejects_with_resync() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let clicked = mc_world::BlockPos { x: 0, y: 64, z: 1 };
        let target = mc_world::BlockPos { x: 0, y: 65, z: 1 };
        set_bucket_block(&state, clicked, world.stone).await;
        let held_slot = PlayerInventory::HOTBAR_BASE;
        for slot in 0..state.inventory.slots.len() {
            state.inventory.slots[slot] = ItemStack::new(world.water_bucket, 1);
        }
        state.inventory.slots[held_slot] = ItemStack::new(world.water_bucket, 2);
        register_survival_test_player(
            &mut state,
            "BucketPlaceFull",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            11,
            clicked,
            Direction::Up,
            InteractionHand::MainHand,
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, target).await, Some(world.air));
        assert_eq!(
            state.inventory.slots[held_slot],
            ItemStack::new(world.water_bucket, 2)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&11));
        assert!(log.slots.contains(&(held_slot as i16, world.water_bucket)));
    }

    #[tokio::test]
    async fn offhand_filled_bucket_use_on_places() {
        let (mut state, world) = bucket_test_world();
        seed_bucket_chunk(&state, world.air).await;
        let clicked = mc_world::BlockPos { x: 0, y: 64, z: 1 };
        let target = mc_world::BlockPos { x: 0, y: 65, z: 1 };
        set_bucket_block(&state, clicked, world.stone).await;
        state.inventory.slots[45] = ItemStack::new(world.water_bucket, 1);
        register_survival_test_player(
            &mut state,
            "BucketPlaceOffhand",
            SurvivalState::FULL,
            &XpState::default(),
        );
        let owner = spawn_bucket_owner(&mut state);
        let mut writer = Vec::new();
        let handled = handle_bucket_use_on(
            &mut state,
            &mut writer,
            GameMode::Survival,
            standing_pose(0.5, 64.0, 0.5, 0.0, 0.0),
            11,
            clicked,
            Direction::Up,
            InteractionHand::OffHand,
        )
        .await
        .unwrap();
        owner.abort();
        assert!(handled);
        assert_eq!(bucket_block_at(&state, target).await, Some(world.water));
        assert_eq!(
            state.inventory.slots[45],
            ItemStack::new(world.empty_bucket, 1)
        );
        let log = decode_bucket_wire(&state, &writer);
        assert!(log.acks.contains(&11));
        assert!(log.slots.contains(&(45, world.empty_bucket)));
    }
}
