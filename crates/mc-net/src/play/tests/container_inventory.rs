use std::sync::Arc;

use super::{
    ActiveContainer, ChestBlockEntity, ChestClickAction, ChestClickInput, ChestView, ChestWindow,
    CraftingTableWindow, FurnaceKind, FurnaceWindow, Identifier, ItemFactsTable, ItemRegistry,
    ItemReport, ItemStack, PlayerInventory, QuickCraftClick, SINGLE_CHEST_STORAGE_SLOTS,
    apply_chest_quick_move_click, chest_menu_state_change_count, crafting_menu_state_change_count,
    furnace_slot_to_stack, interaction_state_for_items, persistent_container_claim_allowed,
    plan_chest_click, stack_to_furnace_slot,
};

#[test]
fn chest_quick_move_places_player_stack_in_first_empty_storage_slot() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(dirt_id, 1);
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };

    assert!(apply_chest_quick_move_click(
        &mut state,
        &mut view,
        SINGLE_CHEST_STORAGE_SLOTS + 27,
    ));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(dirt_id, 1)
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::EMPTY
    );
}

#[test]
fn chest_quick_move_from_storage_uses_vanilla_reverse_player_range() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: dirt,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut view = ChestView {
        chests: vec![chest],
    };

    assert!(apply_chest_quick_move_click(&mut state, &mut view, 0));
    assert!(view.chests[0].slots[0].is_empty());
    assert_eq!(
        state.inventory.slots[44],
        ItemStack::new(dirt_id, 2),
        "vanilla fills the reverse player range before earlier main-inventory slots"
    );
    assert!(state.inventory.slots[9..44].iter().all(ItemStack::is_empty));
}

#[test]
fn chest_actions_respect_item_specific_stack_limits() {
    let bucket = Identifier::parse("minecraft:bucket").unwrap();
    let snowball = Identifier::parse("minecraft:snowball").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: bucket.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: snowball.clone(),
            protocol_id: 11,
        },
    ]);
    let item_facts = ItemFactsTable::from_entries([
        (
            bucket,
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(1),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
        (
            snowball,
            mc_data::item_components::ItemFacts {
                max_stack_size: Some(16),
                ..mc_data::item_components::ItemFacts::default()
            },
        ),
    ]);
    let new_window = || ChestWindow::new(vec![mc_world::BlockPos { x: 0, y: 64, z: 0 }], 7);
    let empty_view = || ChestView {
        chests: vec![ChestBlockEntity::default()],
    };

    let mut bucket_view = empty_view();
    bucket_view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(10, 1));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(10, 1);
    let bucket_quick_move = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: bucket_view,
        inventory,
        carried_item: ItemStack::EMPTY,
        action: ChestClickAction::QuickMove {
            slot: SINGLE_CHEST_STORAGE_SLOTS + 27,
        },
    });
    assert!(bucket_quick_move.changed);
    assert!(bucket_quick_move.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());
    assert_eq!(
        furnace_slot_to_stack(&bucket_quick_move.view.chests[0].slots[0]),
        ItemStack::new(10, 1)
    );
    assert_eq!(
        furnace_slot_to_stack(&bucket_quick_move.view.chests[0].slots[1]),
        ItemStack::new(10, 1)
    );
    assert!(
        bucket_quick_move.view.chests[0].slots[2..]
            .iter()
            .all(mc_world::FurnaceSlot::is_empty)
    );

    let mut full_bucket = ChestBlockEntity::default();
    full_bucket.slots[0] = stack_to_furnace_slot(&ItemStack::new(10, 1));
    let bucket_pickup = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: ChestView {
            chests: vec![full_bucket],
        },
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::new(10, 1),
        action: ChestClickAction::Pickup { slot: 0, button: 1 },
    });
    assert!(!bucket_pickup.changed);
    assert_eq!(bucket_pickup.carried_item, ItemStack::new(10, 1));
    assert_eq!(
        furnace_slot_to_stack(&bucket_pickup.view.chests[0].slots[0]),
        ItemStack::new(10, 1)
    );

    let mut snowball_view = empty_view();
    snowball_view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(11, 15));
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(11, 16);
    let snowball_quick_move = plan_chest_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: new_window(),
        view: snowball_view,
        inventory,
        carried_item: ItemStack::EMPTY,
        action: ChestClickAction::QuickMove {
            slot: SINGLE_CHEST_STORAGE_SLOTS + 27,
        },
    });
    assert!(snowball_quick_move.changed);
    assert_eq!(
        furnace_slot_to_stack(&snowball_quick_move.view.chests[0].slots[0]),
        ItemStack::new(11, 16)
    );
    assert_eq!(
        furnace_slot_to_stack(&snowball_quick_move.view.chests[0].slots[1]),
        ItemStack::new(11, 15)
    );

    let mut view = empty_view();
    view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(11, 15));
    let mut window = new_window();
    let mut inventory = PlayerInventory::empty();
    let mut carried_item = ItemStack::new(11, 3);
    let mut changed = false;
    for click in [
        QuickCraftClick {
            header: 0,
            kind: 1,
            slot: None,
        },
        QuickCraftClick {
            header: 1,
            kind: 1,
            slot: Some(0),
        },
        QuickCraftClick {
            header: 1,
            kind: 1,
            slot: Some(1),
        },
        QuickCraftClick {
            header: 2,
            kind: 1,
            slot: None,
        },
    ] {
        let plan = plan_chest_click(ChestClickInput {
            items: &items,
            item_facts: &item_facts,
            window,
            view,
            inventory,
            carried_item,
            action: ChestClickAction::QuickCraft(click),
        });
        window = plan.window;
        view = plan.view;
        inventory = plan.inventory;
        carried_item = plan.carried_item;
        changed = plan.changed;
    }
    assert!(changed);
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(11, 16)
    );
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[1]),
        ItemStack::new(11, 1)
    );
    assert_eq!(carried_item, ItemStack::new(11, 1));
}

#[test]
fn chest_menu_revision_counts_source_and_destination_slot_changes() {
    let mut before_chest = ChestBlockEntity::default();
    before_chest.slots[0] = mc_world::FurnaceSlot {
        item_id: 10,
        count: 2,
        damage: None,
        enchantments: Vec::new(),
    };
    let before_view = ChestView {
        chests: vec![before_chest],
    };
    let after_view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };
    let before_inventory = PlayerInventory::empty();
    let mut after_inventory = PlayerInventory::empty();
    after_inventory.slots[44] = ItemStack::new(10, 2);

    assert_eq!(
        chest_menu_state_change_count(
            &before_view,
            &after_view,
            &before_inventory,
            &after_inventory,
            &ItemStack::EMPTY,
            &ItemStack::EMPTY,
        ),
        2
    );
}

#[test]
fn crafting_menu_revision_counts_result_input_and_destination_changes() {
    let mut before_window = CraftingTableWindow::new(7);
    before_window.input[0] = ItemStack::new(10, 1);
    before_window.result = ItemStack::new(11, 4);
    let after_window = CraftingTableWindow::new(7);
    let before_inventory = PlayerInventory::empty();
    let mut after_inventory = PlayerInventory::empty();
    after_inventory.slots[44] = ItemStack::new(11, 4);

    assert_eq!(
        crafting_menu_state_change_count(
            &before_window,
            &after_window,
            &before_inventory,
            &after_inventory,
            &ItemStack::EMPTY,
            &ItemStack::EMPTY,
        ),
        3
    );
}

#[test]
fn persistent_container_claim_check_covers_furnace_and_both_chest_halves() {
    let first = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let second = mc_world::BlockPos { x: 2, y: 64, z: 2 };
    let chest = ActiveContainer::Chest(ChestWindow::new(vec![first, second], 7));
    assert!(!persistent_container_claim_allowed(&chest, |position| {
        position != second
    }));
    assert!(persistent_container_claim_allowed(&chest, |_| true));

    let furnace = ActiveContainer::Furnace(FurnaceWindow::new(first, 8, FurnaceKind::Furnace));
    assert!(!persistent_container_claim_allowed(&furnace, |_| false));
    assert!(persistent_container_claim_allowed(&furnace, |_| true));
}
