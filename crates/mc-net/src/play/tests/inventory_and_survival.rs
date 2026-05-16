#[test]
fn inventory_merge_prefers_existing_stacks_then_empty_slots() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[10] = ItemStack::new(42, 63);

    let (remaining, changed) = inventory.merge_stack(ItemStack::new(42, 3), 64);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[10], ItemStack::new(42, 64));
    assert_eq!(inventory.slots[9], ItemStack::new(42, 2));
    assert_eq!(changed.len(), 2);
}

#[test]
fn inventory_merge_keeps_different_damage_components_separate() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[10] = ItemStack::new(42, 1).with_damage(1);

    let (remaining, changed) = inventory.merge_stack(ItemStack::new(42, 1).with_damage(2), 1);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[10], ItemStack::new(42, 1).with_damage(1));
    assert_eq!(inventory.slots[9], ItemStack::new(42, 1).with_damage(2));
    assert_eq!(changed, vec![(9, ItemStack::new(42, 1).with_damage(2))]);
}

#[test]
fn pickup_merge_prefers_hotbar_for_new_stacks() {
    let mut inventory = PlayerInventory::empty();

    let (remaining, changed) = inventory.merge_pickup_stack(ItemStack::new(42, 3), 64);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[36], ItemStack::new(42, 3));
    assert_eq!(changed, vec![(36, ItemStack::new(42, 3))]);
}

#[test]
fn pickup_merge_prefers_existing_stacks_before_empty_hotbar() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[10] = ItemStack::new(42, 63);

    let (remaining, changed) = inventory.merge_pickup_stack(ItemStack::new(42, 3), 64);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[10], ItemStack::new(42, 64));
    assert_eq!(inventory.slots[36], ItemStack::new(42, 2));
    assert_eq!(changed.len(), 2);
}

#[test]
fn click_swap_button_maps_hotbar_and_offhand_slots() {
    assert_eq!(hotbar_swap_slot(0), Some(36));
    assert_eq!(hotbar_swap_slot(8), Some(44));
    assert_eq!(hotbar_swap_slot(9), None);
    assert_eq!(player_swap_slot(40), Some(45));
}

#[test]
fn throw_click_takes_one_or_full_stack() {
    let mut stack = ItemStack::new(42, 3);
    assert_eq!(take_throw_stack(&mut stack, 0), Some(ItemStack::new(42, 1)));
    assert_eq!(stack, ItemStack::new(42, 2));
    assert_eq!(take_throw_stack(&mut stack, 1), Some(ItemStack::new(42, 2)));
    assert!(stack.is_empty());
    assert_eq!(take_throw_stack(&mut stack, 0), None);
}

#[test]
fn death_drops_inventory_and_carried_item() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[0] = ItemStack::new(1, 1);
    inventory.slots[5] = ItemStack::new(2, 1);
    inventory.slots[36] = ItemStack::new(3, 4);
    inventory.slots[45] = ItemStack::new(4, 1);
    let mut carried = ItemStack::new(5, 2);

    let drops = take_death_inventory_drops(&mut inventory, &mut carried);

    assert_eq!(
        drops,
        vec![
            ItemStack::new(2, 1),
            ItemStack::new(3, 4),
            ItemStack::new(4, 1),
            ItemStack::new(5, 2),
        ]
    );
    assert_eq!(inventory.slots[0], ItemStack::new(1, 1));
    assert!(inventory.slots[1..].iter().all(ItemStack::is_empty));
    assert!(carried.is_empty());
}

#[test]
fn double_chest_view_uses_verified_nine_by_six_menu_and_split_storage() {
    let left = mc_world::BlockPos { x: 0, y: 64, z: 0 };
    let right = mc_world::BlockPos { x: 1, y: 64, z: 0 };
    let window = ChestWindow::new(vec![right, left], 7);
    assert_eq!(window.positions, vec![left, right]);
    assert_eq!(window.menu_type(), DOUBLE_CHEST_MENU_TYPE_ID);

    let mut inventory = PlayerInventory::empty();
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default(), ChestBlockEntity::default()],
    };
    assert!(set_chest_menu_stack(
        &mut view,
        &mut inventory,
        0,
        ItemStack::new(1, 2),
    ));
    assert!(set_chest_menu_stack(
        &mut view,
        &mut inventory,
        27,
        ItemStack::new(2, 3),
    ));
    assert!(set_chest_menu_stack(
        &mut view,
        &mut inventory,
        54,
        ItemStack::new(3, 4),
    ));

    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(1, 2)
    );
    assert_eq!(
        furnace_slot_to_stack(&view.chests[1].slots[0]),
        ItemStack::new(2, 3)
    );
    assert_eq!(inventory.slots[9], ItemStack::new(3, 4));
    assert_eq!(chest_player_slot(54, 54), Some(9));
    assert_eq!(chest_player_slot(54, 80), Some(35));
    assert_eq!(chest_player_slot(54, 81), Some(36));
    assert_eq!(chest_player_slot(54, 89), Some(44));
    assert_eq!(chest_wire_items(&view, &inventory).len(), 90);
}

#[test]
fn tool_damage_limits_cover_common_fallback_tools() {
    assert_eq!(max_tool_damage_for_path("wooden_pickaxe"), Some(59));
    assert_eq!(max_tool_damage_for_path("stone_axe"), Some(131));
    assert_eq!(max_tool_damage_for_path("iron_shovel"), Some(250));
    assert_eq!(max_tool_damage_for_path("diamond_sword"), Some(1561));
    assert_eq!(max_tool_damage_for_path("golden_hoe"), Some(32));
    assert_eq!(max_tool_damage_for_path("netherite_pickaxe"), Some(2031));
    assert_eq!(max_tool_damage_for_path("apple"), None);
}

#[test]
fn armor_material_rules_match_local_vanilla_basics() {
    let armor = mc_data::armor::builtin();
    let iron_chestplate = armor
        .entry(&mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap())
        .unwrap();
    assert_eq!(iron_chestplate.slot, mc_data::armor::ArmorSlot::Chest);
    assert_eq!(armor_slot_for_kind(iron_chestplate.slot), 6);
    assert_eq!(iron_chestplate.armor, 6.0);
    assert_eq!(iron_chestplate.toughness, 0.0);
    assert_eq!(iron_chestplate.max_damage, 240);

    let diamond_leggings = armor
        .entry(&mc_data::Identifier::parse("minecraft:diamond_leggings").unwrap())
        .unwrap();
    assert_eq!(diamond_leggings.armor, 6.0);
    assert_eq!(diamond_leggings.toughness, 2.0);
    assert_eq!(
        armor.entry(&mc_data::Identifier::parse("minecraft:apple").unwrap()),
        None
    );
}

#[test]
fn armor_reduction_uses_vanilla_combat_rule_shape() {
    let unarmored = armor_reduced_damage(
        10.0,
        ArmorStats {
            armor: 0.0,
            toughness: 0.0,
        },
    );
    assert!((unarmored - 10.0).abs() < f32::EPSILON);

    let iron_chestplate = armor_reduced_damage(
        10.0,
        ArmorStats {
            armor: 6.0,
            toughness: 0.0,
        },
    );
    assert!((iron_chestplate - 9.52).abs() < 0.001);
}

#[test]
fn survival_periodic_tick_regens_and_starves() {
    let mut fed = SurvivalState::FULL;
    fed.apply_damage(2.0);
    assert!(!fed.tick_health(1));
    assert!(fed.tick_health(4));
    assert_eq!(fed.health, 19.0);
    assert!(fed.food < SurvivalState::MAX_FOOD || fed.saturation < 5.0);

    let mut starving = SurvivalState::FULL;
    starving.food = 0;
    starving.saturation = 0.0;
    assert!(starving.tick_health(4));
    assert_eq!(starving.health, 19.0);
}

#[test]
fn reach_validation_uses_player_eye_position() {
    let pose = PlayerPose::new(0.0, 64.0, 0.0);
    assert!(within_block_reach(
        pose,
        pack_block_pos(0, 64, 2),
        GameMode::Survival
    ));
    assert!(!within_block_reach(
        pose,
        pack_block_pos(0, 64, 8),
        GameMode::Survival
    ));
    assert!(within_entity_reach(
        pose,
        Vec3::new(0.0, 65.0, 2.0),
        GameMode::Survival
    ));
    assert!(!within_entity_reach(
        pose,
        Vec3::new(0.0, 65.0, 8.0),
        GameMode::Survival
    ));
}

#[test]
fn survival_block_drops_come_from_repo_loot_data() {
    let id = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let loot = mc_data::loot::builtin();

    assert_eq!(
        loot.block_drop(&id("minecraft:grass_block")),
        Some(&id("minecraft:dirt"))
    );
    assert_eq!(
        loot.block_drop(&id("minecraft:stone")),
        Some(&id("minecraft:cobblestone"))
    );
    assert_eq!(
        loot.block_drop(&id("minecraft:coal_ore")),
        Some(&id("minecraft:coal"))
    );
    assert_eq!(
        loot.block_drop(&id("minecraft:iron_ore")),
        Some(&id("minecraft:raw_iron"))
    );
    assert_eq!(
        loot.block_drop(&id("minecraft:redstone_ore")),
        Some(&id("minecraft:redstone"))
    );
    assert_eq!(
        loot.block_drop(&id("minecraft:oak_leaves")),
        Some(&id("minecraft:apple"))
    );
    assert_eq!(loot.block_drop(&id("minecraft:oak_log")), None);
}

#[test]
fn mob_drops_come_from_repo_loot_data() {
    let id = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let loot = mc_data::loot::builtin();

    assert_eq!(
        loot.entity_drop(&id("minecraft:cow")),
        Some(&id("minecraft:beef"))
    );
    assert_eq!(
        loot.entity_drop(&id("minecraft:pig")),
        Some(&id("minecraft:porkchop"))
    );
    assert_eq!(
        loot.entity_drop(&id("minecraft:chicken")),
        Some(&id("minecraft:chicken"))
    );
    assert_eq!(
        loot.entity_drop(&id("minecraft:zombie")),
        Some(&id("minecraft:rotten_flesh"))
    );
}

#[test]
fn recipe_ingredient_matching_resolves_item_tags() {
    use mc_data::items::ItemReport;
    use mc_data::recipes::{Ingredient, IngredientAlternative};

    let oak_log = mc_data::Identifier::parse("minecraft:oak_log").unwrap();
    let birch_log = mc_data::Identifier::parse("minecraft:birch_log").unwrap();
    let apple = mc_data::Identifier::parse("minecraft:apple").unwrap();
    let logs = mc_data::Identifier::parse("minecraft:logs").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: oak_log,
            protocol_id: 10,
        },
        ItemReport {
            id: birch_log,
            protocol_id: 11,
        },
        ItemReport {
            id: apple,
            protocol_id: 12,
        },
    ]);
    let tags = TagsData {
        registries: BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:item").unwrap(),
            BTreeMap::from([(logs.clone(), vec![10, 11])]),
        )]),
    };
    let ingredient = Ingredient {
        alternatives: vec![IngredientAlternative::Tag(logs)],
    };

    assert!(ingredient_accepts_item(&items, &tags, 10, &ingredient));
    assert!(ingredient_accepts_item(&items, &tags, 11, &ingredient));
    assert!(!ingredient_accepts_item(&items, &tags, 12, &ingredient));
}

#[test]
fn fallback_recipes_include_tag_driven_survival_basics() {
    let recipes = fallback_crafting_recipes();

    assert_eq!(recipes[0].id.as_str(), "minecraft:torch");
    assert_eq!(recipes[1].id.as_str(), "minecraft:oak_planks");
    assert_eq!(recipes[2].id.as_str(), "minecraft:stick");
    assert_eq!(recipes[3].id.as_str(), "minecraft:crafting_table");
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:wooden_pickaxe")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:stone_pickaxe")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:furnace")
    );
    assert!(recipes.iter().any(|recipe| matches!(
        (&recipe.kind, recipe.result.item.as_str()),
        (
            mc_data::recipes::RecipeKind::Smelting(_),
            "minecraft:iron_ingot"
        )
    )));

    let mc_data::recipes::RecipeKind::Shapeless(oak_planks) = &recipes[1].kind else {
        panic!("expected shapeless oak planks recipe");
    };
    assert_eq!(
        oak_planks.ingredients[0].alternatives[0],
        mc_data::recipes::IngredientAlternative::Tag(
            mc_data::Identifier::parse("minecraft:oak_logs").unwrap()
        )
    );
}

#[test]
fn durability_tool_detection_covers_fallback_tool_families() {
    assert!(is_durability_tool_path("iron_pickaxe"));
    assert!(is_durability_tool_path("wooden_shovel"));
    assert!(is_durability_tool_path("diamond_axe"));
    assert!(is_durability_tool_path("stone_hoe"));
    assert!(is_durability_tool_path("netherite_sword"));
    assert!(!is_durability_tool_path("apple"));
    assert!(!is_durability_tool_path("oak_planks"));
}

#[test]
fn fallback_food_rules_include_common_edibles() {
    let item_facts = ItemFactsTable::default();
    assert_eq!(
        food_rule_for_item(
            &item_facts,
            &mc_data::Identifier::parse("minecraft:apple").unwrap()
        ),
        Some((
            mc_data::food::FoodEntry {
                food: 4,
                saturation: 2.4,
            },
            DEFAULT_FOOD_USE_DURATION,
        ))
    );
    assert_eq!(
        food_rule_for_item(
            &item_facts,
            &mc_data::Identifier::parse("minecraft:dirt").unwrap()
        ),
        None
    );
}

#[test]
fn food_rules_prefer_item_component_facts() {
    let apple = mc_data::Identifier::parse("minecraft:apple").unwrap();
    let item_facts = ItemFactsTable::from_entries([(
        apple.clone(),
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(64),
            max_damage: None,
            food: Some(mc_data::food::FoodEntry {
                food: 7,
                saturation: 1.5,
            }),
            use_duration_ticks: Some(10),
            use_action: Some(mc_data::item_components::UseAction::Eat),
            tool: None,
            equippable_slot: None,
        },
    )]);

    assert_eq!(
        food_rule_for_item(&item_facts, &apple),
        Some((
            mc_data::food::FoodEntry {
                food: 7,
                saturation: 1.5
            },
            Duration::from_millis(500),
        ))
    );
}

#[test]
fn item_max_stack_prefers_component_facts() {
    use mc_data::items::ItemReport;

    let ender_pearl = mc_data::Identifier::parse("minecraft:ender_pearl").unwrap();
    let stone = mc_data::Identifier::parse("minecraft:stone").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: ender_pearl.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: stone.clone(),
            protocol_id: 2,
        },
    ]);
    let facts = ItemFactsTable::from_entries([(
        ender_pearl,
        mc_data::item_components::ItemFacts {
            max_stack_size: Some(16),
            ..Default::default()
        },
    )]);

    assert_eq!(item_max_stack(&facts, &items, &ItemStack::new(1, 1)), 16);
    assert_eq!(item_max_stack(&facts, &items, &ItemStack::new(2, 1)), 64);
    assert_eq!(
        item_max_stack(&facts, &items, &ItemStack::new(1, 1).with_damage(3)),
        1
    );
}

#[test]
fn fallback_mining_rules_use_block_family_and_matching_tool() {
    let stone_hand = fallback_mining_time("stone", None);
    let stone_pickaxe = fallback_mining_time("stone", Some("iron_pickaxe"));
    let stone_shovel = fallback_mining_time("stone", Some("iron_shovel"));

    assert!(stone_pickaxe < stone_hand);
    assert_eq!(stone_shovel, stone_hand);
    assert!(
        fallback_mining_time("oak_log", Some("stone_axe"))
            < fallback_mining_time("oak_log", None)
    );
    assert!(
        fallback_mining_time("dirt", Some("wooden_shovel"))
            < fallback_mining_time("dirt", None)
    );
    assert_eq!(
        fallback_mining_time("podzol", None),
        Duration::from_millis(200)
    );
    assert_eq!(
        fallback_mining_time("unknown_custom_block", None),
        Duration::from_millis(800)
    );
}
