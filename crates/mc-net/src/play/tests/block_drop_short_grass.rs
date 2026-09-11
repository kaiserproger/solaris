use super::*;
use crate::play::survival::block_drop_stacks_with_tool_and_facts_from_seeded;

#[test]
fn short_grass_fallback_resolves_empty_and_seed_drops_from_context() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:short_grass"),
    ])
    .unwrap();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let grass = blocks
        .block(&Identifier::parse("minecraft:short_grass").unwrap())
        .unwrap()
        .default;
    let mut hits = 0;
    for seed in 0..1024 {
        let drops = block_drop_stacks_with_tool_and_facts_from_seeded(
            &mc_data::loot::LootTables::default(),
            &items,
            &mc_data::item_components::ItemFactsTable::default(),
            &blocks,
            grass,
            None,
            seed,
        );
        if !drops.is_empty() {
            assert_eq!(drops, [ItemStack::new(51, 1)]);
            hits += 1;
        }
    }
    assert!((90..=165).contains(&hits), "seed-producing breaks: {hits}");
}
