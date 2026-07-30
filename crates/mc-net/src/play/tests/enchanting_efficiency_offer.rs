use super::{EnchantingTableWindow, Identifier, ItemStack, XpState, enchanting_data_values};

#[test]
fn enchanting_data_exposes_the_supported_efficiency_offer() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let mut window = EnchantingTableWindow::at_position(7, mc_world::BlockPos { x: 0, y: 0, z: 0 });
    window.inputs[0] = ItemStack::new(pickaxe, 1);
    let xp = XpState {
        seed: 123,
        ..XpState::default()
    };

    assert_eq!(
        enchanting_data_values(&items, &item_facts, &window, &xp, 0),
        [
            (0, 1),
            (1, 0),
            (2, 0),
            (3, 123),
            (4, 8),
            (5, -1),
            (6, -1),
            (7, 1),
            (8, -1),
            (9, -1),
        ]
    );
}
