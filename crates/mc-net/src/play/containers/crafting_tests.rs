use mc_data::Identifier;
use mc_data::item_components::{ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;
use mc_protocol::packets::play::ItemStack;

use super::{CraftingTableWindow, crafting_remainder_for_item, repair_item_crafting_result};
use crate::play::containers::{QuickCraftClick, QuickCraftOutcome, QuickCraftState};
use crate::play::inventory::PlayerInventory;

#[test]
fn repair_combines_remaining_durability_and_five_percent_bonus() {
    let item = Identifier::parse("minecraft:iron_pickaxe").unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: item.clone(),
        protocol_id: 42,
    }]);
    let facts = ItemFactsTable::from_entries([(
        item,
        ItemFacts {
            max_damage: Some(100),
            ..ItemFacts::default()
        },
    )]);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = ItemStack::new(42, 1).with_damage(80);
    input[8] = ItemStack::new(42, 1).with_damage(80);

    assert_eq!(
        repair_item_crafting_result(&items, &facts, &input),
        Some(ItemStack::new(42, 1).with_damage(55))
    );
}

#[test]
fn inventory_quickcraft_distributes_cursor_across_the_two_by_two_grid() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    let mut carried = ItemStack::new(42, 4);
    let mut quickcraft = QuickCraftState::default();

    let click = |header, slot| QuickCraftClick {
        header,
        kind: 1,
        slot,
    };
    assert_eq!(
        inventory.apply_crafting_quickcraft_click(
            &items,
            &item_facts,
            &mut carried,
            &mut quickcraft,
            click(0, None),
            &tags,
            &[],
        ),
        QuickCraftOutcome::Pending
    );
    for slot in 1..=4 {
        assert_eq!(
            inventory.apply_crafting_quickcraft_click(
                &items,
                &item_facts,
                &mut carried,
                &mut quickcraft,
                click(1, Some(slot)),
                &tags,
                &[],
            ),
            QuickCraftOutcome::Pending
        );
    }
    assert_eq!(
        inventory.apply_crafting_quickcraft_click(
            &items,
            &item_facts,
            &mut carried,
            &mut quickcraft,
            click(2, None),
            &tags,
            &[],
        ),
        QuickCraftOutcome::Changed
    );

    assert!(carried.is_empty());
    for slot in 1..=4 {
        assert_eq!(inventory.slots[slot], ItemStack::new(42, 1));
    }
}

#[test]
fn crafting_table_quickcraft_distributes_cursor_across_the_three_by_three_grid() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    let mut carried = ItemStack::new(42, 9);
    let mut window = CraftingTableWindow::new(7);

    let click = |header, slot| QuickCraftClick {
        header,
        kind: 1,
        slot,
    };
    assert_eq!(
        window.apply_quickcraft_click(
            &items,
            &item_facts,
            &mut inventory,
            &mut carried,
            click(0, None),
            &tags,
            &[],
        ),
        QuickCraftOutcome::Pending
    );
    for slot in 1..=9 {
        assert_eq!(
            window.apply_quickcraft_click(
                &items,
                &item_facts,
                &mut inventory,
                &mut carried,
                click(1, Some(slot)),
                &tags,
                &[],
            ),
            QuickCraftOutcome::Pending
        );
    }
    assert_eq!(
        window.apply_quickcraft_click(
            &items,
            &item_facts,
            &mut inventory,
            &mut carried,
            click(2, None),
            &tags,
            &[],
        ),
        QuickCraftOutcome::Changed
    );

    assert!(carried.is_empty());
    for slot in &window.input {
        assert_eq!(*slot, ItemStack::new(42, 1));
    }
}

#[test]
fn inventory_result_quick_move_is_transactional_when_only_part_fits() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[0] = ItemStack::new(42, 4);
    inventory.slots[1] = ItemStack::new(7, 1);
    inventory.slots[9] = ItemStack::new(42, 62);
    for slot in 10..=44 {
        inventory.slots[slot] = ItemStack::new(99, 64);
    }
    let before = inventory.clone();

    let (changed, discarded_remainders) =
        inventory.apply_crafting_quick_move_click(&items, &item_facts, &tags, &[], 0);

    assert!(!changed);
    assert!(discarded_remainders.is_empty());
    assert_eq!(inventory.slots, before.slots);
}

#[test]
fn crafting_table_result_pickup_consumes_the_matching_grid() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    let mut carried_item = ItemStack::EMPTY;
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(7, 1);
    window.result = ItemStack::new(42, 4);

    let (changed, discarded_remainders) = window.apply_pickup_click(
        &items,
        &item_facts,
        &tags,
        &[],
        &mut inventory,
        &mut carried_item,
        0,
        0,
    );

    assert!(changed);
    assert!(discarded_remainders.is_empty());
    assert_eq!(carried_item, ItemStack::new(42, 4));
    assert!(window.input.iter().all(ItemStack::is_empty));
    assert!(window.result.is_empty());
}

#[test]
fn filled_bucket_crafting_remainder_is_an_empty_bucket() {
    let bucket = Identifier::parse("minecraft:bucket").unwrap();
    let milk_bucket = Identifier::parse("minecraft:milk_bucket").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: bucket,
            protocol_id: 1,
        },
        ItemReport {
            id: milk_bucket,
            protocol_id: 2,
        },
    ]);

    assert_eq!(
        crafting_remainder_for_item(&items, 2),
        Some(ItemStack::new(1, 1))
    );
}

#[test]
fn crafting_table_player_quick_move_preserves_the_stack() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(42, 3);
    let mut window = CraftingTableWindow::new(7);

    let (changed, discarded_remainders) =
        window.apply_quick_move_click(&items, &item_facts, &tags, &[], &mut inventory, 10);

    assert!(changed);
    assert!(discarded_remainders.is_empty());
    assert!(inventory.slots[9].is_empty());
    assert_eq!(inventory.slots[36], ItemStack::new(42, 3));
}

#[test]
fn inventory_main_quick_move_keeps_hotbar_overflow_in_source_slot() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(42, 3);
    inventory.slots[36] = ItemStack::new(42, 63);
    for slot in 37..=44 {
        inventory.slots[slot] = ItemStack::new(99, 64);
    }

    let (changed, discarded_remainders) =
        inventory.apply_crafting_quick_move_click(&items, &item_facts, &tags, &[], 9);

    assert!(changed);
    assert!(discarded_remainders.is_empty());
    assert_eq!(inventory.slots[36], ItemStack::new(42, 64));
    assert_eq!(inventory.slots[9], ItemStack::new(42, 2));
    assert!(inventory.slots[10].is_empty());
}

#[test]
fn crafting_table_grid_quick_move_returns_input_to_inventory() {
    let items = ItemRegistry::from_report(&[]);
    let item_facts = ItemFactsTable::default();
    let tags = TagsData::default();
    let mut inventory = PlayerInventory::empty();
    let mut window = CraftingTableWindow::new(7);
    window.input[0] = ItemStack::new(42, 3);

    let (changed, discarded_remainders) =
        window.apply_quick_move_click(&items, &item_facts, &tags, &[], &mut inventory, 1);

    assert!(changed);
    assert!(discarded_remainders.is_empty());
    assert!(window.input[0].is_empty());
    assert_eq!(inventory.slots[9], ItemStack::new(42, 3));
}
