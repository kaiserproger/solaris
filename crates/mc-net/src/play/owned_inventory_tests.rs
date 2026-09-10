use std::collections::BTreeMap;

use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_data::{Identifier, ItemStack};
use mc_script::{ScriptInventoryEndpoint, ScriptOperationFailure, ScriptOwnedItemTransfer};

use super::owned_inventory::plan_owned_item_transfers;

fn endpoints() -> (ScriptInventoryEndpoint, ScriptInventoryEndpoint) {
    (
        ScriptInventoryEndpoint::PlayerInventory { player_id: 7 },
        ScriptInventoryEndpoint::Warehouse {
            handle: "completed-container".to_owned(),
        },
    )
}

#[test]
fn owned_transfer_preserves_components_in_both_directions() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let item_id = items
        .id_of(&Identifier::parse("minecraft:iron_sword").unwrap())
        .unwrap();
    let sword = ItemStack::new(item_id, 1)
        .with_damage(17)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 2)
        .with_custom_name("Settler's blade")
        .with_item_model(Identifier::parse("minecraft:diamond_sword").unwrap());
    let (player, warehouse) = endpoints();
    let mut player_slots = vec![ItemStack::EMPTY; 46];
    player_slots[9] = sword.clone();
    let inventories = BTreeMap::from([
        (player.clone(), player_slots),
        (warehouse.clone(), vec![ItemStack::EMPTY; 27]),
    ]);
    let deposited = plan_owned_item_transfers(
        &[ScriptOwnedItemTransfer::new(
            player.clone(),
            9,
            warehouse.clone(),
            0,
            1,
        )],
        &inventories,
        &items,
        &facts,
    )
    .unwrap();
    assert_eq!(deposited[&player][9], ItemStack::EMPTY);
    assert_eq!(deposited[&warehouse][0], sword);
    let withdrawn = plan_owned_item_transfers(
        &[ScriptOwnedItemTransfer::new(
            warehouse.clone(),
            0,
            player.clone(),
            10,
            1,
        )],
        &deposited,
        &items,
        &facts,
    )
    .unwrap();
    assert_eq!(withdrawn[&warehouse][0], ItemStack::EMPTY);
    assert_eq!(withdrawn[&player][10], sword);
    assert_eq!(inventories[&player][9], sword);
}

#[test]
fn owned_transfer_rejects_late_capacity_failure_without_debiting_any_owner() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let item_id = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let (player, warehouse) = endpoints();
    let mut player_slots = vec![ItemStack::EMPTY; 46];
    player_slots[9] = ItemStack::new(item_id, 8);
    let mut chest_slots = vec![ItemStack::EMPTY; 27];
    chest_slots[1] = ItemStack::new(item_id, 64);
    let inventories = BTreeMap::from([
        (player.clone(), player_slots),
        (warehouse.clone(), chest_slots),
    ]);
    let before = inventories.clone();
    assert_eq!(
        plan_owned_item_transfers(
            &[
                ScriptOwnedItemTransfer::new(player.clone(), 9, warehouse.clone(), 0, 3),
                ScriptOwnedItemTransfer::new(player, 9, warehouse, 1, 2),
            ],
            &inventories,
            &items,
            &facts
        ),
        Err(ScriptOperationFailure::Capacity)
    );
    assert_eq!(inventories, before);
}

#[test]
fn owned_transfer_cannot_merge_different_components_or_spend_a_stack_twice() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let item_id = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let (player, warehouse) = endpoints();
    let mut player_slots = vec![ItemStack::EMPTY; 46];
    player_slots[9] = ItemStack::new(item_id, 3).with_custom_name("Ration");
    let mut chest_slots = vec![ItemStack::EMPTY; 27];
    chest_slots[0] = ItemStack::new(item_id, 2);
    let inventories = BTreeMap::from([
        (player.clone(), player_slots),
        (warehouse.clone(), chest_slots),
    ]);
    assert_eq!(
        plan_owned_item_transfers(
            &[ScriptOwnedItemTransfer::new(
                player.clone(),
                9,
                warehouse.clone(),
                0,
                1
            ),],
            &inventories,
            &items,
            &facts
        ),
        Err(ScriptOperationFailure::Blocked)
    );
    assert_eq!(
        plan_owned_item_transfers(
            &[
                ScriptOwnedItemTransfer::new(player.clone(), 9, warehouse.clone(), 1, 2),
                ScriptOwnedItemTransfer::new(player, 9, warehouse, 2, 2),
            ],
            &inventories,
            &items,
            &facts
        ),
        Err(ScriptOperationFailure::InsufficientItems)
    );
}

#[test]
fn owned_snapshot_fence_detects_native_changes_without_a_journal_commit() {
    let items = solaris_required_items();
    let item_id = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let (player, _) = endpoints();
    let mut slots = vec![ItemStack::EMPTY; 46];
    slots[9] = ItemStack::new(item_id, 3);
    let before =
        super::owned_inventory::owned_inventory_snapshot(player.clone(), 5, &slots, &items)
            .unwrap();
    slots[9].count -= 1;
    let after =
        super::owned_inventory::owned_inventory_snapshot(player, 5, &slots, &items).unwrap();
    assert_ne!(before.fence, after.fence);
}
