use mc_data::Identifier;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_world::FurnaceBlockEntity;

use super::{FurnaceClickAction, FurnaceClickInput, FurnaceKind, plan_click};
use crate::play::{ItemStack, PlayerInventory};

#[test]
fn non_flammable_wood_is_rejected_by_every_fuel_slot_click_path() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:warped_stairs").unwrap(),
        protocol_id: 11,
    }]);
    let tags = mc_data::tags::solaris_required_item_tags(&items);
    let item_facts = ItemFactsTable::default();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(11, 1);

    for action in [
        FurnaceClickAction::Pickup { slot: 1, button: 0 },
        FurnaceClickAction::QuickMove { slot: 30 },
        FurnaceClickAction::Swap { slot: 1, button: 0 },
    ] {
        let carried_item = match action {
            FurnaceClickAction::Pickup { .. } => ItemStack::new(11, 1),
            _ => ItemStack::EMPTY,
        };
        let plan = plan_click(FurnaceClickInput {
            recipes: &[],
            items: &items,
            item_facts: &item_facts,
            tags: &tags,
            kind: FurnaceKind::Furnace,
            furnace: FurnaceBlockEntity::default(),
            inventory: inventory.clone(),
            carried_item,
            action,
            experience_seed: 0,
        });

        assert!(plan.is_none());
    }
}
