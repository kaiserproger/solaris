//! Serverbound creative-mode inventory slot ingress.
//!
//! Fixes the creative-menu phantom-item cascade: vanilla 26.1.2 clients
//! report every creative inventory edit through
//! `ServerboundSetCreativeModeSlot` (javap:
//! `ServerboundSetCreativeModeSlotPacket(short slotNum, ItemStack
//! itemStack)`, game-SB wire id 0x38). Without a handler the server never
//! sees the stack, so placement, flint-and-steel and bucket use all miss,
//! and a client-side delete desyncs against the server's copy.
//!
//! Vanilla semantics (`ServerGamePacketListenerImpl
//! .handleSetCreativeModeSlot`, verified by javap against 26.1.2):
//! creative-capable players only; `slotNum` in `1..=45` writes the full
//! reported stack into that player-inventory slot (slot 0 is the crafting
//! result and stays server-authoritative); a negative `slotNum` is the
//! creative drop action — the reported stack is thrown into the world
//! (an empty stack destroys nothing); anything refused is answered with a
//! resync of the authoritative inventory so the client converges.

use tokio::io::AsyncWriteExt;

use mc_domain::GameMode;
use mc_protocol::packets::play::ServerboundSetCreativeModeSlot;

use crate::error::ConnectionError;

use super::super::{
    InteractionState, ItemStack, PlayerPose, commit_player_inventory_candidate, item_max_stack,
    write_inventory_content_resync, write_inventory_slot_updates,
};

/// Vanilla only lets a creative client write `inventoryMenu` slots
/// `1..=45` directly; slot 0 (crafting result) stays server-authoritative.
const CREATIVE_WRITABLE_SLOTS: std::ops::RangeInclusive<i16> = 1..=45;

pub(in crate::play) async fn handle_creative_mode_slot<W>(
    state: Option<&mut InteractionState>,
    writer: &mut W,
    game_mode: GameMode,
    player_pose: PlayerPose,
    packet: ServerboundSetCreativeModeSlot,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    let Some(state) = state else {
        tracing::debug!(
            slot = packet.slot,
            "SetCreativeModeSlot ignored — no world configured"
        );
        return Ok(());
    };
    if !matches!(game_mode, GameMode::Creative | GameMode::Spectator) {
        // Anti-cheat: a survival/adventure client cannot mint or delete
        // items by writing inventory slots; converge it on the server copy.
        tracing::debug!(
            slot = packet.slot,
            ?game_mode,
            "creative slot set refused outside creative mode"
        );
        return write_inventory_content_resync(state, writer).await;
    }
    if packet.slot < 0 {
        return handle_creative_drop(state, writer, player_pose, packet.item_stack).await;
    }
    if !CREATIVE_WRITABLE_SLOTS.contains(&packet.slot) {
        tracing::debug!(
            slot = packet.slot,
            "creative slot set out of range; resyncing authoritative inventory"
        );
        return write_inventory_content_resync(state, writer).await;
    }
    let stack = packet.item_stack;
    if !stack.is_empty() {
        let max_stack = item_max_stack(&state.item_facts, &state.items, &stack);
        if stack.count > max_stack {
            tracing::debug!(
                slot = packet.slot,
                count = stack.count,
                max_stack,
                "creative slot set refused: oversized stack"
            );
            return write_inventory_content_resync(state, writer).await;
        }
    }
    let slot = usize::from(packet.slot as u16);
    let mut updated_inventory = state.inventory.clone();
    updated_inventory.slots[slot] = stack;
    if !commit_player_inventory_candidate(
        state,
        updated_inventory,
        state.carried_item.clone(),
        None,
        player_pose,
    )
    .await?
    {
        return write_inventory_content_resync(state, writer).await;
    }
    // Acknowledge with the authoritative slot so client and server converge
    // (vanilla: `setRemoteSlot` + `broadcastChanges`).
    write_inventory_slot_updates(
        state,
        writer,
        vec![(slot, state.inventory.slots[slot].clone())],
    )
    .await
}

/// Creative drop action (`slotNum < 0`). Vanilla throws the reported stack
/// into the world (`player.drop(stack, true)`) and removes nothing from
/// the inventory because the stack came from the creative menu; the
/// empty-stack variant is the client-side destroy of a phantom and is a
/// server-side no-op. Vanilla additionally throttles drop spam
/// (`dropSpamThrottler`); this server bounds the equivalent flow through
/// the shared inventory-commit drop path instead.
async fn handle_creative_drop<W>(
    state: &mut InteractionState,
    writer: &mut W,
    player_pose: PlayerPose,
    stack: ItemStack,
) -> Result<(), ConnectionError>
where
    W: AsyncWriteExt + Unpin,
{
    if stack.is_empty() {
        return Ok(());
    }
    let unchanged_inventory = state.inventory.clone();
    if !commit_player_inventory_candidate(
        state,
        unchanged_inventory,
        state.carried_item.clone(),
        Some(stack),
        player_pose,
    )
    .await?
    {
        return write_inventory_content_resync(state, writer).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use bytes::BytesMut;
    use mc_data::items::solaris_required_items;
    use mc_data::{ItemStack, blocks::solaris_required_blocks_report};
    use mc_domain::GameMode;
    use mc_protocol::Packet;
    use mc_protocol::codec::Identifier;
    use mc_protocol::frame::try_decode_frame;
    use mc_protocol::packets::play::{ClientboundContainerSetContent, ClientboundContainerSetSlot};

    use super::*;
    use crate::play::simulation::{SimulationOwner, simulation_channel};
    use crate::play::tests::{
        interaction_state_for_items_and_blocks, register_survival_test_player,
    };
    use crate::play::{SurvivalState, XpState};

    const HOTBAR_SLOT: usize = 36;

    fn stone_item_id() -> u32 {
        solaris_required_items()
            .id_of(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
    }
    /// Build a world-backed interaction state, seed its inventory, then
    /// register the session owner (the persistence fence snapshots the
    /// seeded inventory at registration time) and wire the simulation.
    fn prepared_state(player: &str, seed: ItemStack) -> InteractionState {
        let items = Arc::new(solaris_required_items());
        let blocks = Arc::new(
            mc_world::BlockRegistry::from_report(&solaris_required_blocks_report()).unwrap(),
        );
        let mut state =
            interaction_state_for_items_and_blocks(Arc::clone(&items), Arc::clone(&blocks));
        if !seed.is_empty() {
            state.inventory.slots[HOTBAR_SLOT] = seed;
        }
        register_survival_test_player(&mut state, player, SurvivalState::FULL, &XpState::default());
        let (simulation, _owner) = simulation_channel();
        state.simulation = simulation.for_session(state.session_id);
        state
    }

    /// Poll the handler once (letting it enqueue its inventory commit),
    /// serve the commit from the simulation owner, then drive it to
    /// completion.
    async fn drive_creative_slot<W>(
        state: &mut InteractionState,
        writer: &mut W,
        game_mode: GameMode,
        packet: ServerboundSetCreativeModeSlot,
        owner: &mut SimulationOwner,
    ) -> Result<(), ConnectionError>
    where
        W: AsyncWriteExt + Unpin,
    {
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        // Clone the registry/world handles before the handler future takes
        // its exclusive borrow of `state`.
        let sessions = Arc::clone(&state.sessions);
        let world = Arc::clone(&state.world);
        let mut handler = Box::pin(handle_creative_mode_slot(
            Some(state),
            writer,
            game_mode,
            pose,
            packet,
        ));
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        if let Poll::Ready(result) = handler.as_mut().poll(&mut context) {
            return result;
        }
        owner.process_tick_with_world(&sessions, Some(&world), None, 1);
        handler.await
    }

    #[tokio::test]
    async fn creative_slot_set_writes_stack_and_publishes_slot() {
        let stone = stone_item_id();
        let mut state = prepared_state("CreativeSet", ItemStack::EMPTY);
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(state.session_id);
        let mut writer = Vec::new();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Creative,
            ServerboundSetCreativeModeSlot {
                slot: HOTBAR_SLOT as i16,
                item_stack: ItemStack::new(stone, 64),
            },
            &mut owner,
        )
        .await
        .expect("creative slot set commits");

        assert_eq!(
            state.inventory.slots[HOTBAR_SLOT],
            ItemStack::new(stone, 64),
            "creative take must land in the server inventory"
        );
        let mut buf = BytesMut::from(writer.as_slice());
        let mut frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("slot acknowledgement");
        assert_eq!(frame.id, ClientboundContainerSetSlot::ID);
        let slot = ClientboundContainerSetSlot::decode(&mut frame.body).unwrap();
        assert_eq!(slot.container_id, 0);
        assert_eq!(slot.slot, HOTBAR_SLOT as i16);
        assert_eq!(slot.item_stack, ItemStack::new(stone, 64));
        assert!(buf.is_empty(), "exactly one acknowledgement is published");
    }

    #[tokio::test]
    async fn creative_slot_destroy_clears_the_server_stack() {
        let stone = stone_item_id();
        let mut state = prepared_state("CreativeDestroy", ItemStack::new(stone, 12));
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(state.session_id);
        let mut writer = Vec::new();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Creative,
            ServerboundSetCreativeModeSlot {
                slot: HOTBAR_SLOT as i16,
                item_stack: ItemStack::EMPTY,
            },
            &mut owner,
        )
        .await
        .expect("creative destroy commits");

        assert!(
            state.inventory.slots[HOTBAR_SLOT].is_empty(),
            "creative destroy must clear the server stack"
        );
        let mut buf = BytesMut::from(writer.as_slice());
        let mut frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("slot acknowledgement");
        assert_eq!(frame.id, ClientboundContainerSetSlot::ID);
        let slot = ClientboundContainerSetSlot::decode(&mut frame.body).unwrap();
        assert_eq!(slot.container_id, 0);
        assert_eq!(slot.slot, HOTBAR_SLOT as i16);
        assert!(slot.item_stack.is_empty());
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn creative_drop_throws_stack_without_touching_inventory() {
        let stone = stone_item_id();
        let mut state = prepared_state("CreativeDrop", ItemStack::EMPTY);
        let (simulation, mut owner) = simulation_channel();
        state.simulation = simulation.for_session(state.session_id);
        let mut writer = Vec::new();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Creative,
            ServerboundSetCreativeModeSlot {
                slot: -1,
                item_stack: ItemStack::new(stone, 8),
            },
            &mut owner,
        )
        .await
        .expect("creative drop commits");

        assert!(
            state.inventory.slots[HOTBAR_SLOT].is_empty(),
            "the dropped creative stack never enters the inventory"
        );
        assert!(
            writer.is_empty(),
            "a drop mutates no inventory slot, so nothing is published"
        );
    }

    #[tokio::test]
    async fn survival_creative_slot_set_is_refused_and_resyncs() {
        let stone = stone_item_id();
        let mut state = prepared_state("SurvivalRefused", ItemStack::EMPTY);
        let mut writer = Vec::new();
        let (mut _simulation, mut owner) = simulation_channel();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Survival,
            ServerboundSetCreativeModeSlot {
                slot: HOTBAR_SLOT as i16,
                item_stack: ItemStack::new(stone, 64),
            },
            &mut owner,
        )
        .await
        .expect("refusal answers without error");

        assert!(
            state.inventory.slots[HOTBAR_SLOT].is_empty(),
            "a survival client cannot mint items"
        );
        let mut buf = BytesMut::from(writer.as_slice());
        let mut frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("resync content");
        assert_eq!(frame.id, ClientboundContainerSetContent::ID);
        let content = ClientboundContainerSetContent::decode(&mut frame.body).unwrap();
        assert_eq!(content.container_id, 0);
        assert_eq!(content.items.len(), state.inventory.slots.len());
        assert!(content.items[HOTBAR_SLOT].is_empty());
        assert!(buf.is_empty(), "only the resync is published");
    }

    #[tokio::test]
    async fn survival_creative_delete_cannot_remove_a_server_stack() {
        let stone = stone_item_id();
        let mut state = prepared_state("SurvivalDelete", ItemStack::new(stone, 12));
        let mut writer = Vec::new();
        let (mut _simulation, mut owner) = simulation_channel();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Survival,
            ServerboundSetCreativeModeSlot {
                slot: HOTBAR_SLOT as i16,
                item_stack: ItemStack::EMPTY,
            },
            &mut owner,
        )
        .await
        .expect("refusal answers without error");

        assert_eq!(
            state.inventory.slots[HOTBAR_SLOT],
            ItemStack::new(stone, 12),
            "a survival client cannot delete server-side items"
        );
        let mut buf = BytesMut::from(writer.as_slice());
        let mut frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("resync content");
        assert_eq!(frame.id, ClientboundContainerSetContent::ID);
        let content = ClientboundContainerSetContent::decode(&mut frame.body).unwrap();
        assert_eq!(content.items[HOTBAR_SLOT], ItemStack::new(stone, 12));
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn creative_slot_set_rejects_oversized_stack_with_resync() {
        let stone = stone_item_id();
        let mut state = prepared_state("CreativeOversized", ItemStack::EMPTY);
        let mut writer = Vec::new();
        let (mut _simulation, mut owner) = simulation_channel();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Creative,
            ServerboundSetCreativeModeSlot {
                slot: HOTBAR_SLOT as i16,
                item_stack: ItemStack::new(stone, 200),
            },
            &mut owner,
        )
        .await
        .expect("oversized refusal answers without error");

        assert!(state.inventory.slots[HOTBAR_SLOT].is_empty());
        let mut buf = BytesMut::from(writer.as_slice());
        let frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("resync content");
        assert_eq!(frame.id, ClientboundContainerSetContent::ID);
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn creative_slot_set_outside_writable_range_is_refused_with_resync() {
        let stone = stone_item_id();
        let mut state = prepared_state("CreativeRange", ItemStack::EMPTY);
        let mut writer = Vec::new();
        let (mut _simulation, mut owner) = simulation_channel();

        drive_creative_slot(
            &mut state,
            &mut writer,
            GameMode::Creative,
            ServerboundSetCreativeModeSlot {
                slot: 0,
                item_stack: ItemStack::new(stone, 1),
            },
            &mut owner,
        )
        .await
        .expect("out-of-range refusal answers without error");

        let mut buf = BytesMut::from(writer.as_slice());
        let frame = try_decode_frame(&mut buf, state.compression)
            .unwrap()
            .expect("resync content");
        assert_eq!(frame.id, ClientboundContainerSetContent::ID);
        assert!(buf.is_empty());
    }
}
