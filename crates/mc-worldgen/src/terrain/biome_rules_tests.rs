use std::collections::{BTreeMap, BTreeSet};

use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;
use mc_world::ChunkGenerator;

use crate::terrain::tests::tiny_registry;
use crate::terrain::{SEA_LEVEL, TerrainGenerator};

use super::BiomeRules;

#[test]
fn inland_river_bank_retains_a_grass_surface() {
    let g = TerrainGenerator::with_worldgen_mode(
        712_816,
        tiny_registry(),
        super::super::WorldgenMode::TellusLike(Default::default()),
    );
    let (x, z) = (-252_i32, 59_i32);
    let chunk = g.generate(mc_world::ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    });
    assert_eq!(
        chunk.get_block(
            x.rem_euclid(16) as u8,
            g.surface_height(x, z),
            z.rem_euclid(16) as u8,
        ),
        Some(g.grass_block),
        "an exposed inland river bank became a beach",
    );
}

#[test]
fn raised_coastal_land_stays_grass_while_rocky_shore_keeps_gravel() {
    let g = TerrainGenerator::with_worldgen_mode(
        -17_711,
        tiny_registry(),
        super::super::WorldgenMode::TellusLike(Default::default()),
    );
    for (x, z, expected) in [(-195_i32, 781_i32, g.grass_block), (-187, 811, g.gravel)] {
        let chunk = g.generate(mc_world::ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        });
        assert_eq!(
            chunk.get_block(
                x.rem_euclid(16) as u8,
                g.surface_height(x, z),
                z.rem_euclid(16) as u8,
            ),
            Some(expected),
            "coastal surface at ({x}, {z})",
        );
    }
}

#[test]
fn every_overworld_biome_is_reachable_by_selector() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let expected: BTreeSet<_> = g.biomes.all.iter().map(Identifier::as_str).collect();
    let mut seen = BTreeSet::new();

    'search: for x in (-16_384..=16_384).step_by(64) {
        for z in (-16_384..=16_384).step_by(64) {
            let surface = g.surface_height(x, z);
            seen.insert(g.biome_for_cell(x, surface, z, surface).to_string());
            seen.insert(g.biome_for(x, z, SEA_LEVEL - 20).to_string());
            seen.insert(
                g.biome_for_cell(x, surface.saturating_sub(32), z, surface)
                    .to_string(),
            );
            if expected.iter().all(|biome| seen.contains(*biome)) {
                break 'search;
            }
        }
    }

    let missing = expected
        .into_iter()
        .filter(|biome| !seen.contains(*biome))
        .collect::<Vec<_>>();
    assert!(missing.is_empty(), "selector never emitted {missing:?}");
}

#[test]
fn subtype_picker_includes_world_seed_identity() {
    let rules = BiomeRules::vanilla_overworld();
    let subtype_fingerprint = |seed| {
        (-4_096..=4_096)
            .step_by(256)
            .map(|x| {
                rules
                    .pick(&rules.hot_dry, x, x / 3, seed, 0x484F_5444)
                    .to_string()
            })
            .collect::<Vec<_>>()
    };
    assert_ne!(subtype_fingerprint(0), subtype_fingerprint(712_816));
}

#[test]
fn biome_rules_can_use_sidecar_tags() {
    let data = BiomeWorldgenData::from_parts(
        BTreeMap::from([
            (Identifier::parse("minecraft:plains").unwrap(), Vec::new()),
            (Identifier::parse("minecraft:forest").unwrap(), Vec::new()),
            (Identifier::parse("minecraft:badlands").unwrap(), Vec::new()),
            (Identifier::parse("minecraft:ocean").unwrap(), Vec::new()),
            (
                Identifier::parse("minecraft:deep_ocean").unwrap(),
                Vec::new(),
            ),
        ]),
        BTreeMap::from([
            (
                Identifier::parse("minecraft:is_overworld").unwrap(),
                vec![
                    Identifier::parse("minecraft:plains").unwrap(),
                    Identifier::parse("minecraft:forest").unwrap(),
                    Identifier::parse("minecraft:badlands").unwrap(),
                    Identifier::parse("minecraft:ocean").unwrap(),
                    Identifier::parse("minecraft:deep_ocean").unwrap(),
                ],
            ),
            (
                Identifier::parse("minecraft:is_forest").unwrap(),
                vec![Identifier::parse("minecraft:forest").unwrap()],
            ),
            (
                Identifier::parse("minecraft:is_badlands").unwrap(),
                vec![Identifier::parse("minecraft:badlands").unwrap()],
            ),
            (
                Identifier::parse("minecraft:is_ocean").unwrap(),
                vec![
                    Identifier::parse("minecraft:ocean").unwrap(),
                    Identifier::parse("minecraft:deep_ocean").unwrap(),
                ],
            ),
            (
                Identifier::parse("minecraft:is_deep_ocean").unwrap(),
                vec![Identifier::parse("minecraft:deep_ocean").unwrap()],
            ),
        ]),
    );

    let rules = BiomeRules::from_worldgen_data(&data).expect("is_overworld tag is present");

    assert!(
        rules
            .all
            .contains(&Identifier::parse("minecraft:plains").unwrap())
    );
    assert!(
        rules
            .temperate_forest
            .contains(&Identifier::parse("minecraft:forest").unwrap())
    );
    assert!(
        rules
            .hot_dry
            .contains(&Identifier::parse("minecraft:badlands").unwrap())
    );
    assert!(
        rules
            .ocean
            .contains(&Identifier::parse("minecraft:ocean").unwrap())
    );
    assert!(
        rules
            .deep_ocean
            .contains(&Identifier::parse("minecraft:deep_ocean").unwrap())
    );
}
