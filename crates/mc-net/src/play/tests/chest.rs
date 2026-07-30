use std::sync::Arc;

use mc_data::items::ItemReport;

use super::{
    BlockStateId, ChestBlockEntity, ChestView, ChestWindow, Chunk, ChunkPos, ContainerInput,
    FurnaceSlot, Identifier, ItemRegistry, ItemStack, PlayerInventory, PlayerPose,
    ServerboundContainerClick, apply_chest_swap_click, apply_chest_throw_click,
    decode_container_set_content_packets, furnace_slot_to_stack, handle_chest_container_click,
    interaction_state_for_items, load_chest_commit_snapshot, stack_to_furnace_slot,
};

#[test]
fn chest_window_swap_and_throw_mutate_storage_slots() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(stone_id, 2);
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };
    view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));

    assert!(apply_chest_swap_click(&mut state, &mut view, 0, 0));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 2)
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(dirt_id, 5)
    );

    let dropped = apply_chest_throw_click(&mut state, &mut view, 0, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(stone_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 1)
    );
}

#[tokio::test]
async fn stale_chest_click_after_peer_mutation_resyncs_without_mutating_storage() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));
        storage.set_chest_block_entity(position, chest).unwrap();
    }

    let window = ChestWindow::new(vec![position], 7);
    {
        let mut storage = state.world.lock().await;
        let mut chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("test chest exists");
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(stone_id, 2));
        storage.set_chest_block_entity(position, chest).unwrap();
    }
    let _ = state
        .sessions
        .try_chest_slot_dispatches(position, 1, 1, 99, vec![ItemStack::new(stone_id, 2)])
        .expect("peer mutation claims initial chest state");

    let mut writer = Vec::new();
    let returned = handle_chest_container_click(
        &mut state,
        &mut writer,
        window,
        PlayerPose::new(0.5, 65.0, 0.5),
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();

    assert_eq!(returned.state_id, 2);
    assert!(state.carried_item.is_empty());
    {
        let mut storage = state.world.lock().await;
        let chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("test chest exists");
        assert_eq!(
            furnace_slot_to_stack(&chest.slots[0]),
            ItemStack::new(stone_id, 2)
        );
    }
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 2);
    assert_eq!(packets[0].items[0], ItemStack::new(stone_id, 2));
    assert!(packets[0].carried_item.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chest_commit_snapshot_pairs_world_contents_with_viewer_state_id() {
    let state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut initial = ChestBlockEntity::default();
    initial.slots[0] = FurnaceSlot {
        item_id: 10,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    {
        let mut storage = state.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage
            .set_chest_block_entity(position, initial.clone())
            .unwrap();
    }

    let world = Arc::clone(&state.world);
    let sessions = Arc::clone(&state.sessions);
    let mut guard = world.lock().await;
    let window = ChestWindow::new(vec![position], 7);
    let mut snapshot = Box::pin(load_chest_commit_snapshot(&state, &window));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(snapshot.as_mut(), cx).is_pending(),
            "snapshot must wait for the held world lock"
        );
        std::task::Poll::Ready(())
    })
    .await;
    let mut updated = initial;
    updated.slots[0].count = 1;
    guard
        .set_chest_block_entity(position, updated.clone())
        .unwrap();
    let (state_id, _) = sessions
        .try_chest_slot_dispatches(position, 1, 1, 99, vec![ItemStack::new(10, 1)])
        .unwrap();
    assert_eq!(state_id, 2);
    drop(guard);

    let (view, observed_state_id) = snapshot.await.unwrap();
    assert_eq!(observed_state_id, 2);
    assert_eq!(view.chests, vec![updated]);
}

#[tokio::test]
async fn shared_chest_same_version_click_commits_once_and_conserves_items() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut actor = interaction_state_for_items(Arc::clone(&items));
    let mut observer = interaction_state_for_items(items);
    observer.world = Arc::clone(&actor.world);
    observer.sessions = Arc::clone(&actor.sessions);
    observer.session_id = 2;

    let position = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    {
        let mut storage = actor.world.lock().await;
        let cpos = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let mut chest = ChestBlockEntity::default();
        chest.slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 2));
        storage.set_chest_block_entity(position, chest).unwrap();
    }

    let click = |container_id| ServerboundContainerClick {
        container_id,
        state_id: 1,
        slot_num: 0,
        button_num: 1,
        container_input: ContainerInput::Pickup,
        changed_slots: Vec::new(),
        carried_item: mc_protocol::packets::play::HashedStack::Actual {
            item_id: dirt_id,
            count: 1,
            components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
        },
    };
    let mut actor_writer = Vec::new();
    let actor_window = handle_chest_container_click(
        &mut actor,
        &mut actor_writer,
        ChestWindow::new(vec![position], 7),
        PlayerPose::new(0.5, 65.0, 0.5),
        click(7),
    )
    .await
    .unwrap();
    let mut observer_writer = Vec::new();
    let observer_window = handle_chest_container_click(
        &mut observer,
        &mut observer_writer,
        ChestWindow::new(vec![position], 8),
        PlayerPose::new(0.5, 65.0, 0.5),
        click(8),
    )
    .await
    .unwrap();

    assert_eq!(actor_window.state_id, 3);
    assert_eq!(observer_window.state_id, 3);
    assert_eq!(actor.carried_item, ItemStack::new(dirt_id, 1));
    assert!(observer.carried_item.is_empty());
    let chest_count = {
        let mut storage = actor.world.lock().await;
        let chest = storage
            .chest_block_entity(position)
            .unwrap()
            .expect("shared chest exists");
        assert_eq!(chest.slots[0].item_id, dirt_id);
        chest.slots[0].count
    };
    assert_eq!(chest_count, 1);
    assert_eq!(
        chest_count + actor.carried_item.count + observer.carried_item.count,
        2,
        "the rejected stale click must neither duplicate nor delete the shared stack"
    );
    let observer_packets = decode_container_set_content_packets(&observer_writer);
    assert_eq!(observer_packets.len(), 1);
    assert_eq!(observer_packets[0].state_id, 3);
    assert_eq!(observer_packets[0].items[0], ItemStack::new(dirt_id, 1));
    assert!(observer_packets[0].carried_item.is_empty());
}
