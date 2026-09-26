use mc_data::Identifier;
use mc_data::items::ItemReport;

use super::*;

#[test]
fn chest_deposit_and_withdraw_preserve_item_components() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_sword").unwrap(),
        protocol_id: 11,
    }]);
    let item_facts = ItemFactsTable::default();
    let sword = ItemStack::new(11, 1)
        .with_damage(17)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 2)
        .with_custom_name("Guard's sword")
        .with_item_model(Identifier::parse("solaris:guard_sword").unwrap());
    let deposited = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: ChestWindow::new(vec![BlockPos { x: 0, y: 64, z: 0 }], 1),
        view: ChestView {
            chests: vec![ChestBlockEntity::default()],
        },
        inventory: PlayerInventory::empty(),
        carried_item: sword.clone(),
        action: ChestClickAction::Pickup { slot: 0, button: 0 },
    });
    assert!(deposited.changed);
    assert!(deposited.carried_item.is_empty());
    let withdrawn = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: deposited.window,
        view: deposited.view,
        inventory: deposited.inventory,
        carried_item: deposited.carried_item,
        action: ChestClickAction::Pickup { slot: 0, button: 0 },
    });
    assert_eq!(withdrawn.carried_item, sword);
    assert!(withdrawn.view.chests[0].slots[0].is_empty());
}

#[test]
fn registered_custom_item_chest_transfer_keeps_identity_and_stack_limit() {
    use mc_data::item_components::{CustomItemDefinition, ItemFacts};

    let paper = Identifier::parse("minecraft:paper").unwrap();
    let ruby = Identifier::parse("ruby-live:ruby").unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: paper.clone(),
        protocol_id: 11,
    }]);
    let item_facts = ItemFactsTable::default()
        .with_custom_items(
            [CustomItemDefinition {
                id: ruby.clone(),
                carrier: paper,
                name: "Ruby".into(),
                crafting_ingredient: None,
                facts: ItemFacts {
                    max_stack_size: Some(4),
                    ..ItemFacts::default()
                },
            }],
            &items,
        )
        .unwrap();
    let ruby_stack = ItemStack::new(11, 4).with_item_model(ruby);
    let window = ChestWindow::new(vec![BlockPos { x: 0, y: 64, z: 0 }], 1);
    let deposited = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window,
        view: ChestView {
            chests: vec![ChestBlockEntity::default()],
        },
        inventory: PlayerInventory::empty(),
        carried_item: ruby_stack.clone(),
        action: ChestClickAction::Pickup { slot: 0, button: 0 },
    });
    assert!(deposited.changed);
    assert!(deposited.carried_item.is_empty());

    let swapped_carrier = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: deposited.window,
        view: deposited.view,
        inventory: deposited.inventory,
        carried_item: ItemStack::new(11, 1),
        action: ChestClickAction::Pickup { slot: 0, button: 0 },
    });
    assert!(swapped_carrier.changed);
    assert_eq!(swapped_carrier.view.chests[0].slots[0].count, 1);
    assert!(swapped_carrier.view.chests[0].slots[0].item_model.is_none());
    assert_eq!(swapped_carrier.carried_item, ruby_stack);

    let deposited_again = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: swapped_carrier.window,
        view: swapped_carrier.view,
        inventory: swapped_carrier.inventory,
        carried_item: swapped_carrier.carried_item,
        action: ChestClickAction::Pickup { slot: 1, button: 0 },
    });
    assert!(deposited_again.carried_item.is_empty());
    let withdrawn = plan_click(ChestClickInput {
        items: &items,
        item_facts: &item_facts,
        window: deposited_again.window,
        view: deposited_again.view,
        inventory: deposited_again.inventory,
        carried_item: ItemStack::EMPTY,
        action: ChestClickAction::Pickup { slot: 1, button: 0 },
    });
    assert_eq!(withdrawn.carried_item, ruby_stack);
}
