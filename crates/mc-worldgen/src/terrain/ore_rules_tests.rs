use mc_world::BlockStateId;
use mc_world::chunk::{MAX_Y, MIN_Y, OVERWORLD_GEOMETRY};

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

    let error = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &features,
        None,
        OVERWORLD_GEOMETRY,
    )
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

#[test]
fn relative_height_anchors_use_short_and_tall_chunk_geometry() {
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    for geometry in [
        mc_world::ChunkGeometry::new(-32, 16).expect("one section"),
        mc_world::ChunkGeometry::new(-128, 496).expect("31 sections"),
    ] {
        for (offset, expected_max) in [
            (0, geometry.max_y() - 1),
            (1, geometry.max_y() - 2),
            (geometry.height() - 1, geometry.min_y()),
        ] {
            let mut feature = ore_feature(
                "minecraft:ore_iron",
                "minecraft:iron_ore",
                "minecraft:deepslate_iron_ore",
                0,
                0,
                4,
            );
            let height = feature.placement.height.as_mut().expect("height range");
            height.min = mc_data::worldgen_ores::HeightAnchor::AboveBottom(0);
            height.max = mc_data::worldgen_ores::HeightAnchor::BelowTop(offset);

            let rules =
                OreRules::from_features(registry.as_ref(), &biomes, &[feature], None, geometry)
                    .expect("bounded rules")
                    .expect("resolved rule");

            assert_eq!(rules.rules()[0].y.min, geometry.min_y());
            assert_eq!(rules.rules()[0].y.max, expected_max);
        }

        let mut below_dimension = ore_feature(
            "minecraft:ore_iron",
            "minecraft:iron_ore",
            "minecraft:deepslate_iron_ore",
            0,
            0,
            4,
        );
        let height = below_dimension
            .placement
            .height
            .as_mut()
            .expect("height range");
        height.min = mc_data::worldgen_ores::HeightAnchor::AboveBottom(0);
        height.max = mc_data::worldgen_ores::HeightAnchor::BelowTop(geometry.height());

        assert!(
            OreRules::from_features(
                registry.as_ref(),
                &biomes,
                &[below_dimension],
                None,
                geometry,
            )
            .expect("out-of-range top-relative rule is ignored")
            .is_none()
        );
    }
}

#[test]
fn trapezoid_spacing_uses_wide_midpoint_at_i32_endpoint() {
    let mut feature = ore_feature(
        "minecraft:ore_iron",
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore",
        0,
        0,
        4,
    );
    feature
        .placement
        .height
        .as_mut()
        .expect("height range")
        .kind = mc_data::Identifier::parse("minecraft:trapezoid").unwrap();
    let range = YRange::new(i32::MAX - 16, i32::MAX);

    let spacing = super::ore_spacing(&feature.placement, range);

    assert!(matches!(
        spacing,
        OreSpacing::Peaked { peak_y, .. } if peak_y == i32::MAX - 8
    ));
}

#[test]
fn peaked_spacing_uses_wide_distance_at_i32_endpoints() {
    assert_eq!(
        super::peaked_spacing(i32::MIN, i32::MIN, i32::MAX, 0, 11, 17),
        28
    );
}

#[test]
fn ore_features_outside_or_empty_for_geometry_are_skipped() {
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let geometry = mc_world::ChunkGeometry::new(0, 16).expect("one section");
    let below = ore_feature(
        "minecraft:ore_iron_below",
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore",
        -32,
        -16,
        4,
    );
    let above = ore_feature(
        "minecraft:ore_iron_above",
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore",
        16,
        32,
        4,
    );

    assert!(
        OreRules::from_features(registry.as_ref(), &biomes, &[below, above], None, geometry,)
            .expect("out-of-range rules are ignored")
            .is_none()
    );
    assert!(
        OreRules::from_features(registry.as_ref(), &biomes, &[], None, geometry)
            .expect("empty input")
            .is_none()
    );
}

#[test]
fn extreme_relative_height_offsets_are_rejected_without_panicking() {
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let geometry = mc_world::ChunkGeometry::new(0, 16).expect("one section");
    let mut feature = ore_feature(
        "minecraft:ore_iron",
        "minecraft:iron_ore",
        "minecraft:deepslate_iron_ore",
        0,
        0,
        4,
    );
    let height = feature.placement.height.as_mut().expect("height range");
    height.min = mc_data::worldgen_ores::HeightAnchor::AboveBottom(i32::MAX);
    height.max = mc_data::worldgen_ores::HeightAnchor::BelowTop(i32::MIN);

    assert!(
        OreRules::from_features(registry.as_ref(), &biomes, &[feature], None, geometry,)
            .expect("extreme offsets are ignored")
            .is_none()
    );
}
