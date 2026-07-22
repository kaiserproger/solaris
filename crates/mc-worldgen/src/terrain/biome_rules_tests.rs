use std::collections::{BTreeMap, BTreeSet};

use mc_data::Identifier;
use mc_data::biomes::BiomeWorldgenData;

use crate::terrain::tests::tiny_registry;
use crate::terrain::{SEA_LEVEL, TerrainGenerator};

use super::BiomeRules;

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
