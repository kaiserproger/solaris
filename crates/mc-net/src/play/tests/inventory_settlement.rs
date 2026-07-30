use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use mc_data::items::ItemReport;
use tokio::sync::mpsc;

use crate::play::tests::{
    interaction_state_for_items, no_script_player_context, play_loop_slow_client_test_config,
    spawn_test_simulation_owner,
};
use crate::play::{
    ActiveContainer, CommandPermissions, ContainerClickContext, ContainerInput, CraftedItem,
    CraftingTableWindow, EnchantingTableWindow, EntityItemStack, GameMode, Identifier,
    ItemRegistry, ItemStack, LoggedInProfile, PlayerInventory, PlayerPersistedState, PlayerPose,
    RegisteredSessionCleanup, ScriptCraftingSource, ScriptEvent, ScriptEventSink,
    ScriptGameplayEventPublisher, ScriptPlayerId, ServerboundContainerClick, SessionRegistry,
    SurvivalState, Vec3, XpState, crafted_item_from_inventory_delta,
    crafting_table_input_projection, handle_container_click, handle_crafting_container_click,
    handle_enchanting_container_click, load_player_state, refresh_crafting_result,
    refresh_inventory_crafting_result, settle_disconnected_cursor, settle_disconnected_inventory,
    settle_recovered_player_inventory, simulation_channel, store_active_container,
    store_inventory_crafting_inputs,
};

#[tokio::test]
async fn disconnected_cursor_is_preserved_when_simulation_owner_is_unavailable() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.carried_item = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DisconnectCursor"),
        name: "DisconnectCursor".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, owner) = simulation_channel();
    drop(owner);
    state.simulation = simulation.for_session(session_id);

    settle_disconnected_cursor(&mut state, &saved).await;

    assert_eq!(state.carried_item, ItemStack::new(10, 2));
    assert_eq!(saved.lock().unwrap().carried_item, ItemStack::new(10, 2));
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn disconnected_cursor_settlement_commits_inventory_and_drop_in_one_owner_turn() {
    let item = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.carried_item = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicDisconnectCursor"),
        name: "AtomicDisconnectCursor".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    settle_disconnected_cursor(&mut state, &saved).await;
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.carried_item.is_empty());
    assert!(saved.lock().unwrap().carried_item.is_empty());
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].snapshot.item_stack,
        Some(EntityItemStack::new(10, 2))
    );
    assert_eq!(drops[0].snapshot.position, Vec3::new(4.5, 66.0, 6.5));
}

#[tokio::test]
async fn crafting_table_close_commits_returned_inputs_and_all_drops_in_one_owner_turn() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let cobblestone = Identifier::parse("minecraft:cobblestone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: cobblestone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 2);
    window.input[1] = ItemStack::new(11, 3);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicCraftingClose"),
        name: "AtomicCraftingClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.crafting_table_input = crafting_table_input;
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_active_container(&mut state, pose).await.unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.active_container.is_none());
    assert_eq!(
        saved.lock().unwrap().inventory.slots[9],
        ItemStack::new(10, 64)
    );
    let mut drops = state
        .sessions
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.item_stack.unwrap())
        .collect::<Vec<_>>();
    drops.sort_by_key(|stack| stack.item_id);
    assert_eq!(
        drops,
        vec![EntityItemStack::new(10, 1), EntityItemStack::new(11, 3)]
    );
}

#[tokio::test]
async fn inventory_crafting_close_commits_returned_inputs_and_drop_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicInventoryCraftingClose"),
        name: "AtomicInventoryCraftingClose".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    store_inventory_crafting_inputs(&mut state, pose)
        .await
        .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    let saved = saved.lock().unwrap();
    assert!(saved.inventory.slots[1].is_empty());
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 64));
    drop(saved);
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 1);
    assert_eq!(
        drops[0].snapshot.item_stack,
        Some(EntityItemStack::new(10, 1))
    );
}

#[tokio::test]
async fn login_recovers_persisted_container_inputs_and_cursor_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let mut recovered = PlayerPersistedState::new_default(pose);
    recovered.carried_item = ItemStack::new(10, 1);
    let mut crafting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
    crafting_table_input[0] = ItemStack::new(10, 2);
    recovered.crafting_table_input = Some(Box::new(crafting_table_input));
    let mut enchanting_table_input = std::array::from_fn(|_| ItemStack::EMPTY);
    enchanting_table_input[0] = ItemStack::new(10, 3);
    recovered.enchanting_table_input = Some(Box::new(enchanting_table_input));
    state.inventory = recovered.inventory.clone();
    state.carried_item = recovered.carried_item.clone();

    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredContainerLogin"),
        name: "RecoveredContainerLogin".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let saved = Arc::new(Mutex::new(recovered.clone()));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    settle_recovered_player_inventory(&mut state, &recovered)
        .await
        .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert_eq!(
        state
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        6
    );
    assert!(state.carried_item.is_empty());
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots, state.inventory.slots);
    assert!(saved.carried_item.is_empty());
    assert!(saved.crafting_table_input.is_none());
    assert!(saved.enchanting_table_input.is_none());
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[tokio::test]
async fn cancelled_connection_cleanup_retains_owner_state_for_checkpoint() {
    let sessions = Arc::new(SessionRegistry::new());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CancelledOwnerState"),
        name: "CancelledOwnerState".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut state = PlayerPersistedState::new_default(pose);
    state.carried_item = ItemStack::new(10, 3);
    sessions.register_player_persistence(session_id, Arc::new(Mutex::new(state)));

    let observed = sessions.player_save_generation();
    let mut save_requested = Box::pin(sessions.wait_for_player_save_request(observed));
    std::future::poll_fn(|cx| {
        assert!(
            save_requested.as_mut().poll(cx).is_pending(),
            "save request must wait for connection cleanup"
        );
        Poll::Ready(())
    })
    .await;
    let cleanup =
        RegisteredSessionCleanup::new(Arc::clone(&sessions), session_id, None, None, None);
    drop(cleanup);
    tokio::time::timeout(Duration::from_secs(1), save_requested)
        .await
        .expect("connection cleanup must push a player save request");

    let snapshots = sessions.persisted_player_states();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].0, profile.uuid);
    assert_eq!(snapshots[0].1.carried_item, ItemStack::new(10, 3));
    assert_eq!(
        sessions
            .recoverable_player_state(profile.uuid)
            .unwrap()
            .carried_item,
        ItemStack::new(10, 3)
    );
}

#[tokio::test]
async fn periodic_checkpoint_persists_cancelled_connection_owner_state() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut config = play_loop_slow_client_test_config();
    config.items = Arc::clone(&items);
    config.world = Some(Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open(tmp.path(), Arc::clone(&config.blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    )));
    let sessions = Arc::new(SessionRegistry::new());
    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CheckpointCancelledOwnerState"),
        name: "CheckpointCancelledOwnerState".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = sessions.register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut state = PlayerPersistedState::new_default(pose);
    state.carried_item = ItemStack::new(10, 3);
    sessions.register_player_persistence(session_id, Arc::new(Mutex::new(state)));
    drop(RegisteredSessionCleanup::new(
        Arc::clone(&sessions),
        session_id,
        None,
        None,
        None,
    ));

    let shutdown = crate::server::ShutdownHandle::default();
    let (simulation, mut owner) = simulation_channel();
    let mut save = std::pin::pin!(crate::server::save_periodic_checkpoint(
        &config,
        sessions.as_ref(),
        &simulation,
        &shutdown,
    ));
    let command_ready = tokio::select! {
        report = &mut save => panic!("checkpoint completed before owner barrier: {report:?}"),
        ready = owner.wait_for_command() => ready,
    };
    assert!(command_ready);
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, config.world.as_ref(), None, 1)
            .processed,
        1
    );

    let report = save.await.expect("checkpoint was not superseded");
    assert!(report.is_ok(), "checkpoint errors: {:?}", report.errors);
    assert_eq!(report.players_saved, 1);
    let loaded = load_player_state(
        tmp.path(),
        profile.uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();
    assert_eq!(loaded.carried_item, ItemStack::new(10, 3));
    assert!(sessions.persisted_player_states().is_empty());
}

#[tokio::test]
async fn disconnect_settles_table_grid_inventory_grid_and_cursor_in_one_owner_turn() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(10, 64);
    }
    state.inventory.slots[9] = ItemStack::new(10, 63);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.carried_item = ItemStack::new(10, 1);
    state.entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 2);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("AtomicCraftingDisconnect"),
        name: "AtomicCraftingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.carried_item = state.carried_item.clone();
    saved.crafting_table_input = crafting_table_input;
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert!(state.active_container.is_none());
    let saved = saved.lock().unwrap();
    assert!(saved.inventory.slots[1].is_empty());
    assert_eq!(saved.inventory.slots[9], ItemStack::new(10, 64));
    assert!(saved.carried_item.is_empty());
    drop(saved);
    let drops = state.sessions.persisted_entity_records();
    assert_eq!(drops.len(), 3);
    assert_eq!(
        drops
            .iter()
            .map(|record| record.snapshot.item_stack.as_ref().unwrap().count)
            .sum::<i32>(),
        4
    );
}

#[tokio::test]
async fn disconnect_recovers_crafting_grid_after_connection_projection_is_lost() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(10, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredCraftingDisconnect"),
        name: "RecoveredCraftingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(CraftingTableWindow::new(7)),
        None,
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 10,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        None,
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 3,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.input[0], ItemStack::new(10, 1));
    drop(window);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    let saved = saved.lock().unwrap();
    assert_eq!(
        saved
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1,
        "the owner aggregate must recover a grid item after the connection projection is gone"
    );
    assert!(saved.carried_item.is_empty());
}

#[tokio::test]
async fn disconnect_recovers_enchanting_inputs_after_connection_projection_is_lost() {
    let items = Arc::new(mc_data::items::solaris_required_items());
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    state.inventory.slots[9] = ItemStack::new(pickaxe, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("RecoveredEnchantingDisconnect"),
        name: "RecoveredEnchantingDisconnect".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let xp = XpState::default();
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        &xp,
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
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        window,
        &xp,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 2,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.inputs[0], ItemStack::new(pickaxe, 1));
    drop(window);

    assert!(settle_disconnected_inventory(&mut state, &saved).await);
    let _ = stop.send(());
    task.await.unwrap();

    let saved = saved.lock().unwrap();
    assert_eq!(
        saved
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1,
        "the owner aggregate must recover an enchanting input after the connection projection is gone"
    );
    assert!(saved.carried_item.is_empty());
}

#[tokio::test]
async fn stale_crafting_click_rebuilds_grid_from_owner_projection() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: stone.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 12,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(stone)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1,
        },
    });
    let mut local_window = CraftingTableWindow::new(7);
    local_window.input[0] = ItemStack::new(11, 1);
    refresh_crafting_result(&state, &mut local_window);
    assert_eq!(local_window.result, ItemStack::new(12, 1));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleCraftOwner"),
        name: "StaleCraftOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let mut authoritative_input = std::array::from_fn(|_| ItemStack::EMPTY);
    authoritative_input[0] = ItemStack::new(10, 1);
    saved.crafting_table_input = Some(Box::new(authoritative_input.clone()));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);
    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );

    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(local_window),
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 12,
                count: 1,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(window.state_id, 1);
    assert_eq!(window.input, authoritative_input);
    assert!(state.carried_item.is_empty());
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(89))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 89 }
    ));
}

#[test]
fn quick_move_crafted_event_counts_every_output_batch() {
    let result = ItemStack::new(2, 4);
    let mut before = PlayerInventory::empty();
    before.slots[9] = ItemStack::new(2, 5);
    let mut after = before.clone();
    after.slots[9] = ItemStack::new(2, 17);

    assert_eq!(
        crafted_item_from_inventory_delta(&result, &before, &after),
        Some(CraftedItem {
            item_id: 2,
            count: 12,
            craft_count: 3,
        })
    );
}

#[tokio::test]
async fn crafting_table_result_commit_publishes_once_before_fifo_fence() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 4,
        },
    });
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(1, 1);
    refresh_crafting_result(&state, &mut window);
    assert_eq!(window.result, ItemStack::new(2, 4));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CraftEventOwner"),
        name: "CraftEventOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    saved.crafting_table_input = crafting_table_input_projection(&window.input);
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );
    let carried = mc_protocol::packets::play::HashedStack::Actual {
        item_id: 2,
        count: 4,
        components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
    };
    let mut writer = Vec::new();
    let window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        Box::new(window),
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 1,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: carried.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(window.state_id, 4);
    assert!(window.input.iter().all(ItemStack::is_empty));
    assert_eq!(state.carried_item, ItemStack::new(2, 4));
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 4,
            craft_count: 1,
            source: ScriptCraftingSource::CraftingTable,
            ..
        } if item_id == "minecraft:test_output"
    ));

    let mut window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 4,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: carried.clone(),
        },
    )
    .await
    .unwrap();
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(90))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 90 }
    ));

    window.input[0] = ItemStack::new(1, 1);
    refresh_crafting_result(&state, &mut window);
    saved.lock().unwrap().crafting_table_input = crafting_table_input_projection(&window.input);
    script_boundary.close_event_admission();
    window = handle_crafting_container_click(
        &mut state,
        &mut writer,
        window,
        Some(&script_events),
        GameMode::Survival,
        pose,
        ServerboundContainerClick {
            container_id: 7,
            state_id: 4,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 2,
                count: 8,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(window.state_id, 7);
    assert!(window.input.iter().all(ItemStack::is_empty));
    assert_eq!(state.carried_item, ItemStack::new(2, 8));

    let _ = stop.send(());
    task.await.unwrap();
}

#[tokio::test]
async fn inventory_result_paths_publish_only_after_owner_commit() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let ingredient = Identifier::parse("minecraft:test_ingredient").unwrap();
    let output = Identifier::parse("minecraft:test_output").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: ingredient.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: output.clone(),
            protocol_id: 2,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 4,
        },
    });
    state.inventory.slots[1] = ItemStack::new(1, 1);
    refresh_inventory_crafting_result(&mut state);
    assert_eq!(state.inventory.slots[0], ItemStack::new(2, 4));

    let pose = PlayerPose::new(1.5, 65.0, 2.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InvCraftOwner"),
        name: "InvCraftOwner".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&saved));
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        profile.uuid.to_string(),
        &profile.name,
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );
    let xp = XpState::default();
    let script_player_id = ScriptPlayerId::new(session_id);
    let script_context = no_script_player_context(session_id);
    let mut writer = Vec::new();
    let mismatch_state_id = state.inventory_state_id;

    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context: script_context.clone(),
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: mismatch_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 1,
                count: 1,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(state.inventory.slots[1], ItemStack::new(1, 1));
    assert!(state.carried_item.is_empty());
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(91))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 91 }
    ));

    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::new(99, 64);
    }
    saved.lock().unwrap().inventory = state.inventory.clone();
    let full_state_id = state.inventory_state_id;
    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context: script_context.clone(),
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: full_state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::empty(),
        },
    )
    .await
    .unwrap();
    assert_eq!(state.inventory.slots[1], ItemStack::new(1, 1));
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(92))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 92 }
    ));

    for slot in 9..=44 {
        state.inventory.slots[slot] = ItemStack::EMPTY;
    }
    saved.lock().unwrap().inventory = state.inventory.clone();
    let success_state_id = state.inventory_state_id;
    handle_container_click(
        &mut state,
        &mut writer,
        ContainerClickContext {
            game_mode: GameMode::Survival,
            survival_state: SurvivalState::FULL,
            xp_state: &xp,
            player_pose: pose,
            script_events: Some(&script_events),
            scripts: None,
            script_player_id,
            script_context,
        },
        ServerboundContainerClick {
            container_id: 0,
            state_id: success_state_id.wrapping_sub(1),
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Actual {
                item_id: 2,
                count: 4,
                components: mc_protocol::packets::play::HashedStackComponentHashes::empty(),
            },
        },
    )
    .await
    .unwrap();
    assert!(state.inventory.slots[1].is_empty());
    assert_eq!(state.carried_item, ItemStack::new(2, 4));
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 4,
            craft_count: 1,
            source: ScriptCraftingSource::Inventory,
            ..
        } if item_id == "minecraft:test_output"
    ));

    let _ = stop.send(());
    task.await.unwrap();
}
