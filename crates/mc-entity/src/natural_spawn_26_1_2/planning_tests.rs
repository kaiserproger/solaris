use mc_data::{
    Identifier, biomes::solaris_required_biome_spawn_rules,
    entity_types::solaris_required_entity_types,
};
use mc_world::{BlockStateId, Chunk, ChunkPos};

use super::{entity_aabb, entity_aabbs_intersect, plan_water_group_spawns};

#[test]
fn aquatic_packs_populate_surface_water_without_overlapping() {
    let air = BlockStateId(0);
    let water = BlockStateId(1);
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        air,
        Identifier::parse("minecraft:cold_ocean").unwrap(),
    );
    for x in 0..16 {
        for z in 0..16 {
            for y in 20..=63 {
                chunk.set_block(x, y, z, water).unwrap();
            }
        }
    }
    let rules = solaris_required_biome_spawn_rules();
    let types = solaris_required_entity_types();
    let mut spawns = Vec::new();
    for group in ["water_ambient", "water_creature"] {
        plan_water_group_spawns(&chunk, &[water], group, &rules, &types, 63, &mut spawns);
    }
    assert!(
        spawns
            .iter()
            .any(|spawn| spawn.entity_type_name == "minecraft:cod")
    );
    assert!(
        spawns
            .iter()
            .any(|spawn| spawn.entity_type_name == "minecraft:squid")
    );
    for (index, spawn) in spawns.iter().enumerate() {
        let bounds = entity_aabb(&spawn.entity_type_name);
        assert!(
            spawn.position.y >= 59.0 && spawn.position.y + bounds.height <= 64.0,
            "initial aquatic packs must be submerged in the surface-water band: {spawn:?}"
        );
        for other in &spawns[..index] {
            assert_ne!(
                spawn.slot, other.slot,
                "aquatic groups must not reuse entity identities"
            );
            assert!(
                !entity_aabbs_intersect(
                    spawn.position,
                    bounds,
                    other.position,
                    entity_aabb(&other.entity_type_name)
                ),
                "aquatic pack members overlap"
            );
        }
    }
}

#[test]
fn land_packs_keep_animals_apart_without_losing_members_at_chunk_edges() {
    let air = BlockStateId(0);
    let grass = BlockStateId(1);
    let mut chunk = Chunk::empty(
        ChunkPos { x: -1, z: 2 },
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 64, z, grass).unwrap();
        }
    }
    let mut spawns = Vec::new();
    super::plan_group_spawns(
        &chunk,
        super::LandSpawnSurfaces {
            preferred: grass,
            fallbacks: &[],
        },
        &[air],
        "creature",
        &solaris_required_biome_spawn_rules(),
        &solaris_required_entity_types(),
        &mut spawns,
    );
    assert_eq!(
        spawns.len(),
        4,
        "a flat chunk must accommodate a complete land herd"
    );
    for (index, spawn) in spawns.iter().enumerate() {
        for other in &spawns[..index] {
            let separation =
                (spawn.position.x - other.position.x).hypot(spawn.position.z - other.position.z);
            assert!(
                separation >= 3.0,
                "herd starts crowded: {separation} blocks"
            );
        }
    }
}

#[test]
fn monster_rules_honor_weights_and_counts_beyond_the_third_entry() {
    use mc_data::biomes::{BiomeSpawnEntry, BiomeSpawnRules};
    use std::collections::BTreeMap;
    let air = BlockStateId(0);
    let grass = BlockStateId(1);
    let biome = Identifier::parse("minecraft:plains").unwrap();
    let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, biome.clone());
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 64, z, grass).unwrap();
        }
    }
    let mut entries = ["zombie", "skeleton", "creeper", "witch"].map(|name| BiomeSpawnEntry {
        entity_type: Identifier::parse(format!("minecraft:{name}")).unwrap(),
        min_count: if name == "witch" { 5 } else { 1 },
        max_count: if name == "witch" { 5 } else { 1 },
        weight: if name == "witch" { 10_000 } else { 1 },
    });
    let plan = |entries: &[BiomeSpawnEntry]| {
        let rules = BiomeSpawnRules::from_entries(BTreeMap::from([(
            biome.clone(),
            BTreeMap::from([("monster".to_owned(), entries.to_vec())]),
        )]));
        let mut spawns = Vec::new();
        super::plan_hostile_spawns(
            &chunk,
            super::LandSpawnSurfaces {
                preferred: grass,
                fallbacks: &[],
            },
            &[air],
            &rules,
            &solaris_required_entity_types(),
            &mut spawns,
        );
        spawns
    };
    let witches = plan(&entries);
    assert_eq!(witches.len(), 5);
    assert!(
        witches
            .iter()
            .all(|spawn| spawn.entity_type_name == "minecraft:witch")
    );
    entries[0].weight = 10_000;
    entries[3].weight = 1;
    let zombies = plan(&entries);
    assert_eq!(zombies.len(), 1);
    assert_eq!(zombies[0].entity_type_name, "minecraft:zombie");
}
