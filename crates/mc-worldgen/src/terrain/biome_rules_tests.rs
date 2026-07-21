use std::collections::{BTreeMap, BTreeSet};

use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;

use crate::terrain::TerrainGenerator;
use crate::terrain::tests::tiny_registry;

use super::BiomeRules;

#[test]
fn every_overworld_biome_is_reachable_by_selector() {
    let g = TerrainGenerator::new(42, tiny_registry());
    let expected: BTreeSet<_> = g.biomes.all.iter().map(Identifier::as_str).collect();
    let mut seen = BTreeSet::new();

    let buckets = [
        &g.biomes.ocean,
        &g.biomes.beach,
        &g.biomes.river,
        &g.biomes.swamp,
        &g.biomes.cold,
        &g.biomes.temperate_forest,
        &g.biomes.grassland,
        &g.biomes.hot_dry,
        &g.biomes.mountain,
        &g.biomes.jungle,
        &g.biomes.cave,
    ];
    for (bucket_index, bucket) in buckets.into_iter().enumerate() {
        for x in (-4096..=4096).step_by(64) {
            for z in (-4096..=4096).step_by(64) {
                seen.insert(
                    g.biomes
                        .pick(bucket, x, z, 0x1000 + bucket_index as u64)
                        .as_str()
                        .to_string(),
                );
            }
        }
    }
    for x in (-4096..=4096).step_by(32) {
        for z in (-4096..=4096).step_by(32) {
            seen.insert(
                g.biomes
                    .pick_region_band(&g.biomes.deep_ocean, x, z)
                    .as_str()
                    .to_string(),
            );
        }
    }

    for biome in expected {
        assert!(seen.contains(biome), "selector never emitted {biome}");
    }
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
