use super::{
    Identifier, ItemRegistry, ItemReport, ItemStack, block_drop_stacks_from, simple_block,
};

#[test]
fn block_drop_builtin_short_grass_returns_wheat_seeds() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:short_grass"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let short_grass = blocks
        .block(&Identifier::parse("minecraft:short_grass").unwrap())
        .unwrap()
        .default;

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        short_grass,
    );

    assert_eq!(drops, vec![ItemStack::new(51, 1)]);
}
