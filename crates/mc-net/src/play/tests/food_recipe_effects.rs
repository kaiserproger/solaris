use super::*;
use crate::play::persistence::save_player_state;

#[tokio::test]
async fn crafted_stew_keeps_flower_effect_through_wire_save_drop_and_consumption() {
    let names = [
        "bowl",
        "brown_mushroom",
        "red_mushroom",
        "lily_of_the_valley",
        "dandelion",
        "suspicious_stew",
        "crafting_table",
    ];
    let items = Arc::new(ItemRegistry::from_report(
        &names
            .iter()
            .enumerate()
            .map(|(index, name)| ItemReport {
                id: Identifier::parse(format!("minecraft:{name}")).unwrap(),
                protocol_id: index as u32 + 1,
            })
            .collect::<Vec<_>>(),
    ));
    let facts = mc_data::item_components::solaris_required_item_facts();
    let all_recipes = mc_data::recipes::solaris_required_recipes();
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.item_facts = Arc::new(facts);
    let mut crafted = Vec::new();
    for (flower, flower_id, expected_effect, duration) in [
        ("lily_of_the_valley", 4, "minecraft:poison", 220),
        ("dandelion", 5, "minecraft:saturation", 7),
    ] {
        let recipe = all_recipes
            .iter()
            .find(|recipe| recipe.id.as_str() == format!("minecraft:suspicious_stew_from_{flower}"))
            .unwrap();
        let mut grid = std::array::from_fn(|_| ItemStack::EMPTY);
        for (slot, id) in [1, 2, 3, flower_id].into_iter().enumerate() {
            grid[slot] = ItemStack::new(id, 1);
            state.inventory.slots[9 + slot] = grid[slot].clone();
        }
        let manual = mc_data::recipes::crafting_result_from_input(
            &items,
            &state.item_facts,
            &state.tags,
            &all_recipes,
            &grid,
        );
        let (inventory, _) = crate::play::recipes::craft_recipe(&state, recipe, false).unwrap();
        let book_result = inventory
            .slots
            .iter()
            .find(|stack| stack.item_id == 6 && !stack.is_empty())
            .unwrap();
        assert_eq!(book_result, &manual);
        assert_eq!(manual.stew_effects[0].id.as_str(), expected_effect);
        assert_eq!(manual.stew_effects[0].duration, duration);
        let packet = ClientboundContainerSetSlot {
            container_id: 0,
            state_id: 1,
            slot: 36,
            item_stack: manual.clone(),
        };
        let mut wire = Vec::new();
        packet.encode(&mut wire).unwrap();
        let decoded = ClientboundContainerSetSlot::decode(&mut wire.as_slice()).unwrap();
        assert_eq!(decoded.item_stack, manual);
        let book = crate::play::recipes::initial_recipe_book(std::slice::from_ref(recipe), &items);
        let mut book_wire = Vec::new();
        book.encode(&mut book_wire).unwrap();
        let decoded_book =
            mc_protocol::packets::play::ClientboundRecipeBookAdd::decode(&mut book_wire.as_slice())
                .unwrap();
        let mc_protocol::packets::play::RecipeBookDisplay::Shapeless { result, .. } =
            &decoded_book.entries[0].display
        else {
            panic!("shapeless stew recipe");
        };
        assert_eq!(
            result,
            &mc_protocol::packets::play::RecipeBookSlotDisplay::ItemStack(manual.clone())
        );
        crafted.push(manual);
    }
    assert!(!mc_data::inventory_semantics_26_1_2::can_stack(
        &crafted[0],
        &crafted[1]
    ));

    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("StewRecipe"),
        name: "StewRecipe".into(),
    };
    let root = tempfile::tempdir().unwrap();
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory.slots[PlayerInventory::HOTBAR_BASE] = crafted[0].clone();
    saved.survival.food = 1;
    save_player_state(root.path(), profile.uuid, &items, &saved).unwrap();
    let loaded = load_player_state(
        root.path(),
        profile.uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();
    let held = loaded.inventory.slots[PlayerInventory::HOTBAR_BASE].clone();
    assert_eq!(held, crafted[0]);
    let dropped = crate::play::survival::entity_item_stack(held.clone());
    let mc_nbt::Tag::Compound(tag) =
        crate::play::persistence::entity_item_stack_tag(&items, &dropped).unwrap()
    else {
        panic!("item compound");
    };
    assert_eq!(
        crate::play::persistence::read_entity_item_stack(&tag, &items)
            .unwrap()
            .unwrap(),
        dropped
    );

    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let (session_id, _) =
        state
            .sessions
            .register(&profile, (0, 0), 0, HashSet::from([(0, 0)]), tx, pose);
    let survival = loaded.survival;
    let shared = Arc::new(std::sync::Mutex::new(loaded));
    state
        .sessions
        .register_player_persistence(session_id, Arc::clone(&shared));
    state.sessions.mark_loaded(session_id, (0, 0));
    let authority = crate::play::simulation::SimulationAuthority::for_test();
    let committed = state
        .sessions
        .commit_food_use(
            &authority,
            session_id,
            &FoodUsePlan {
                held_slot: PlayerInventory::HOTBAR_BASE,
                expected_held: held,
                expected_survival: survival,
                food: 6,
                saturation: 7.2,
                can_always_eat: true,
                remainder: Some(crate::play::simulation::FoodUseRemainder {
                    stack: ItemStack::new(1, 1),
                    max_stack: 64,
                    entity_type_id: None,
                }),
            },
        )
        .unwrap();
    assert_eq!(
        committed.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(1, 1)
    );
    for tick in 1..=21 {
        state.sessions.tick_player_effects_owned(&authority, tick);
    }
    assert_eq!(shared.lock().unwrap().survival.health, 19.0);
}
