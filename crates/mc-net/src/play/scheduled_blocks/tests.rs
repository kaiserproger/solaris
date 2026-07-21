use mc_data::Identifier;
use mc_data::items::{ItemRegistry, ItemReport};
use mc_world::{FurnaceBlockEntity, FurnaceSlot};

use super::insert_hopper_fuel_into_furnace;
use crate::play::FurnaceKind;

#[test]
fn hopper_rejects_non_flammable_wood_without_mutating_the_furnace() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:warped_stairs").unwrap(),
        protocol_id: 44,
    }]);
    let tags = mc_data::tags::solaris_required_item_tags(&items);
    let moving = FurnaceSlot {
        count: 1,
        item_id: 44,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut furnace = FurnaceBlockEntity::default();

    assert_eq!(
        insert_hopper_fuel_into_furnace(&tags, FurnaceKind::Furnace, &mut furnace, &moving,),
        None
    );
    assert!(furnace.slots[1].is_empty());
}
