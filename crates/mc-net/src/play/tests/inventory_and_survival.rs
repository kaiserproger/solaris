#[test]
fn item_use_duration_rounds_up_to_simulation_ticks() {
    assert_eq!(item_use_ticks(Duration::from_millis(1_600)), 32);
    assert_eq!(item_use_ticks(Duration::from_millis(1_601)), 33);
}

#[test]
fn pending_item_use_and_bow_power_follow_simulation_tick_delta() {
    let pending = PendingUse {
        started_tick: 10,
        required_ticks: 32,
        held_hotbar_slot: 0,
        held_slot: PlayerInventory::HOTBAR_BASE,
        held_item_id: 42,
        kind: UseKind::Food(mc_data::food::FoodEntry {
            food: 4,
            saturation: 2.4,
        }),
    };

    assert!(!pending_use_is_complete(&pending, 41));
    assert!(pending_use_is_complete(&pending, 42));
    assert_eq!(bow_draw_power(10, 12), 0.0);
    assert!((bow_draw_power(10, 13) - 0.1075).abs() < f64::EPSILON);
    assert!((bow_draw_power(10, 20) - (5.0 / 12.0)).abs() < f64::EPSILON);
    assert_eq!(bow_draw_power(10, 30), 1.0);
}

#[test]
fn arrow_selection_finds_main_inventory() {
    let arrow = Identifier::parse("minecraft:arrow").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: arrow,
        protocol_id: 42,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[10] = ItemStack::new(42, 3);

    assert_eq!(available_arrow_slot(&state), Some(10));
}

#[test]
fn arrow_selection_prefers_held_main_hand_before_other_hotbar_slots() {
    let arrow = Identifier::parse("minecraft:arrow").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: arrow,
        protocol_id: 42,
    }]));
    let mut state = interaction_state_for_items(items);
    state.selected_hotbar_slot = 4;
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(42, 3);
    state.inventory.slots[PlayerInventory::HOTBAR_BASE + 4] = ItemStack::new(42, 3);

    assert_eq!(
        available_arrow_slot(&state),
        Some(PlayerInventory::HOTBAR_BASE + 4)
    );
}

#[test]
fn hotbar_access_accepts_vanilla_endpoint_indices() {
    let mut inventory = PlayerInventory::empty();

    inventory.set_hotbar(0, ItemStack::new(40, 1)).unwrap();
    inventory.set_hotbar(8, ItemStack::new(48, 1)).unwrap();
    inventory.held_mut(0).unwrap().count = 2;
    inventory.held_mut(8).unwrap().count = 3;

    assert_eq!(inventory.held(0), Some(&ItemStack::new(40, 2)));
    assert_eq!(inventory.held(8), Some(&ItemStack::new(48, 3)));
}

#[test]
fn invalid_hotbar_reads_fail_closed() {
    let inventory = PlayerInventory::empty();

    for hotbar_slot in [9, u8::MAX] {
        assert_eq!(inventory.held(hotbar_slot), None);
    }
}

#[test]
fn invalid_hotbar_mutable_access_fails_closed() {
    let mut inventory = PlayerInventory::empty();

    for hotbar_slot in [9, u8::MAX] {
        assert_eq!(inventory.held_mut(hotbar_slot), None);
    }

    assert_eq!(inventory.slots, PlayerInventory::empty().slots);
}

#[test]
fn invalid_hotbar_set_returns_the_rejected_stack() {
    let mut inventory = PlayerInventory::empty();

    for hotbar_slot in [9, u8::MAX] {
        let stack = ItemStack::new(42, 3);
        assert_eq!(inventory.set_hotbar(hotbar_slot, stack.clone()), Err(stack));
    }

    assert_eq!(inventory.slots, PlayerInventory::empty().slots);
}

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
fn inventory_merges_fail_closed_for_non_positive_max_stack() {
    for max_stack in [0, -1] {
        let mut inventory = PlayerInventory::empty();
        inventory.slots[9] = ItemStack::new(42, 1);
        let before = inventory.slots.clone();
        let incoming = ItemStack::new(42, 3);

        let (remaining, changed) = inventory.merge_stack(incoming.clone(), max_stack);

        assert_eq!(remaining, incoming);
        assert!(changed.is_empty());
        assert_eq!(inventory.slots, before);

        let Some((remaining, changed)) =
            inventory.merge_pickup_stack(incoming.clone(), max_stack, 0)
        else {
            panic!("valid selected hotbar slot");
        };

        assert_eq!(remaining, incoming);
        assert!(changed.is_empty());
        assert_eq!(inventory.slots, before);
    }
}

#[test]
fn inventory_merge_canonicalizes_negative_counts_and_splits_oversized_counts() {
    let mut inventory = PlayerInventory::empty();
    let residual_empty = ItemStack::new(42, -1).with_damage(7);

    let (remaining, changed) = inventory.merge_stack(residual_empty.clone(), 64);

    assert_eq!(remaining, ItemStack::EMPTY);
    assert!(changed.is_empty());

    let (remaining, changed) = inventory.merge_pickup_stack(residual_empty, 64, 0).unwrap();

    assert_eq!(remaining, ItemStack::EMPTY);
    assert!(changed.is_empty());

    let incoming = ItemStack::new(42, i32::MAX);
    let (remaining, changed) = inventory.merge_stack(incoming, 64);

    assert_eq!(changed.len(), 36);
    assert!(
        inventory.slots[9..=44]
            .iter()
            .all(|stack| stack == &ItemStack::new(42, 64))
    );
    assert_eq!(remaining, ItemStack::new(42, i32::MAX - 36 * 64));
}

#[test]
fn inventory_merge_handles_overflow_adjacent_counts() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(42, i32::MAX - 1);

    let (remaining, changed) = inventory.merge_stack(ItemStack::new(42, 2), i32::MAX);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[9], ItemStack::new(42, i32::MAX));
    assert_eq!(inventory.slots[10], ItemStack::new(42, 1));
    assert_eq!(
        changed,
        vec![
            (9, ItemStack::new(42, i32::MAX)),
            (10, ItemStack::new(42, 1)),
        ]
    );
}

#[test]
fn range_merge_validates_all_ranges_before_mutating() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[44] = ItemStack::new(42, 63);
    let before = inventory.slots.clone();
    let incoming = ItemStack::new(42, 2);

    let remaining = inventory.merge_stack_into_ranges(incoming.clone(), &[44..=46], 64);

    assert_eq!(remaining, incoming);
    assert_eq!(inventory.slots, before);

    let remaining =
        inventory.merge_stack_into_ranges(incoming.clone(), &[usize::MAX..=usize::MAX], 64);

    assert_eq!(remaining, incoming);
    assert_eq!(inventory.slots, before);
}

#[test]
fn range_merge_fails_closed_for_non_positive_max_stack() {
    for max_stack in [0, -1] {
        let mut inventory = PlayerInventory::empty();
        let incoming = ItemStack::new(42, 2);

        let remaining = inventory.merge_stack_into_ranges(incoming.clone(), &[9..=10], max_stack);

        assert_eq!(remaining, incoming);
        assert_eq!(inventory.slots, PlayerInventory::empty().slots);
    }
}

#[test]
fn range_merge_supports_full_and_no_op_ranges() {
    let mut inventory = PlayerInventory::empty();
    let incoming = ItemStack::new(42, 2);
    let empty_range = std::ops::RangeInclusive::new(10, 9);

    assert_eq!(
        inventory.merge_stack_into_ranges(incoming.clone(), &[], 64),
        incoming
    );
    assert_eq!(
        inventory.merge_stack_into_ranges(incoming.clone(), &[empty_range], 64),
        incoming
    );
    assert_eq!(inventory.slots, PlayerInventory::empty().slots);

    let remaining = inventory.merge_stack_into_ranges(ItemStack::new(42, 46), &[0..=45], 1);

    assert!(remaining.is_empty());
    assert!(
        inventory
            .slots
            .iter()
            .all(|stack| stack == &ItemStack::new(42, 1))
    );
}

#[test]
fn range_merge_leaves_input_unchanged_when_targets_are_full() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(42, 64);
    inventory.slots[10] = ItemStack::new(42, 64);
    let before = inventory.slots.clone();
    let incoming = ItemStack::new(42, 2);

    let remaining = inventory.merge_stack_into_ranges(incoming.clone(), &[9..=10], 64);

    assert_eq!(remaining, incoming);
    assert_eq!(inventory.slots, before);
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
fn inventory_merge_keeps_different_custom_names_separate() {
    let existing = ItemStack::new(42, 1).with_custom_name("Catalog Apple");
    let incoming = ItemStack::new(42, 1).with_custom_name("Reward Apple");
    let mut inventory = PlayerInventory::empty();
    inventory.slots[10] = existing.clone();

    let (remaining, changed) = inventory.merge_stack(incoming.clone(), 64);

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[10], existing);
    assert_eq!(inventory.slots[9], incoming.clone());
    assert_eq!(changed, vec![(9, incoming)]);
}

#[test]
fn inventory_merge_compares_enchantments_in_canonical_order() {
    let sharpness = Identifier::parse("minecraft:sharpness").unwrap();
    let unbreaking = Identifier::parse("minecraft:unbreaking").unwrap();
    let existing = ItemStack::new(42, 1)
        .with_enchantment(sharpness.clone(), 2)
        .with_enchantment(unbreaking.clone(), 3);
    let mut reordered = existing.clone();
    reordered.enchantments.reverse();

    let mut inventory = PlayerInventory::empty();
    inventory.slots[10] = existing.clone();
    let (remaining, changed) = inventory.merge_stack(reordered.clone(), 64);
    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[10].count, 2);
    assert_eq!(changed, vec![(10, inventory.slots[10].clone())]);

    let differently_enchanted = ItemStack::new(42, 1).with_enchantment(sharpness.clone(), 3);
    let mut inventory = PlayerInventory::empty();
    inventory.slots[36] = existing.clone();
    let (remaining, changed) = inventory
        .merge_pickup_stack(differently_enchanted.clone(), 64, 0)
        .unwrap();
    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[36], existing);
    assert_eq!(inventory.slots[37], differently_enchanted);
    assert_eq!(changed, vec![(37, inventory.slots[37].clone())]);

    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(42, 1).with_enchantment(unbreaking, 2);
    let incoming = ItemStack::new(42, 1).with_enchantment(sharpness, 2);
    assert!(
        inventory
            .merge_stack_into_ranges(incoming.clone(), &[9..=10], 64)
            .is_empty()
    );
    assert_eq!(inventory.slots[9].count, 1);
    assert_eq!(inventory.slots[10], incoming);
}

#[test]
fn pickup_merge_prefers_selected_hotbar_for_new_stacks() {
    let mut inventory = PlayerInventory::empty();

    let (remaining, changed) = inventory
        .merge_pickup_stack(ItemStack::new(42, 3), 64, 4)
        .unwrap();

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[40], ItemStack::new(42, 3));
    assert_eq!(changed, vec![(40, ItemStack::new(42, 3))]);
}

#[test]
fn pickup_merge_uses_selected_offhand_hotbar_then_main_order() {
    let mut inventory = PlayerInventory::empty();
    for slot in [40, 45, 36, 9] {
        inventory.slots[slot] = ItemStack::new(42, 63);
    }

    let (remaining, changed) = inventory
        .merge_pickup_stack(ItemStack::new(42, 4), 64, 4)
        .unwrap();

    assert!(remaining.is_empty());
    assert_eq!(
        changed.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
        vec![40, 45, 36, 9]
    );
    assert!(changed.iter().all(|(_, stack)| stack.count == 64));
}

#[test]
fn pickup_merge_uses_compatible_offhand_when_inventory_is_full() {
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(99, 64);
    }
    inventory.slots[45] = ItemStack::new(42, 63);

    let (remaining, changed) = inventory
        .merge_pickup_stack(ItemStack::new(42, 1), 64, 4)
        .unwrap();

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[45], ItemStack::new(42, 64));
    assert_eq!(changed, vec![(45, ItemStack::new(42, 64))]);
}

#[test]
fn pickup_merge_does_not_place_new_stack_in_empty_offhand() {
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(99, 64);
    }
    let before = inventory.slots.clone();
    let incoming = ItemStack::new(42, 1);

    let (remaining, changed) = inventory
        .merge_pickup_stack(incoming.clone(), 64, 4)
        .unwrap();

    assert_eq!(remaining, incoming);
    assert!(changed.is_empty());
    assert_eq!(inventory.slots, before);
}

#[test]
fn pickup_merge_credits_exact_partial_capacity() {
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(99, 64);
    }
    inventory.slots[36] = ItemStack::new(42, 63);
    let incoming = ItemStack::new(42, 2);

    let (remaining, changed) = inventory
        .merge_pickup_stack(incoming.clone(), 64, 4)
        .unwrap();

    assert_eq!(remaining, ItemStack::new(42, 1));
    assert_eq!(changed, vec![(36, ItemStack::new(42, 64))]);
    assert_eq!(inventory.slots[36], ItemStack::new(42, 64));
}

#[test]
fn pickup_merge_rejects_invalid_selected_hotbar_before_mutation() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[36] = ItemStack::new(42, 63);
    let before = inventory.slots.clone();

    assert_eq!(
        inventory.merge_pickup_stack(ItemStack::new(42, 1), 64, 9),
        None
    );
    assert_eq!(inventory.slots, before);
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
fn empty_stack_mutations_drop_residual_payload() {
    let residual_empty = || ItemStack::new(42, 0).with_damage(7);
    let mut inventory = PlayerInventory::empty();

    inventory.set_hotbar(0, residual_empty()).unwrap();
    assert_eq!(
        inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );

    inventory.slots[PlayerInventory::HOTBAR_BASE] = residual_empty();
    assert_eq!(inventory.held(0), Some(&ItemStack::EMPTY));
    assert_eq!(
        inventory.held_mut(0).map(|stack| &*stack),
        Some(&ItemStack::EMPTY)
    );
    assert_eq!(
        inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );

    inventory.slots[9] = residual_empty();
    assert_eq!(inventory.as_wire_list()[9], ItemStack::EMPTY);

    let mut slot = residual_empty();
    assert_eq!(inventory::take_from_slot(&mut slot, 1), ItemStack::EMPTY);
    assert_eq!(slot, ItemStack::EMPTY);

    let mut cursor = ItemStack::new(42, i32::MIN).with_damage(7);
    inventory::decrement_cursor(&mut cursor);
    assert_eq!(cursor, ItemStack::EMPTY);

    let mut thrown = residual_empty();
    assert_eq!(take_throw_stack(&mut thrown, 0), None);
    assert_eq!(thrown, ItemStack::EMPTY);

    let mut outside = residual_empty();
    assert_eq!(inventory::apply_outside_pickup_click(&mut outside, 0), None);
    assert_eq!(outside, ItemStack::EMPTY);
}

#[test]
fn regular_pickup_fails_closed_for_invalid_max_stack_and_canonicalizes_empty_cursor() {
    for max_stack in [0, -1] {
        let mut cursor = ItemStack::new(42, 2);

        assert_eq!(
            apply_regular_pickup_slot(&mut cursor, ItemStack::new(42, 3), 0, max_stack, true,),
            None
        );
        assert_eq!(cursor, ItemStack::new(42, 2));
    }

    let mut cursor = ItemStack::new(42, 0).with_damage(7);
    assert_eq!(
        apply_regular_pickup_slot(&mut cursor, ItemStack::EMPTY, 0, 64, true),
        None
    );
    assert_eq!(cursor, ItemStack::EMPTY);
}

#[test]
fn right_click_split_handles_maximum_count_without_overflow() {
    let mut cursor = ItemStack::EMPTY;

    let remaining =
        apply_regular_pickup_slot(&mut cursor, ItemStack::new(42, i32::MAX), 1, i32::MAX, true);

    assert_eq!(cursor, ItemStack::new(42, i32::MAX / 2 + 1));
    assert_eq!(remaining, Some(ItemStack::new(42, i32::MAX / 2)));
}

#[test]
fn regular_pickup_slot_merges_cursor_into_clicked_stack() {
    let mut cursor = ItemStack::new(42, 3);

    let slot = apply_regular_pickup_slot(&mut cursor, ItemStack::new(42, 62), 0, 64, true);

    assert_eq!(slot, Some(ItemStack::new(42, 64)));
    assert_eq!(cursor, ItemStack::new(42, 1));
}

#[test]
fn regular_pickup_slot_respects_menu_placement_rules() {
    let mut cursor = ItemStack::new(42, 3);

    let slot = apply_regular_pickup_slot(&mut cursor, ItemStack::EMPTY, 0, 64, false);

    assert_eq!(slot, None);
    assert_eq!(cursor, ItemStack::new(42, 3));
}

#[test]
fn regular_swap_slot_requires_both_destinations_to_accept_stacks() {
    let clicked = ItemStack::new(1, 1);
    let swap = ItemStack::new(2, 1);

    assert_eq!(
        apply_regular_swap_slot(clicked.clone(), swap.clone(), true, false),
        None
    );
    assert_eq!(
        apply_regular_swap_slot(clicked, swap, true, true),
        Some((ItemStack::new(2, 1), ItemStack::new(1, 1)))
    );
}

#[test]
fn regular_throw_slot_returns_remaining_stack_and_drop() {
    let (remaining, dropped) = apply_regular_throw_slot(ItemStack::new(42, 3), 0).unwrap();

    assert_eq!(remaining, ItemStack::new(42, 2));
    assert_eq!(dropped, ItemStack::new(42, 1));
}

#[test]
fn quick_move_crafting_result_is_transactional_when_inventory_has_partial_room() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    state.inventory.slots[0] = ItemStack::new(42, 4);
    state.inventory.slots[1] = ItemStack::new(7, 1);
    state.inventory.slots[9] = ItemStack::new(42, 62);
    for slot in 10..=44 {
        state.inventory.slots[slot] = ItemStack::new(99, 64);
    }

    assert!(!apply_quick_move_click(&mut state, 0));

    assert_eq!(state.inventory.slots[0], ItemStack::new(42, 4));
    assert_eq!(state.inventory.slots[1], ItemStack::new(7, 1));
    assert_eq!(state.inventory.slots[9], ItemStack::new(42, 62));
}

#[test]
fn outside_pickup_click_drops_from_cursor() {
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::from_report(&[])));
    state.carried_item = ItemStack::new(42, 3);

    let dropped = apply_outside_pickup_click(&mut state, 0);

    assert_eq!(dropped, Some(ItemStack::new(42, 3)));
    assert!(state.carried_item.is_empty());

    state.carried_item = ItemStack::new(42, 3);
    let dropped = apply_outside_pickup_click(&mut state, 1);

    assert_eq!(dropped, Some(ItemStack::new(42, 1)));
    assert_eq!(state.carried_item, ItemStack::new(42, 2));
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
fn dropped_item_entity_stack_preserves_damage_component() {
    let stack = entity_item_stack(ItemStack::new(42, 1).with_damage(19));

    assert_eq!(stack, EntityItemStack::new(42, 1).with_damage(19));
}

#[test]
fn block_break_denylist_blocks_unbreakable_vanilla_blocks() {
    let blocks = BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        simple_block(2, "minecraft:bedrock"),
        simple_block(3, "minecraft:barrier"),
        simple_block(4, "minecraft:end_portal_frame"),
    ])
    .unwrap();

    assert!(!block_break_is_denied(&blocks, BlockStateId(0)));
    assert!(!block_break_is_denied(&blocks, BlockStateId(1)));
    assert!(block_break_is_denied(&blocks, BlockStateId(2)));
    assert!(block_break_is_denied(&blocks, BlockStateId(3)));
    assert!(block_break_is_denied(&blocks, BlockStateId(4)));
    assert!(!block_break_is_denied(&blocks, BlockStateId(99)));
}

#[test]
fn pickup_merge_keeps_damaged_items_separate() {
    let mut inventory = PlayerInventory::empty();
    inventory.slots[36] = ItemStack::new(42, 1).with_damage(1);

    let (remaining, changed) = inventory
        .merge_pickup_stack(ItemStack::new(42, 1).with_damage(2), 1, 0)
        .unwrap();

    assert!(remaining.is_empty());
    assert_eq!(inventory.slots[36], ItemStack::new(42, 1).with_damage(1));
    assert_eq!(inventory.slots[37], ItemStack::new(42, 1).with_damage(2));
    assert_eq!(changed, vec![(37, ItemStack::new(42, 1).with_damage(2))]);
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
fn armor_slot_placement_uses_equippable_component_facts() {
    use mc_data::item_components::ItemFacts;
    use mc_data::items::ItemReport;

    let elytra = Identifier::parse("minecraft:elytra").unwrap();
    let carved_pumpkin = Identifier::parse("minecraft:carved_pumpkin").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: elytra.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: carved_pumpkin.clone(),
            protocol_id: 2,
        },
    ]);
    let item_facts = ItemFactsTable::from_entries([
        (
            elytra,
            ItemFacts {
                equippable_slot: Some("chest".to_string()),
                ..Default::default()
            },
        ),
        (
            carved_pumpkin,
            ItemFacts {
                equippable_slot: Some("head".to_string()),
                ..Default::default()
            },
        ),
    ]);

    assert!(can_place_in_player_slot(
        &item_facts,
        &items,
        6,
        &ItemStack::new(1, 1)
    ));
    assert!(can_place_in_player_slot(
        &item_facts,
        &items,
        5,
        &ItemStack::new(2, 1)
    ));
    assert!(!can_place_in_player_slot(
        &item_facts,
        &items,
        5,
        &ItemStack::new(1, 1)
    ));
    assert!(!can_place_in_player_slot(
        &item_facts,
        &items,
        6,
        &ItemStack::new(2, 1)
    ));
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
fn protection_reduction_uses_vanilla_magic_absorb_formula_and_cap() {
    assert!((protection_reduced_damage(10.0, 0) - 10.0).abs() < f32::EPSILON);
    assert!((protection_reduced_damage(10.0, 3) - 8.8).abs() < 0.001);
    assert!((protection_reduced_damage(10.0, 24) - 2.0).abs() < 0.001);
}

#[test]
fn typed_player_damage_applies_only_the_vanilla_reductions() {
    let chestplate = Identifier::parse("minecraft:iron_chestplate").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chestplate,
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[6] = ItemStack::new(11, 1)
        .with_enchantment(Identifier::parse("minecraft:protection").unwrap(), 3);

    for kind in [
        PlayerDamageKind::MobAttack,
        PlayerDamageKind::PlayerAttack,
        PlayerDamageKind::Projectile,
        PlayerDamageKind::Campfire,
        PlayerDamageKind::Fire,
        PlayerDamageKind::Lava,
        PlayerDamageKind::Explosion,
    ] {
        let reduced = survival_damage_after_equipment(Some(&state), 10.0, kind);
        assert!((reduced - 8.3776).abs() < 0.001, "{kind:?}");
    }
    for kind in [
        PlayerDamageKind::Fall,
        PlayerDamageKind::Drowning,
        PlayerDamageKind::Suffocation,
        PlayerDamageKind::Starvation,
        PlayerDamageKind::Generic,
        PlayerDamageKind::GenericKill,
    ] {
        let reduced = survival_damage_after_equipment(Some(&state), 10.0, kind);
        assert!((reduced - 8.8).abs() < 0.001, "{kind:?}");
    }
    assert_eq!(
        survival_damage_after_equipment(Some(&state), f32::NAN, PlayerDamageKind::Generic),
        0.0
    );
    assert_eq!(
        survival_damage_after_equipment(Some(&state), 10.0, PlayerDamageKind::Unsupported),
        0.0
    );
}

#[test]
fn armor_durability_scales_with_incoming_damage() {
    let chestplate = Identifier::parse("minecraft:iron_chestplate").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chestplate,
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.inventory.slots[6] = ItemStack::new(11, 1);

    let changed = damage_equipped_armor(&mut state, 9.0);

    assert_eq!(changed, vec![(6, ItemStack::new(11, 1).with_damage(2))]);
    assert_eq!(
        state.inventory.slots[6],
        ItemStack::new(11, 1).with_damage(2)
    );
}

#[test]
fn full_iron_armor_set_reduces_damage_and_loses_durability() {
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:iron_helmet").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: Identifier::parse("minecraft:iron_chestplate").unwrap(),
            protocol_id: 12,
        },
        ItemReport {
            id: Identifier::parse("minecraft:iron_leggings").unwrap(),
            protocol_id: 13,
        },
        ItemReport {
            id: Identifier::parse("minecraft:iron_boots").unwrap(),
            protocol_id: 14,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[5] = ItemStack::new(11, 1);
    state.inventory.slots[6] = ItemStack::new(12, 1);
    state.inventory.slots[7] = ItemStack::new(13, 1);
    state.inventory.slots[8] = ItemStack::new(14, 1);

    let reduced = survival_damage_after_armor(Some(&state), 10.0);
    assert!((reduced - 6.0).abs() < 0.001);

    let changed = damage_equipped_armor(&mut state, 9.0);
    assert_eq!(
        changed,
        vec![
            (5, ItemStack::new(11, 1).with_damage(2)),
            (6, ItemStack::new(12, 1).with_damage(2)),
            (7, ItemStack::new(13, 1).with_damage(2)),
            (8, ItemStack::new(14, 1).with_damage(2)),
        ]
    );
}

#[test]
fn full_diamond_armor_set_reduces_damage_and_loses_durability() {
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:diamond_helmet").unwrap(),
            protocol_id: 21,
        },
        ItemReport {
            id: Identifier::parse("minecraft:diamond_chestplate").unwrap(),
            protocol_id: 22,
        },
        ItemReport {
            id: Identifier::parse("minecraft:diamond_leggings").unwrap(),
            protocol_id: 23,
        },
        ItemReport {
            id: Identifier::parse("minecraft:diamond_boots").unwrap(),
            protocol_id: 24,
        },
    ]));
    let mut state = interaction_state_for_items(items);
    state.inventory.slots[5] = ItemStack::new(21, 1);
    state.inventory.slots[6] = ItemStack::new(22, 1);
    state.inventory.slots[7] = ItemStack::new(23, 1);
    state.inventory.slots[8] = ItemStack::new(24, 1);

    let reduced = survival_damage_after_armor(Some(&state), 10.0);
    assert!((reduced - 3.0).abs() < 0.001);

    let changed = damage_equipped_armor(&mut state, 9.0);
    assert_eq!(
        changed,
        vec![
            (5, ItemStack::new(21, 1).with_damage(2)),
            (6, ItemStack::new(22, 1).with_damage(2)),
            (7, ItemStack::new(23, 1).with_damage(2)),
            (8, ItemStack::new(24, 1).with_damage(2)),
        ]
    );
}

#[tokio::test]
async fn projectile_damage_uses_armor_protection_and_damages_armor() {
    let chestplate = Identifier::parse("minecraft:iron_chestplate").unwrap();
    let protection = Identifier::parse("minecraft:protection").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: chestplate,
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.inventory.slots[6] = ItemStack::new(11, 1).with_enchantment(protection.clone(), 3);
    let mut survival_state = SurvivalState::FULL;
    let mut xp_state = XpState::default();
    let (simulation_stop, simulation_task) =
        start_survival_test_owner(&mut state, "ProjectileArmor", survival_state, &xp_state);
    let mut writer = Vec::new();

    apply_player_damage(
        Some(&mut state),
        &mut writer,
        Compression::Disabled,
        &mut survival_state,
        &mut xp_state,
        GameMode::Survival,
        PlayerDamageApplication {
            player_pose: PlayerPose::new(0.0, 64.0, 0.0),
            request: PlayerDamageRequest {
                kind: PlayerDamageKind::Projectile,
                amount: 10.0,
                source_origin: Some(Vec3::new(0.0, 64.0, 2.0)),
            },
        },
    )
    .await
    .unwrap();
    simulation_stop.send(()).unwrap();
    simulation_task.await.unwrap();

    assert!((survival_state.health - 11.6224).abs() < 0.001);
    assert_eq!(
        state.inventory.slots[6],
        ItemStack::new(11, 1)
            .with_enchantment(protection, 3)
            .with_damage(2)
    );
}

#[tokio::test]
async fn bow_melee_attack_does_not_damage_bow() {
    let bow = Identifier::parse("minecraft:bow").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: bow,
        protocol_id: 12,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(12, 1);
    let mut writer = Vec::new();

    damage_held_weapon_after_attack(&mut state, &mut writer)
        .await
        .unwrap();

    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(12, 1)
    );
}

#[test]
fn shield_durability_respects_three_damage_threshold() {
    assert_eq!(shield_durability_damage(2.99), 0);
    assert_eq!(shield_durability_damage(3.0), 4);

    let mut state = shield_item_state();
    let shield = ItemStack::new(77, 1);
    state.inventory.slots[45] = shield.clone();
    state.shield_use = shield_use_from_stack(
        InteractionHand::OffHand,
        45,
        shield,
        state.sessions.world_time(),
        true,
    );

    assert_eq!(damage_active_shield(&mut state, 2.99), None);
    assert_eq!(state.inventory.slots[45], ItemStack::new(77, 1));
    assert_eq!(
        damage_active_shield(&mut state, 3.0),
        Some((45, ItemStack::new(77, 1).with_damage(4)))
    );
}

#[test]
fn shield_blocks_only_after_delay_and_inside_front_hemisphere() {
    let mut state = shield_item_state();
    let shield = ItemStack::new(77, 1);
    state.inventory.slots[45] = shield.clone();
    state.shield_use = shield_use_from_stack(InteractionHand::OffHand, 45, shield, 10, true);
    state
        .sessions
        .set_world_time(10 + SHIELD_ACTIVATION_DELAY_TICKS - 1);
    let pose = PlayerPose::new(0.0, 64.0, 0.0);

    assert!(!shield_blocks_current_damage(
        &mut state,
        pose,
        Some(Vec3::new(0.0, 64.0, 2.0))
    ));

    state
        .sessions
        .set_world_time(10 + SHIELD_ACTIVATION_DELAY_TICKS);
    assert!(shield_blocks_current_damage(
        &mut state,
        pose,
        Some(Vec3::new(0.0, 64.0, 2.0))
    ));
    assert!(shield_blocks_current_damage(
        &mut state,
        pose,
        Some(Vec3::new(3.464_101_615_137_754_4, 64.0, 2.0))
    ));
    assert!(shield_blocks_current_damage(
        &mut state,
        pose,
        Some(Vec3::new(2.0, 64.0, 0.0))
    ));
    assert!(!shield_blocks_current_damage(
        &mut state,
        pose,
        Some(Vec3::new(0.0, 64.0, -2.0))
    ));
}

#[test]
fn shield_blocking_clears_stale_active_shield() {
    let mut state = shield_item_state();
    let shield = ItemStack::new(77, 1);
    state.inventory.slots[45] = shield.clone();
    state.shield_use = shield_use_from_stack(InteractionHand::OffHand, 45, shield, 0, true);
    state.sessions.set_world_time(SHIELD_ACTIVATION_DELAY_TICKS);
    state.inventory.slots[45] = ItemStack::EMPTY;

    assert!(!shield_blocks_current_damage(
        &mut state,
        PlayerPose::new(0.0, 64.0, 0.0),
        Some(Vec3::new(0.0, 64.0, 2.0))
    ));
    assert!(state.shield_use.is_none());
}

#[test]
fn weapon_attack_durability_is_survival_only() {
    assert!(weapon_attacks_damage_held_item(GameMode::Survival));
    assert!(!weapon_attacks_damage_held_item(GameMode::Creative));
    assert!(!weapon_attacks_damage_held_item(GameMode::Adventure));
    assert!(!weapon_attacks_damage_held_item(GameMode::Spectator));
}

#[test]
fn survival_periodic_tick_regens_and_starves() {
    let mut saturated = SurvivalState::FULL;
    saturated.apply_damage(2.0);
    let mut saturated_timer = 0;
    for _ in 0..9 {
        assert_eq!(
            saturated.tick_health(&mut saturated_timer),
            SurvivalHealthTick::Unchanged
        );
    }
    assert_eq!(
        saturated.tick_health(&mut saturated_timer),
        SurvivalHealthTick::Changed
    );
    assert!((saturated.health - (18.0 + 5.0 / 6.0)).abs() < 0.001);
    assert!(saturated.saturation < 5.0);

    let mut fed = SurvivalState {
        health: 18.0,
        food: 18,
        saturation: 0.0,
        exhaustion: 0.0,
    };
    let mut fed_timer = 0;
    for _ in 0..79 {
        assert_eq!(
            fed.tick_health(&mut fed_timer),
            SurvivalHealthTick::Unchanged
        );
    }
    assert_eq!(fed.tick_health(&mut fed_timer), SurvivalHealthTick::Changed);
    assert_eq!(fed.health, 19.0);

    let mut starving = SurvivalState::FULL;
    starving.food = 0;
    starving.saturation = 0.0;
    let mut starving_timer = 0;
    for _ in 0..79 {
        assert_eq!(
            starving.tick_health(&mut starving_timer),
            SurvivalHealthTick::Unchanged
        );
    }
    let SurvivalHealthTick::StarvationDamage(damage) = starving.tick_health(&mut starving_timer)
    else {
        panic!("starvation timer must request damage");
    };
    starving.apply_damage(damage);
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
        Vec3::new(0.0, 64.0, 5.0),
        entity_aabb("minecraft:zombie"),
        GameMode::Survival
    ));
    assert!(!within_entity_reach(
        pose,
        Vec3::new(0.0, 65.0, 8.0),
        entity_aabb("minecraft:chicken"),
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
    assert_eq!(
        loot.block_drop(&id("minecraft:oak_log")),
        Some(&id("minecraft:oak_log"))
    );
}

#[test]
fn mob_drops_come_from_repo_loot_data() {
    let id = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let loot = mc_data::loot::builtin();

    assert_eq!(
        loot.entity_drop_stacks(&id("minecraft:cow"))
            .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
        Some(vec![&id("minecraft:leather"), &id("minecraft:beef")])
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
    let tags = TagsData::from_registries(BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:item").unwrap(),
            BTreeMap::from([(logs.clone(), vec![10, 11])]),
        )]));
    let ingredient = Ingredient {
        alternatives: vec![IngredientAlternative::Tag(logs)],
    };

    assert!(ingredient_accepts_item(&items, &tags, 10, &ingredient));
    assert!(ingredient_accepts_item(&items, &tags, 11, &ingredient));
    assert!(!ingredient_accepts_item(&items, &tags, 12, &ingredient));
}

#[test]
fn solaris_required_recipes_include_tag_driven_survival_basics() {
    let recipes = mc_data::recipes::solaris_required_recipes();

    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:torch")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:birch_planks")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:stick")
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:crafting_table")
    );
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

    let oak_recipe = recipes
        .iter()
        .find(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        .expect("oak planks recipe present");
    let mc_data::recipes::RecipeKind::Shapeless(oak_planks) = &oak_recipe.kind else {
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
fn inventory_crafting_grid_crafts_birch_logs_to_planks() {
    use mc_data::items::ItemReport;

    let birch_log = mc_data::Identifier::parse("minecraft:birch_log").unwrap();
    let birch_planks = mc_data::Identifier::parse("minecraft:birch_planks").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: birch_log,
            protocol_id: 11,
        },
        ItemReport {
            id: birch_planks,
            protocol_id: 12,
        },
    ]);
    let tags = TagsData::from_registries(BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:item").unwrap(),
            BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:birch_logs").unwrap(),
                vec![11],
            )]),
        )]));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[1] = ItemStack::new(11, 1);

    let result = crafting_result_from_input(
        &items,
        &ItemFactsTable::default(),
        &tags,
        &mc_data::recipes::solaris_required_recipes(),
        &inventory_crafting_input(&inventory),
    );

    assert_eq!(result, ItemStack::new(12, 4));
}

#[test]
fn crafting_table_grid_crafts_harvested_wheat_to_bread() {
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:wheat").unwrap(),
            protocol_id: 11,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:bread").unwrap(),
            protocol_id: 12,
        },
    ]);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = ItemStack::new(11, 1);
    input[1] = ItemStack::new(11, 1);
    input[2] = ItemStack::new(11, 1);

    let result = crafting_result_from_input(
        &items,
        &ItemFactsTable::default(),
        &TagsData::default(),
        &mc_data::recipes::solaris_required_recipes(),
        &input,
    );

    assert_eq!(result, ItemStack::new(12, 1));
}

#[test]
fn crafting_grid_repairs_two_matching_damaged_items_with_vanilla_bonus() {
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::ItemReport;

    let pickaxe = mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap();
    let efficiency = mc_data::Identifier::parse("minecraft:efficiency").unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: pickaxe.clone(),
        protocol_id: 11,
    }]);
    let item_facts = ItemFactsTable::from_entries([(
        pickaxe,
        ItemFacts {
            max_damage: Some(59),
            ..ItemFacts::default()
        },
    )]);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = ItemStack::new(11, 1)
        .with_damage(50)
        .with_enchantment(efficiency, 3);
    input[8] = ItemStack::new(11, 1).with_damage(40);

    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        Some(ItemStack::new(11, 1).with_damage(29))
    );
}

#[test]
fn crafting_grid_repair_clamps_to_full_durability() {
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::ItemReport;

    let pickaxe = mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: pickaxe.clone(),
        protocol_id: 11,
    }]);
    let item_facts = ItemFactsTable::from_entries([(
        pickaxe,
        ItemFacts {
            max_damage: Some(59),
            ..ItemFacts::default()
        },
    )]);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = ItemStack::new(11, 1).with_damage(1);
    input[1] = ItemStack::new(11, 1).with_damage(1);

    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        Some(ItemStack::new(11, 1).with_damage(0))
    );
}

#[test]
fn crafting_grid_repair_rejects_non_vanilla_inputs() {
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::ItemReport;

    let pickaxe = mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap();
    let shovel = mc_data::Identifier::parse("minecraft:wooden_shovel").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: pickaxe.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: shovel,
            protocol_id: 12,
        },
    ]);
    let item_facts = ItemFactsTable::from_entries([(
        pickaxe,
        ItemFacts {
            max_damage: Some(59),
            ..ItemFacts::default()
        },
    )]);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);

    input[0] = ItemStack::new(11, 2).with_damage(20);
    input[1] = ItemStack::new(11, 1).with_damage(20);
    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        None
    );

    input[0] = ItemStack::new(11, 1);
    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        None
    );

    input[0] = ItemStack::new(11, 1).with_damage(20);
    input[1] = ItemStack::new(12, 1).with_damage(20);
    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        None
    );

    input[1] = ItemStack::new(11, 1).with_damage(20);
    input[2] = ItemStack::new(11, 1).with_damage(20);
    assert_eq!(
        repair_item_crafting_result(&items, &item_facts, &input),
        None
    );
}

#[test]
fn inventory_crafting_repair_consumes_both_tools_and_returns_one() {
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::ItemReport;

    let pickaxe = mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: pickaxe.clone(),
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        pickaxe,
        ItemFacts {
            max_damage: Some(59),
            ..ItemFacts::default()
        },
    )]));
    state.inventory.slots[1] = ItemStack::new(11, 1).with_damage(50);
    state.inventory.slots[4] = ItemStack::new(11, 1).with_damage(40);
    refresh_inventory_crafting_result(&mut state);

    assert_eq!(state.inventory.slots[0].damage, Some(29));
    assert!(apply_pickup_click(&mut state, 0, 0));
    assert_eq!(state.carried_item, ItemStack::new(11, 1).with_damage(29));
    assert!(state.inventory.slots[0].is_empty());
    assert!(state.inventory.slots[1].is_empty());
    assert!(state.inventory.slots[4].is_empty());
}

#[test]
fn crafting_table_repair_consumes_inputs_across_the_three_by_three_grid() {
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::ItemReport;

    let pickaxe = mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: pickaxe.clone(),
        protocol_id: 11,
    }]));
    let mut state = interaction_state_for_items(items);
    state.item_facts = Arc::new(ItemFactsTable::from_entries([(
        pickaxe,
        ItemFacts {
            max_damage: Some(59),
            ..ItemFacts::default()
        },
    )]));
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(11, 1).with_damage(50);
    window.input[8] = ItemStack::new(11, 1).with_damage(40);
    refresh_crafting_result(&state, &mut window);

    assert_eq!(window.result.damage, Some(29));
    assert!(apply_crafting_pickup_click(&mut state, &mut window, 0, 0));
    assert_eq!(state.carried_item, ItemStack::new(11, 1).with_damage(29));
    assert!(window.result.is_empty());
    assert!(window.input.iter().all(ItemStack::is_empty));
}

#[test]
fn crafting_table_grid_crafts_missing_iron_tools_and_armor() {
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_ingot").unwrap(),
            protocol_id: 1,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:stick").unwrap(),
            protocol_id: 2,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_axe").unwrap(),
            protocol_id: 3,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_shovel").unwrap(),
            protocol_id: 4,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_hoe").unwrap(),
            protocol_id: 5,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_helmet").unwrap(),
            protocol_id: 6,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_leggings").unwrap(),
            protocol_id: 7,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:iron_boots").unwrap(),
            protocol_id: 8,
        },
    ]);
    let recipes = mc_data::recipes::solaris_required_recipes();
    let craft = |occupied: &[(usize, u32)], expected_item_id| {
        let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
        for &(slot, item_id) in occupied {
            input[slot] = ItemStack::new(item_id, 1);
        }
        assert_eq!(
            crafting_result_from_input(
                &items,
                &ItemFactsTable::default(),
                &TagsData::default(),
                &recipes,
                &input,
            ),
            ItemStack::new(expected_item_id, 1)
        );
    };

    craft(&[(0, 1), (1, 1), (3, 1), (4, 2), (7, 2)], 3);
    craft(&[(0, 1), (3, 2), (6, 2)], 4);
    craft(&[(0, 1), (1, 1), (4, 2), (7, 2)], 5);
    craft(&[(0, 1), (1, 1), (2, 1), (3, 1), (5, 1)], 6);
    craft(&[(0, 1), (1, 1), (2, 1), (3, 1), (5, 1), (6, 1), (8, 1)], 7);
    craft(&[(0, 1), (2, 1), (3, 1), (5, 1)], 8);
}

#[test]
fn crafting_table_grid_crafts_missing_diamond_tools_and_armor() {
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond").unwrap(),
            protocol_id: 1,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:stick").unwrap(),
            protocol_id: 2,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_axe").unwrap(),
            protocol_id: 3,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_boots").unwrap(),
            protocol_id: 4,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_chestplate").unwrap(),
            protocol_id: 5,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_helmet").unwrap(),
            protocol_id: 6,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_hoe").unwrap(),
            protocol_id: 7,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_leggings").unwrap(),
            protocol_id: 8,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:diamond_shovel").unwrap(),
            protocol_id: 9,
        },
    ]);
    let recipes = mc_data::recipes::solaris_required_recipes();
    let craft = |occupied: &[(usize, u32)], expected_item_id| {
        let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
        for &(slot, item_id) in occupied {
            input[slot] = ItemStack::new(item_id, 1);
        }
        assert_eq!(
            crafting_result_from_input(
                &items,
                &ItemFactsTable::default(),
                &TagsData::default(),
                &recipes,
                &input,
            ),
            ItemStack::new(expected_item_id, 1)
        );
    };

    craft(&[(0, 1), (1, 1), (3, 1), (4, 2), (7, 2)], 3);
    craft(&[(0, 1), (2, 1), (3, 1), (5, 1)], 4);
    craft(
        &[
            (0, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 1),
        ],
        5,
    );
    craft(&[(0, 1), (1, 1), (2, 1), (3, 1), (5, 1)], 6);
    craft(&[(0, 1), (1, 1), (4, 2), (7, 2)], 7);
    craft(&[(0, 1), (1, 1), (2, 1), (3, 1), (5, 1), (6, 1), (8, 1)], 8);
    craft(&[(0, 1), (3, 2), (6, 2)], 9);
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
            &mc_data::Identifier::parse("minecraft:bread").unwrap()
        ),
        Some((
            mc_data::food::FoodEntry {
                food: 5,
                saturation: 6.0,
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
            ..mc_data::item_components::ItemFacts::default()
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
fn attack_damage_prefers_item_component_modifiers() {
    use mc_data::items::ItemReport;

    let sword = mc_data::Identifier::parse("minecraft:diamond_sword").unwrap();
    let stick = mc_data::Identifier::parse("minecraft:stick").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: sword.clone(),
            protocol_id: 1,
        },
        ItemReport {
            id: stick,
            protocol_id: 2,
        },
    ]);
    let facts = ItemFactsTable::from_entries([(
        sword,
        mc_data::item_components::ItemFacts {
            attack_damage_modifier: Some(6.0),
            ..Default::default()
        },
    )]);

    assert_eq!(attack_damage_for_item(&facts, &items, Some(1)), 7.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(2)), 1.0);
    assert_eq!(attack_damage_for_item(&facts, &items, None), 1.0);
}

#[test]
fn fallback_sword_damage_uses_material_tier() {
    use mc_data::items::ItemReport;

    let reports: Vec<_> = [
        "wooden_sword",
        "stone_sword",
        "iron_sword",
        "diamond_sword",
        "netherite_sword",
        "golden_sword",
        "custom_sword",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, path)| ItemReport {
        id: mc_data::Identifier::parse(format!("minecraft:{path}")).unwrap(),
        protocol_id: u32::try_from(index + 1).unwrap(),
    })
    .collect();
    let items = ItemRegistry::from_report(&reports);
    let facts = ItemFactsTable::default();

    assert_eq!(attack_damage_for_item(&facts, &items, Some(1)), 4.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(2)), 5.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(3)), 6.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(4)), 7.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(5)), 8.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(6)), 4.0);
    assert_eq!(attack_damage_for_item(&facts, &items, Some(7)), 2.0);
}

#[test]
fn fallback_mining_rules_use_block_family_and_matching_tool() {
    let stone_hand = fallback_mining_time("stone", None);
    let stone_pickaxe = fallback_mining_time("stone", Some("iron_pickaxe"));
    let stone_shovel = fallback_mining_time("stone", Some("iron_shovel"));

    assert!(stone_pickaxe < stone_hand);
    assert_eq!(stone_shovel, stone_hand);
    assert!(
        fallback_mining_time("oak_log", Some("stone_axe")) < fallback_mining_time("oak_log", None)
    );
    assert!(
        fallback_mining_time("dirt", Some("wooden_shovel")) < fallback_mining_time("dirt", None)
    );
    assert_eq!(
        fallback_mining_time("podzol", None),
        Duration::from_millis(750)
    );
    assert_eq!(
        fallback_mining_time("unknown_custom_block", None),
        Duration::from_millis(1_200)
    );
}

#[test]
fn zero_hardness_blocks_have_instant_vanilla_progress() {
    assert!(fallback_mining_time("short_grass", None).is_zero());
    assert!(fallback_mining_time("wheat", None).is_zero());
    assert!(!fallback_mining_time("grass_block", None).is_zero());
    assert!(!fallback_mining_time("oak_log", None).is_zero());
}

#[test]
fn fallback_drop_rules_enforce_common_pickaxe_progression() {
    assert!(fallback_tool_allows_block_drop("dirt", None));
    assert!(!fallback_tool_allows_block_drop("stone", None));
    assert!(!fallback_tool_allows_block_drop(
        "stone",
        Some("wooden_shovel")
    ));
    assert!(fallback_tool_allows_block_drop(
        "stone",
        Some("wooden_pickaxe")
    ));

    assert!(!fallback_tool_allows_block_drop(
        "iron_ore",
        Some("wooden_pickaxe")
    ));
    assert!(fallback_tool_allows_block_drop(
        "deepslate_iron_ore",
        Some("stone_pickaxe")
    ));
    assert!(!fallback_tool_allows_block_drop(
        "diamond_ore",
        Some("stone_pickaxe")
    ));
    assert!(fallback_tool_allows_block_drop(
        "deepslate_diamond_ore",
        Some("iron_pickaxe")
    ));
    assert!(!fallback_tool_allows_block_drop(
        "obsidian",
        Some("iron_pickaxe")
    ));
    assert!(fallback_tool_allows_block_drop(
        "obsidian",
        Some("diamond_pickaxe")
    ));
}
