use mc_world::BlockStateId;
use mc_world::chunk::{MAX_Y, MIN_Y};

use crate::terrain::TerrainGenerator;
use crate::terrain::tests::{ore_feature, tiny_registry};

use super::{
    BiomeRules, BiomeScope, MAX_ORE_RULES, MAX_ORE_VEIN_SIZE, MAX_ORE_WORK_UNITS_PER_CHUNK,
    OreRule, OreRules, OreRulesError, OreSpacing, YRange,
};

#[test]
fn default_ore_rules_keep_priority_order() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let rules = g.ores.rules();

    assert_eq!(rules.len(), 9);
    assert_eq!(rules[0].normal, BlockStateId(19));
    assert!(matches!(&rules[0].biomes, BiomeScope::Only(_)));
    assert_eq!(rules[1].normal, BlockStateId(15));
    assert!(matches!(&rules[1].spacing, OreSpacing::Fixed(58)));
    assert_eq!(rules[2].normal, BlockStateId(17));
    assert_eq!(rules[8].normal, BlockStateId(8));
}

#[test]
fn ore_rules_reject_more_than_the_admission_limit() {
    let rule = ore_feature(
        "minecraft:ore_iron",
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore",
        -32,
        32,
        4,
    );
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let features = vec![rule; MAX_ORE_RULES + 1];

    let error = OreRules::from_features(registry.as_ref(), &biomes, &features, None)
        .expect_err("oversized sidecar must be rejected instead of truncated");

    assert_eq!(
        error,
        OreRulesError::TooManyRules {
            provided: MAX_ORE_RULES + 1,
            max: MAX_ORE_RULES,
        }
    );
}

#[test]
fn ore_rules_reject_excessive_total_chunk_work() {
    let expensive_rule = OreRule {
        normal: BlockStateId(8),
        deepslate: BlockStateId(13),
        y: YRange::new(MIN_Y, MAX_Y - 1),
        spacing: OreSpacing::Fixed(1),
        biomes: BiomeScope::Any,
        size: MAX_ORE_VEIN_SIZE as u32,
        discard_chance_on_air_exposure: 0.0,
    };

    let error = OreRules::new(vec![expensive_rule; MAX_ORE_RULES])
        .expect_err("rules over the chunk-work budget must be rejected");

    assert!(matches!(
        error,
        OreRulesError::ChunkWorkBudgetExceeded { max, .. }
            if max == MAX_ORE_WORK_UNITS_PER_CHUNK
    ));
}

#[test]
fn ore_rules_admit_an_ordinary_bounded_set() {
    let ordinary_rule = OreRule {
        normal: BlockStateId(8),
        deepslate: BlockStateId(13),
        y: YRange::new(-32, 32),
        spacing: OreSpacing::Fixed(32),
        biomes: BiomeScope::Any,
        size: 8,
        discard_chance_on_air_exposure: 0.0,
    };

    let rules =
        OreRules::new(vec![ordinary_rule; 16]).expect("ordinary ore rules must remain admitted");

    assert_eq!(rules.rules().len(), 16);
}
