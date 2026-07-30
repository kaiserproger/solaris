use super::{
    Identifier, ItemRegistry, ItemReport, ItemStack, block_drop_stacks_from, crop_test_registry,
};
use std::collections::BTreeMap;

#[test]
fn block_drop_configured_loot_count_reaches_runtime_stack() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:carrot").unwrap(),
        protocol_id: 52,
    }]);
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;
    let loot = mc_data::loot::LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(
            Identifier::parse("minecraft:dirt").unwrap(),
            mc_data::loot::LootDrop {
                item: Identifier::parse("minecraft:carrot").unwrap(),
                count: mc_data::loot::LootCount::Fixed(3),
            },
        )]),
    );

    let drops = block_drop_stacks_from(&loot, &items, &blocks, dirt);

    assert_eq!(drops, vec![ItemStack::new(52, 3)]);
}
