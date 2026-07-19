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

    for seed_offset in 0..4 {
        let g = TerrainGenerator::new(42 + seed_offset, tiny_registry());
        for wx in (-4096..=4096).step_by(32) {
            for wz in (-4096..=4096).step_by(32) {
                let height = g.surface_height(wx, wz);
                let biome = g.biome_for(wx, wz, height);
                seen.insert(biome.as_str().to_string());
                for y in (-48..=48).step_by(16) {
                    let biome = g.biome_for_cell(wx, y, wz, height);
                    seen.insert(biome.as_str().to_string());
                }
            }
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
            .overworld_ids()
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
