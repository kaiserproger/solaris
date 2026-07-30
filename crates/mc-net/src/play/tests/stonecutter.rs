use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{ContainerInput, ItemStack, ServerboundContainerClick};
use mc_world::{BlockStateId, Chunk, ChunkPos};
use tokio::sync::mpsc;

use crate::error::ConnectionError;
use crate::login::LoggedInProfile;
use crate::play::tests::{
    decode_container_set_content_packets, interaction_state_for_blocks,
    interaction_state_for_items, simple_block, spawn_test_simulation_owner,
};
use crate::play::{
    ActiveContainer, ActiveShield, InteractionState, PlayerPersistedState, PlayerPose, SessionId,
    StonecutterWindow, apply_stonecutter_quick_move_click, handle_stonecutter_container_click,
    open_stonecutter_container, select_stonecutter_recipe, settle_disconnected_inventory,
    simulation_channel, stonecutter_input_from_projection, stonecutter_input_projection,
    store_active_container,
};

pub(super) fn stonecutter_test_recipe() -> mc_data::recipes::Recipe {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, StonecuttingRecipe,
    };

    Recipe {
        id: Identifier::parse("minecraft:test_stonecutter").unwrap(),
        kind: RecipeKind::Stonecutting(StonecuttingRecipe {
            ingredient: Ingredient {
                alternatives: vec![IngredientAlternative::Item(
                    Identifier::parse("minecraft:cobblestone").unwrap(),
                )],
            },
        }),
        result: RecipeResult {
            item: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
            count: 2,
        },
    }
}

pub(super) fn stonecutter_test_items() -> Arc<ItemRegistry> {
    Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            protocol_id: 0,
        },
        ItemReport {
            id: Identifier::parse("minecraft:cobblestone").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:cobblestone_slab").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 12,
        },
    ]))
}

fn register_stonecutter_owner(
    state: &mut InteractionState,
    name: &str,
    pose: PlayerPose,
    input: &ItemStack,
) -> (LoggedInProfile, SessionId, Arc<Mutex<PlayerPersistedState>>) {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut persisted = PlayerPersistedState::new_default(pose);
    persisted.inventory = state.inventory.clone();
    persisted.carried_item = state.carried_item.clone();
    persisted.crafting_table_input = stonecutter_input_projection(input);
    let persisted = Arc::new(Mutex::new(persisted));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&persisted));
    state.sessions.set_active_shield(
        session_id,
        state.shield_use.as_ref().map(|shield| ActiveShield {
            started_tick: shield.started_tick,
            slot: shield.slot,
            expected_stack: shield.stack.clone(),
        }),
    );
    state.session_id = session_id;
    (profile, session_id, persisted)
}

#[test]
fn stonecutter_invalid_selection_and_input_fail_closed() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(12, 1);

    assert!(!select_stonecutter_recipe(&state, &mut window, 0));
    assert!(window.result.is_empty());

    window.input = ItemStack::new(10, 1);
    assert!(!select_stonecutter_recipe(&state, &mut window, 1));
    assert!(window.result.is_empty());
}

#[test]
fn stonecutter_selection_uses_the_filtered_advertised_offer_order() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    let mut air = stonecutter_test_recipe();
    air.result.item = Identifier::parse("minecraft:air").unwrap();
    let mut over_stack = stonecutter_test_recipe();
    over_stack.result.count = 65;
    state
        .recipes
        .extend([air, over_stack, stonecutter_test_recipe()]);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 1);

    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    assert_eq!(window.result, ItemStack::new(11, 2));
    assert!(!select_stonecutter_recipe(&state, &mut window, 1));
    assert!(window.result.is_empty());
}

#[test]
fn stonecutter_quick_move_rejects_input_with_no_advertised_offer() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    let mut unsupported = stonecutter_test_recipe();
    unsupported.result.item = Identifier::parse("minecraft:missing_result").unwrap();
    state.recipes.push(unsupported);
    state.inventory.slots[9] = ItemStack::new(10, 1);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        2,
    ));
    assert!(state.inventory.slots[9].is_empty());
    assert_eq!(state.inventory.slots[36], ItemStack::new(10, 1));
    assert!(window.input.is_empty());
}

#[test]
fn stonecutter_quick_move_crafts_until_input_is_exhausted() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 2);
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        1
    ));

    assert!(window.input.is_empty());
    assert!(window.result.is_empty());
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 4));
    assert_eq!(
        window.input.count + state.inventory.slots[9].count / 2,
        2,
        "all craftable cobblestone must debit in the same candidate that credits the slabs",
    );
}

#[test]
fn stonecutter_quick_move_stops_at_exact_result_capacity() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        Identifier::parse("minecraft:cobblestone_slab").unwrap(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(16),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(12, 64);
    }
    state.inventory.slots[9] = ItemStack::new(11, 12);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = ItemStack::new(10, 4);
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    assert!(apply_stonecutter_quick_move_click(
        &mut state,
        &mut window,
        1
    ));

    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(window.result, ItemStack::new(11, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 16));
    assert_eq!(
        window.input.count + (state.inventory.slots[9].count - 12) / 2,
        4,
    );
}

#[tokio::test]
async fn stonecutter_result_pickup_commits_input_and_cursor_through_owner() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterPickupOwner", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(probe.snapshot().processed, 1);
    assert_eq!(window.state_id, 2);
    assert_eq!(window.input, ItemStack::new(10, 1));
    assert_eq!(state.carried_item, ItemStack::new(11, 2));
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.carried_item, state.carried_item);
    assert_eq!(
        stonecutter_input_from_projection(persisted.crafting_table_input.clone()),
        window.input,
    );
}

#[tokio::test]
async fn stonecutter_result_quick_move_commits_all_outputs_in_one_owner_turn() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 4);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterQuickMoveOwner", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(probe.snapshot().processed, 1);
    assert_eq!(window.state_id, 2);
    assert!(window.input.is_empty());
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 8));
    let persisted = persisted.lock().unwrap();
    assert_eq!(persisted.inventory.slots, state.inventory.slots);
    assert!(persisted.crafting_table_input.is_none());
}

#[tokio::test]
async fn stonecutter_output_plan_rejects_stale_owner_snapshot_and_resyncs() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StaleStonecutterOutput", pose, &input);
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let mut click = Box::pin(handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(click.as_mut(), cx).is_pending(),
            "stonecutter output must wait for its owner commit",
        );
        assert_eq!(probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    persisted.lock().unwrap().inventory.slots[9] = ItemStack::new(12, 1);
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);

    let window = click.await.unwrap();
    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(12, 1));
    assert!(state.carried_item.is_empty());
    let packets = decode_container_set_content_packets(&writer);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].state_id, 1);
    assert_eq!(packets[0].items[0], ItemStack::new(10, 2));
    assert!(packets[0].items[1].is_empty());
}

#[tokio::test]
async fn stonecutter_output_plan_rejects_stale_session_without_conservation_loss() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (_, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StaleStonecutterSession", pose, &input);
    let sessions = Arc::clone(&state.sessions);
    let (simulation, mut owner) = simulation_channel();
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let mut click = Box::pin(handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(click.as_mut(), cx).is_pending(),
            "stonecutter output must be planned before stale-session rejection",
        );
        assert_eq!(probe.snapshot().depth, 1);
        Poll::Ready(())
    })
    .await;
    let _ = sessions.unregister(session_id);
    assert_eq!(owner.process_tick(&sessions, 1).processed, 0);

    assert!(matches!(
        click.await,
        Err(ConnectionError::RuntimeUnavailable {
            operation: "committing stonecutter input"
        })
    ));
    assert_eq!(probe.snapshot().rejected_stale_session, 1);
    assert!(state.inventory.slots[9].is_empty());
    assert!(state.carried_item.is_empty());
    let persisted = persisted.lock().unwrap();
    assert!(persisted.inventory.slots[9].is_empty());
    assert_eq!(
        stonecutter_input_from_projection(persisted.crafting_table_input.clone()),
        ItemStack::new(10, 2),
    );
}

#[tokio::test]
async fn stonecutter_disconnect_rejoin_conserves_crafted_output_and_remaining_input() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let input = ItemStack::new(10, 2);
    let (profile, session_id, persisted) =
        register_stonecutter_owner(&mut state, "StonecutterRejoin", pose, &input);
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut window = StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 });
    window.input = input;
    assert!(select_stonecutter_recipe(&state, &mut window, 0));
    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        window,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    state.active_container = Some(ActiveContainer::Stonecutter(window));

    assert!(settle_disconnected_inventory(&mut state, &persisted).await);
    let _ = stop.send(());
    task.await.unwrap();
    assert_eq!(probe.snapshot().processed, 2);
    let _ = state
        .sessions
        .unregister_preserving_player_state(session_id);
    let recovered = state
        .sessions
        .recoverable_player_state(profile.uuid)
        .expect("settled stonecutter state must be recoverable on rejoin");
    let (tx, _rx) = mpsc::channel(8);
    let (rejoined, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    state
        .sessions
        .register_player_persistence(rejoined, Arc::new(Mutex::new(recovered.clone())));
    let (inventory, carried_item) = state
        .sessions
        .player_container_state(rejoined)
        .expect("rejoined player container state");

    assert_eq!(inventory.slots[9], ItemStack::new(10, 1));
    assert_eq!(inventory.slots[10], ItemStack::new(11, 2));
    assert!(carried_item.is_empty());
    assert!(recovered.crafting_table_input.is_none());
    assert_eq!(
        inventory.slots[9].count + inventory.slots[10].count / 2,
        2,
        "disconnect and rejoin must conserve the two original cobblestone",
    );
}

#[tokio::test]
async fn stale_stonecutter_click_rebuilds_input_from_owner_projection() {
    let mut state = interaction_state_for_items(stonecutter_test_items());
    state.recipes.push(stonecutter_test_recipe());
    state.inventory.slots[9] = ItemStack::new(12, 1);
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleStonecutterProjection"),
        name: "StaleStonecutterProjection".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let mut authoritative_input = std::array::from_fn(|_| ItemStack::EMPTY);
    authoritative_input[0] = ItemStack::new(10, 2);
    saved.crafting_table_input = Some(Box::new(authoritative_input));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_stonecutter_container_click(
        &mut state,
        &mut writer,
        StonecutterWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, ItemStack::new(10, 2));
    assert_eq!(state.inventory.slots[9], ItemStack::new(12, 1));
    assert!(state.carried_item.is_empty());
}

#[tokio::test]
async fn stonecutter_close_reopen_conserves_input_through_one_owner_turn() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:stonecutter"),
        ])
        .unwrap(),
    );
    let mut state = interaction_state_for_blocks(blocks);
    state.items = stonecutter_test_items();
    state.recipes.push(stonecutter_test_recipe());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let position = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    {
        let mut storage = state.world.lock().await;
        let chunk = ChunkPos { x: 0, z: 0 };
        storage
            .insert_generated_chunk(
                chunk,
                Chunk::empty(
                    chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        storage.set_block_at(position, BlockStateId(1)).unwrap();
    }
    let mut window = StonecutterWindow::at_position(7, position);
    window.input = ItemStack::new(10, 2);
    state.active_container = Some(ActiveContainer::Stonecutter(window));

    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicStonecutterClose"),
        name: "AtomicStonecutterClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.crafting_table_input = stonecutter_input_projection(&ItemStack::new(10, 2));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_active_container(&mut state, pose).await.unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    let mut writer = Vec::new();
    assert!(
        open_stonecutter_container(&mut state, &mut writer, pose, 8, position)
            .await
            .unwrap()
    );

    assert_eq!(probe.snapshot().processed, 1);
    let Some(ActiveContainer::Stonecutter(window)) = state.active_container.as_ref() else {
        panic!("stonecutter must reopen");
    };
    assert!(window.input.is_empty());
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 2));
    assert!(saved.crafting_table_input.is_none());
}
