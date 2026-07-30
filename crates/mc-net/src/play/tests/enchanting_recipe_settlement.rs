use super::{
    ActiveContainer, CommandPermissions, ContainerInput, CraftedItem, CraftingTableWindow,
    EnchantingTableWindow, GameMode, Identifier, ItemFactsTable, ItemRegistry, ItemStack,
    LoggedInProfile, PlayerPersistedState, PlayerPose, ScriptCraftingSource, ScriptEvent,
    ScriptEventSink, ScriptGameplayEventPublisher, ScriptPlayerId, ServerboundContainerClick,
    ServerboundPlaceRecipe, SurvivalState, XpState, craft_recipe, crafting_table_input_projection,
    handle_enchanting_container_click, handle_place_recipe, interaction_state_for_items,
    register_interaction_player, settle_disconnected_inventory, simulation_channel,
    spawn_test_simulation_owner,
};
use mc_data::items::ItemReport;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[tokio::test]
async fn stale_enchanting_click_rebuilds_inputs_from_owner_projection() {
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 10,
        },
        ItemReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(11, 1);

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StaleEnchantingProjection"),
        name: "StaleEnchantingProjection".to_owned(),
    };
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) = state
        .sessions
        .register(&profile, (0, 0), 0, HashSet::new(), tx, pose);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory = state.inventory.clone();
    let authoritative_input = [ItemStack::new(10, 1), ItemStack::EMPTY];
    saved.enchanting_table_input = Some(Box::new(authoritative_input.clone()));
    let saved = Arc::new(Mutex::new(saved));
    state
        .sessions
        .register_player_persistence(session_id, saved);
    state.session_id = session_id;
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    state.simulation = simulation.for_session(session_id);

    let mut writer = Vec::new();
    let window = handle_enchanting_container_click(
        &mut state,
        &mut writer,
        EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 64, z: 0 }),
        &XpState::default(),
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
    assert_eq!(window.inputs, authoritative_input);
    assert_eq!(state.inventory.slots[9], ItemStack::new(11, 1));
    assert!(state.carried_item.is_empty());
}

#[tokio::test]
async fn disconnect_settlement_fails_closed_when_owner_is_unavailable() {
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[1] = ItemStack::new(10, 2);
    state.carried_item = ItemStack::new(10, 1);
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(10, 3);
    let crafting_table_input = crafting_table_input_projection(&window.input);
    state.active_container = Some(ActiveContainer::CraftingTable(Box::new(window)));

    let pose = PlayerPose::new(4.5, 65.0, 6.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("UnavailableCraftingDisconnect"),
        name: "UnavailableCraftingDisconnect".to_owned(),
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
    let (simulation, owner) = simulation_channel();
    drop(owner);
    state.simulation = simulation.for_session(session_id);

    assert!(!settle_disconnected_inventory(&mut state, &saved).await);

    let Some(ActiveContainer::CraftingTable(window)) = &state.active_container else {
        panic!("failed settlement must not discard the connection-local crafting grid");
    };
    assert_eq!(window.input[0], ItemStack::new(10, 3));
    assert_eq!(state.inventory.slots[1], ItemStack::new(10, 2));
    assert_eq!(state.carried_item, ItemStack::new(10, 1));
    let saved = saved.lock().unwrap();
    assert_eq!(saved.inventory.slots[1], ItemStack::new(10, 2));
    assert_eq!(saved.carried_item, ItemStack::new(10, 1));
    assert!(state.sessions.persisted_entity_records().is_empty());
}

#[test]
fn use_max_recipe_is_bounded_when_output_recreates_ingredient() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };

    let item = Identifier::parse("minecraft:loop_item").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: item.clone(),
        protocol_id: 1,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[9] = ItemStack::new(1, 1);
    let recipe = Recipe {
        id: Identifier::parse("minecraft:loop_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(item.clone())],
            }],
        }),
        result: RecipeResult { item, count: 1 },
    };

    let outcome = craft_recipe(&mut state, &recipe, true).expect("one bounded craft");

    assert!(!outcome.changed_slots.is_empty());
    assert_eq!(
        outcome.crafted,
        CraftedItem {
            item_id: 1,
            count: 1,
            craft_count: 1,
        }
    );
    assert_eq!(state.inventory.slots[9], ItemStack::new(1, 1));
}

#[test]
fn use_max_recipe_reports_large_aggregate_without_partial_mutation_failure() {
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
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        output.clone(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(2_000_000_000),
            ..mc_data::item_components::ItemFacts::default()
        },
    )]));
    for slot in 9..=11 {
        state.inventory.slots[slot] = ItemStack::new(1, 1);
    }
    let recipe = Recipe {
        id: Identifier::parse("minecraft:test_large_output").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1_500_000_000,
        },
    };

    let outcome = craft_recipe(&mut state, &recipe, true).expect("three complete crafts");

    assert_eq!(outcome.crafted.item_id, 2);
    assert_eq!(outcome.crafted.count, 4_500_000_000);
    assert_eq!(outcome.crafted.craft_count, 3);
    assert_eq!(
        state.inventory.slots[9..=44]
            .iter()
            .map(|stack| i64::from(stack.count.max(0)))
            .sum::<i64>(),
        4_500_000_000
    );
}

#[tokio::test]
async fn placed_recipe_commits_inventory_and_publishes_aggregate_craft() {
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
    state.inventory.slots[9] = ItemStack::new(1, 1);
    state.recipes.push(Recipe {
        id: Identifier::parse("minecraft:test_recipe").unwrap(),
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: vec![Ingredient {
                alternatives: vec![IngredientAlternative::Item(ingredient)],
            }],
        }),
        result: RecipeResult {
            item: output,
            count: 1,
        },
    });
    let session_id = register_interaction_player(&mut state, "RecipeOwner");
    let (simulation, stop, task) = spawn_test_simulation_owner(Arc::clone(&state.sessions));
    let simulation_probe = simulation.clone();
    state.simulation = simulation.for_session(session_id);
    let mut writer = Vec::new();
    let one = NonZeroUsize::new(1).unwrap();
    let (script_boundary, mut script_endpoint) = mc_script::script_boundary_pair(one, one);
    let script_events = ScriptGameplayEventPublisher::new(
        ScriptEventSink::new(script_boundary.clone()),
        ScriptPlayerId::new(session_id),
        "recipe-owner",
        "RecipeOwner",
        CommandPermissions::from_op(false),
        "minecraft:overworld",
    );

    handle_place_recipe(
        &mut state,
        &mut writer,
        Some(&script_events),
        PlayerPose::new(0.5, 64.0, 0.5),
        GameMode::Survival,
        SurvivalState::FULL,
        ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: 0,
            use_max_items: false,
        },
    )
    .await
    .unwrap();

    let (owner_inventory, owner_carried_item) = state
        .sessions
        .player_container_state(session_id)
        .expect("registered owner inventory");
    let _ = stop.send(());
    task.await.unwrap();

    assert_eq!(simulation_probe.snapshot().processed, 1);
    assert_eq!(state.inventory.slots[9], ItemStack::new(2, 1));
    assert_eq!(owner_inventory.slots, state.inventory.slots);
    assert_eq!(owner_carried_item, state.carried_item);
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::PlayerItemCrafted {
            item_id,
            count: 1,
            craft_count: 1,
            source: ScriptCraftingSource::Inventory,
            game_mode: mc_script::ScriptGameMode::Survival,
            ..
        } if item_id == "minecraft:test_output"
    ));

    handle_place_recipe(
        &mut state,
        &mut writer,
        Some(&script_events),
        PlayerPose::new(0.5, 64.0, 0.5),
        GameMode::Survival,
        SurvivalState::FULL,
        ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: 0,
            use_max_items: true,
        },
    )
    .await
    .unwrap();
    script_boundary
        .try_enqueue_event(ScriptEvent::server_tick(88))
        .unwrap();
    assert!(matches!(
        script_endpoint.recv_event().await.unwrap().kind(),
        mc_script::ScriptEventKind::ServerTick { tick: 88 }
    ));
}
