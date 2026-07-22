use mc_data::Identifier;
use mc_world::BlockStateId;
use mc_world::chunk::{MAX_Y, MIN_Y, OVERWORLD_GEOMETRY};

use crate::terrain::TerrainGenerator;
use crate::terrain::tests::{ore_feature, tiny_registry};

use super::{
    BiomeRules, BiomeScope, EMERALD_ORE_BIOMES, EXTRA_GOLD_BIOMES, EmbeddedOreDistribution,
    MAX_ORE_RULES, MAX_ORE_VEIN_SIZE, MAX_ORE_WORK_UNITS_PER_CHUNK, OreRule, OreRules,
    OreRulesError, OreSpacing, VANILLA_OVERWORLD_ORE_PASSES, YRange,
};

#[test]
fn default_ore_rules_keep_all_vanilla_overworld_passes() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let rules = g.ores.rules();

    assert_eq!(rules.len(), 18);
    for (state, expected_passes) in [
        (BlockStateId(8), 2),
        (BlockStateId(9), 3),
        (BlockStateId(10), 1),
        (BlockStateId(15), 3),
        (BlockStateId(16), 2),
        (BlockStateId(17), 4),
        (BlockStateId(18), 2),
        (BlockStateId(19), 1),
    ] {
        assert_eq!(
            rules.iter().filter(|rule| rule.normal == state).count(),
            expected_passes
        );
    }
    assert_eq!(
        rules
            .iter()
            .filter(|rule| matches!(rule.biomes, BiomeScope::Only(_)))
            .count(),
        2
    );
    assert!(rules.iter().any(|rule| {
        rule.normal == BlockStateId(17)
            && rule.size == 4
            && matches!(
                rule.spacing,
                OreSpacing::Trapezoid {
                    raw_min: -144,
                    raw_max: 16,
                    ..
                }
            )
    }));
    assert!(rules.iter().any(|rule| {
        rule.normal == BlockStateId(9) && rule.y.min == 80 && rule.y.max == MAX_Y - 1
    }));
}

#[test]
fn embedded_ore_passes_match_local_vanilla_2612_data_when_available() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/vanilla/data/minecraft/worldgen");
    if !root.join("placed_feature").is_dir() || !root.join("configured_feature").is_dir() {
        return;
    }
    let features = mc_data::worldgen_ores::load_ore_features(&root).expect("local ore sidecars");

    for pass in VANILLA_OVERWORLD_ORE_PASSES {
        let feature = features
            .iter()
            .find(|feature| feature.placed_feature.as_str() == pass.placed_feature)
            .unwrap_or_else(|| panic!("missing local feature {}", pass.placed_feature));
        let height = feature
            .placement
            .height
            .as_ref()
            .unwrap_or_else(|| panic!("missing height for {}", pass.placed_feature));
        assert_eq!(height.min, pass.min, "{} minimum", pass.placed_feature);
        assert_eq!(height.max, pass.max, "{} maximum", pass.placed_feature);
        assert_eq!(
            height.kind.as_str() == "minecraft:trapezoid",
            matches!(pass.distribution, EmbeddedOreDistribution::Trapezoid),
            "{} distribution",
            pass.placed_feature
        );
        let (attempts_numerator, mut attempts_denominator) = match feature.placement.count {
            Some(mc_data::worldgen_ores::OrePlacementCount::Constant(count)) => (count, 1),
            Some(mc_data::worldgen_ores::OrePlacementCount::Uniform { min, max }) => (min + max, 2),
            None => (1, 1),
        };
        if let Some(chance) = feature.placement.rarity_chance {
            attempts_denominator *= chance;
        }
        assert_eq!(
            (attempts_numerator, attempts_denominator),
            (pass.attempts_numerator, pass.attempts_denominator),
            "{} attempts",
            pass.placed_feature
        );
        assert_eq!(feature.size, pass.size, "{} size", pass.placed_feature);
        assert_eq!(
            feature.discard_chance_on_air_exposure.to_bits(),
            pass.discard_chance_on_air_exposure.to_bits(),
            "{} discard chance",
            pass.placed_feature
        );
        for expected in [pass.normal, pass.deepslate] {
            assert!(
                feature
                    .targets
                    .iter()
                    .any(|target| target.state.as_str() == expected),
                "{} missing target {expected}",
                pass.placed_feature
            );
        }
    }

    let minecraft_root = root.parent().expect("worldgen parent");
    let biome_data = mc_data::biomes::load_biome_worldgen_data(
        root.join("biome"),
        minecraft_root.join("tags/worldgen/biome"),
    )
    .expect("local biome sidecars");
    for (feature, expected) in [
        ("minecraft:ore_emerald", EMERALD_ORE_BIOMES),
        ("minecraft:ore_gold_extra", EXTRA_GOLD_BIOMES),
    ] {
        let feature = Identifier::parse(feature).unwrap();
        let actual = biome_data.biomes_for_feature(&feature);
        let expected = expected
            .iter()
            .map(|biome| Identifier::parse(*biome).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{feature} biome scope");
    }
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
fn trapezoid_peak_uses_unclipped_vanilla_anchor_midpoint() {
    let mut feature = ore_feature(
        "minecraft:ore_diamond",
        "minecraft:diamond_ore",
        "minecraft:deepslate_diamond_ore",
        0,
        0,
        7,
    );
    let height = feature.placement.height.as_mut().expect("height range");
    height.kind = mc_data::Identifier::parse("minecraft:trapezoid").unwrap();
    height.min = mc_data::worldgen_ores::HeightAnchor::AboveBottom(-80);
    height.max = mc_data::worldgen_ores::HeightAnchor::AboveBottom(80);
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let rules = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &[feature],
        None,
        OVERWORLD_GEOMETRY,
    )
    .unwrap()
    .unwrap();
    let rule = &rules.rules()[0];

    assert_eq!(rule.y.min, MIN_Y);
    assert_eq!(rule.y.max, 16);
    let OreSpacing::Trapezoid {
        raw_min,
        raw_max,
        average_spacing,
    } = rule.spacing
    else {
        panic!("vanilla trapezoid must retain its source anchors");
    };
    assert_eq!((raw_min, raw_max), (-144, 16));
    assert!(rule.spacing.at_y(-64, rule.y) < rule.spacing.at_y(0, rule.y));
    assert!(rule.spacing.at_y(-64, rule.y) < rule.spacing.at_y(16, rule.y));

    let actual_density = (raw_min..=raw_max)
        .map(|y| 1.0 / rule.spacing.at_y(y as i32, rule.y) as f64)
        .sum::<f64>();
    let expected_density = (raw_max - raw_min + 1) as f64 / average_spacing as f64;
    assert!(
        (actual_density / expected_density - 1.0).abs() < 0.02,
        "trapezoid density {actual_density} must preserve expected density {expected_density}"
    );
}

#[test]
fn embedded_scoped_ores_use_exact_vanilla_biome_lists() {
    let g = TerrainGenerator::new(42, tiny_registry());
    for (normal, expected) in [
        (BlockStateId(19), EMERALD_ORE_BIOMES),
        (BlockStateId(15), EXTRA_GOLD_BIOMES),
    ] {
        let scoped = g
            .ores
            .rules()
            .iter()
            .find(|rule| rule.normal == normal && matches!(rule.biomes, BiomeScope::Only(_)))
            .expect("scoped ore pass");
        let BiomeScope::Only(actual) = &scoped.biomes else {
            unreachable!();
        };
        let actual = actual.iter().map(Identifier::as_str).collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn rarity_filter_reduces_ore_density_instead_of_becoming_count_one() {
    let mut common = ore_feature(
        "minecraft:ore_diamond_common",
        "minecraft:diamond_ore",
        "minecraft:deepslate_diamond_ore",
        -64,
        16,
        1,
    );
    common.placement.count = None;
    let mut rare = common.clone();
    rare.placed_feature = mc_data::Identifier::parse("minecraft:ore_diamond_rare").unwrap();
    rare.placement.rarity_chance = Some(9);
    let registry = tiny_registry();
    let biomes = BiomeRules::vanilla_overworld();
    let common = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &[common],
        None,
        OVERWORLD_GEOMETRY,
    )
    .unwrap()
    .unwrap();
    let rare = OreRules::from_features(
        registry.as_ref(),
        &biomes,
        &[rare],
        None,
        OVERWORLD_GEOMETRY,
    )
    .unwrap()
    .unwrap();

    assert!(rare.rules()[0].spacing.minimum() >= common.rules()[0].spacing.minimum() * 8);
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
