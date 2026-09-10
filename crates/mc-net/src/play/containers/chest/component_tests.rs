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
