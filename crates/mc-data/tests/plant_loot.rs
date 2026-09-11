use std::collections::BTreeMap;

use mc_data::Identifier;
use mc_data::loot::{
    self, BlockLootContext, LootContextItem, LootCount, LootDrop, LootEnchantments,
    LootRandomBinding,
};

fn id(name: &str) -> Identifier {
    Identifier::parse(format!("minecraft:{name}")).unwrap()
}

fn drops(block: &str, tool: &LootContextItem, seed: u64) -> Vec<LootDrop> {
    let table = loot::builtin();
    let block = id(block);
    table
        .evaluate_block(
            &BlockLootContext::try_new(
                &block,
                &[],
                tool,
                LootRandomBinding::new(table.block_random_sequence(&block).cloned(), seed),
            )
            .unwrap(),
        )
        .unwrap()
        .unwrap()
}

#[test]
fn bare_hand_plant_drops_have_vanilla_probabilities() {
    let hand = LootContextItem::empty();
    let mut grass_hits = 0;
    let mut leaves = BTreeMap::<Identifier, u32>::new();
    for seed in 0..65_536 {
        let grass = drops("short_grass", &hand, seed);
        if !grass.is_empty() {
            assert_eq!(grass, [LootDrop::single(id("wheat_seeds"))]);
            grass_hits += 1;
        }
        for drop in drops("oak_leaves", &hand, seed) {
            let LootCount::Fixed(count) = drop.count else {
                panic!("evaluated count")
            };
            *leaves.entry(drop.item).or_default() += count;
        }
    }
    // Fixed seed corpus, broad six-sigma bands around 1/8, 1/20, 1/200 and 2% × 1.5.
    assert!(
        (7_600..=8_800).contains(&grass_hits),
        "grass hits: {grass_hits}"
    );
    assert!((2_900..=3_650).contains(&leaves[&id("oak_sapling")]));
    assert!((210..=460).contains(&leaves[&id("apple")]));
    assert!((1_600..=2_350).contains(&leaves[&id("stick")]));
    assert_eq!(leaves.len(), 3);
}

#[test]
fn preservation_tools_suppress_leaf_loot_and_shears_preserve_grass() {
    let shears = LootContextItem::new(id("shears"));
    let silk = LootContextItem::new(id("diamond_hoe"))
        .with_enchantments(LootEnchantments::try_from_levels([(id("silk_touch"), 1)]).unwrap());
    for leaf in [
        "oak_leaves",
        "dark_oak_leaves",
        "jungle_leaves",
        "acacia_leaves",
        "birch_leaves",
        "spruce_leaves",
        "cherry_leaves",
        "azalea_leaves",
        "flowering_azalea_leaves",
        "mangrove_leaves",
        "pale_oak_leaves",
    ] {
        for seed in 0..32 {
            for tool in [&shears, &silk] {
                assert_eq!(drops(leaf, tool, seed), [LootDrop::single(id(leaf))]);
            }
        }
    }
    assert_eq!(
        drops("short_grass", &shears, 1),
        [LootDrop::single(id("short_grass"))]
    );
    assert!(
        drops("short_grass", &silk, 1)
            .iter()
            .all(|drop| drop.item == id("wheat_seeds"))
    );
}

#[test]
fn fortune_improves_chance_but_never_guarantees_seeds_or_apples() {
    let fortune = LootContextItem::new(id("diamond_hoe"))
        .with_enchantments(LootEnchantments::try_from_levels([(id("fortune"), 3)]).unwrap());
    let mut seed_hits = 0;
    let mut seed_count = 0;
    let mut apple_count = 0;
    for seed in 0..16_384 {
        for drop in drops("short_grass", &fortune, seed) {
            seed_hits += 1;
            let LootCount::Fixed(count) = drop.count else {
                panic!("evaluated count")
            };
            assert!((1..=7).contains(&count));
            seed_count += count;
        }
        apple_count += drops("oak_leaves", &fortune, seed)
            .iter()
            .filter(|drop| drop.item == id("apple"))
            .count();
    }
    assert!((1_750..=2_350).contains(&seed_hits));
    assert!((6_900..=9_500).contains(&seed_count));
    assert!((70..=210).contains(&apple_count));
}
