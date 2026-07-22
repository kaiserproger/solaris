use mc_data::Identifier;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_world::{FurnaceBlockEntity, FurnaceSlot};

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

#[test]
fn output_quick_move_uses_the_vanilla_reverse_player_range() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:iron_ingot").unwrap(),
        protocol_id: 11,
    }]);
    let mut furnace = FurnaceBlockEntity::default();
    furnace.slots[2] = FurnaceSlot {
        item_id: 11,
        count: 3,
        ..FurnaceSlot::default()
    };
    let plan = plan_click(FurnaceClickInput {
        recipes: &[],
        items: &items,
        item_facts: &ItemFactsTable::default(),
        tags: &mc_data::tags::TagsData::default(),
        kind: FurnaceKind::Furnace,
        furnace,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        action: FurnaceClickAction::QuickMove { slot: 2 },
        experience_seed: 0,
    })
    .expect("furnace output moves");

    assert!(plan.furnace.slots[2].is_empty());
    assert_eq!(plan.inventory.slots[44], ItemStack::new(11, 3));
}
