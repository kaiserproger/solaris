use super::{
    Identifier, ItemStack, XpState, enchant_item_candidate, enchanting_offer,
    item_is_efficiency_enchantable,
};

#[test]
fn enchanting_selection_consumes_lapis_and_level_but_preserves_total_xp() {
    let items = mc_data::items::solaris_required_items();
    let item_facts = mc_data::item_components::solaris_required_item_facts();
    let pickaxe = items
        .id_of(&Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .unwrap();
    let lapis = items
        .id_of(&Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .unwrap();
    let pickaxe_name = items.name_of(pickaxe).expect("pickaxe registry name");
    assert!(item_is_efficiency_enchantable(&item_facts, pickaxe_name));
    let mut inputs = [ItemStack::new(pickaxe, 1), ItemStack::new(lapis, 2)];
    let mut xp = XpState {
        level: 1,
        progress: 0.5,
        total: 12,
        seed: 123,
    };

    assert!(enchant_item_candidate(
        &items,
        &item_facts,
        &mut inputs,
        &mut xp,
        enchanting_offer(0, 0).expect("base offer"),
    ));

    assert_eq!(
        inputs[0].enchantments,
        vec![mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:efficiency").unwrap(),
            level: 1,
        }]
    );
    assert_eq!(inputs[1], ItemStack::new(lapis, 1));
    assert_eq!(xp.level, 0);
    assert_eq!(xp.progress, 0.5);
    assert_eq!(xp.total, 12);
    assert_ne!(xp.seed, 123);
}
