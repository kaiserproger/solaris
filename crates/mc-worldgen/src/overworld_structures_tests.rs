use super::*;
use mc_world::{ChunkGenerator, ChunkPos};
use std::sync::Arc;

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}
fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

#[test]
fn generated_fully_eyed_stronghold_activates_portal() {
    let registry = registry();
    let original = stronghold(&registry).unwrap();
    let eyes = stronghold_eye_states(&registry).unwrap();
    let blocks = original
        .blocks()
        .iter()
        .map(|block| {
            let mut block = *block;
            if let Some((_, filled)) = eyes.iter().find(|(empty, _)| *empty == block.state) {
                block.state = *filled;
            }
            block
        })
        .collect();
    let template = StructureTemplate::new(original.size(), blocks)
        .with_chests(original.chests().to_vec())
        .with_block_entities(original.block_entities().to_vec())
        .with_entities(original.entities().to_vec());
    let structures = OverworldStructures {
        seed: 42,
        strongholds: vec![(0, 0)],
        stronghold: template,
        stronghold_eye_states: eyes,
        stronghold_portal: state(&registry, "minecraft:end_portal").unwrap(),
        families: Vec::new(),
        features: Vec::new(),
        reach_before: [3, 2],
        reach_after: [0, 0],
    };
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        state(&registry, "minecraft:stone").unwrap(),
        id("minecraft:plains"),
    );
    structures.paste(
        &mut chunk,
        |_, _| 32,
        |_, _| 32,
        |_, _, _| id("minecraft:plains"),
    );
    let portal = state(&registry, "minecraft:end_portal").unwrap();
    for x in 6..=8 {
        for z in 6..=8 {
            assert_eq!(chunk.get_block(x, 3, z), Some(portal), "portal at {x},{z}");
        }
    }
}

#[test]
fn generated_stronghold_eye_patterns_vary_between_sites_in_one_world() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let structures = OverworldStructures {
        seed: 42,
        strongholds: (0..16).map(|x| (x * 10, 0)).collect(),
        stronghold: template.clone(),
        stronghold_eye_states: stronghold_eye_states(&registry).unwrap(),
        stronghold_portal: state(&registry, "minecraft:end_portal").unwrap(),
        families: Vec::new(),
        features: Vec::new(),
        reach_before: [3, 2],
        reach_after: [0, 0],
    };
    let mut patterns = HashSet::new();
    for x in 0..16 {
        let mut chunk = Chunk::empty(
            ChunkPos { x: x * 10, z: 0 },
            state(&registry, "minecraft:stone").unwrap(),
            id("minecraft:plains"),
        );
        structures.paste(
            &mut chunk,
            |_, _| 32,
            |_, _| 32,
            |_, _, _| id("minecraft:plains"),
        );
        let pattern: Vec<_> = template
            .blocks()
            .iter()
            .filter(|block| {
                registry.by_id(block.state).unwrap().block.id == id("minecraft:end_portal_frame")
            })
            .map(|block| {
                let [x, y, z] = block.pos;
                let state = chunk.get_block(x as u8, y, z as u8).unwrap();
                registry
                    .by_id(state)
                    .unwrap()
                    .properties
                    .iter()
                    .any(|(k, v)| k == "eye" && v == "true")
            })
            .collect();
        patterns.insert(pattern);
    }
    assert!(
        patterns.len() > 1,
        "strongholds in one world must not share a single eye pattern"
    );
}

#[test]
fn generated_stronghold_frame_eyes_are_independent_and_seeded() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let mut eyes = 0;
    let mut empty_rooms = 0;
    let mut mixed_rooms = 0;
    for seed in 0..256 {
        let structures = OverworldStructures {
            seed,
            strongholds: vec![(0, 0)],
            stronghold: template.clone(),
            stronghold_eye_states: stronghold_eye_states(&registry).unwrap(),
            stronghold_portal: state(&registry, "minecraft:end_portal").unwrap(),
            families: Vec::new(),
            features: Vec::new(),
            reach_before: [3, 2],
            reach_after: [0, 0],
        };
        let generate = || {
            let mut chunk = Chunk::empty(
                ChunkPos { x: 0, z: 0 },
                state(&registry, "minecraft:stone").unwrap(),
                id("minecraft:plains"),
            );
            structures.paste(
                &mut chunk,
                |_, _| 32,
                |_, _| 32,
                |_, _, _| id("minecraft:plains"),
            );
            chunk
        };
        let chunk = generate();
        let repeat = generate();
        let mut room_eyes = 0;
        for block in template.blocks().iter().filter(|block| {
            registry.by_id(block.state).unwrap().block.id == id("minecraft:end_portal_frame")
        }) {
            let [x, y, z] = block.pos;
            let actual = chunk.get_block(x as u8, y, z as u8).unwrap();
            assert_eq!(repeat.get_block(x as u8, y, z as u8), Some(actual));
            let actual = registry.by_id(actual).unwrap();
            let original = registry.by_id(block.state).unwrap();
            assert_eq!(actual.block.id, original.block.id);
            assert_eq!(
                actual.properties.iter().find(|(k, _)| k == "facing"),
                original.properties.iter().find(|(k, _)| k == "facing")
            );
            if actual
                .properties
                .iter()
                .any(|(k, v)| k == "eye" && v == "true")
            {
                room_eyes += 1;
            }
        }
        eyes += room_eyes;
        empty_rooms += usize::from(room_eyes == 0);
        mixed_rooms += usize::from((1..12).contains(&room_eyes));
    }
    assert!(
        (215..=400).contains(&eyes),
        "expected approximately 10% of 3072 eyes, got {eyes}"
    );
    assert!(
        empty_rooms > 20,
        "no forced guaranteed eyes: {empty_rooms} empty rooms"
    );
    assert!(
        mixed_rooms > 100,
        "frame outcomes must vary independently: {mixed_rooms} mixed rooms"
    );
}

#[test]
fn stronghold_portal_room_has_twelve_inward_frames_spawner_and_loot() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let frame = id("minecraft:end_portal_frame");
    let frames: Vec<_> = template
        .blocks()
        .iter()
        .filter(|b| registry.by_id(b.state).unwrap().block.id == frame)
        .collect();
    assert_eq!(frames.len(), 12);
    for block in frames {
        let state = registry.by_id(block.state).unwrap();
        let facing = state
            .properties
            .iter()
            .find(|(k, _)| k == "facing")
            .unwrap()
            .1
            .as_str();
        assert_eq!(
            facing,
            if block.pos[0] == 5 {
                "east"
            } else if block.pos[0] == 9 {
                "west"
            } else if block.pos[2] == 5 {
                "south"
            } else {
                "north"
            }
        );
    }
    assert_eq!(template.chests().len(), 2);
    assert_eq!(template.block_entities().len(), 1);
    let pos = BlockPos {
        x: -49,
        y: -20,
        z: 70,
    };
    let bytes = template.block_entities()[0].at_world_position(pos);
    let nbt = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
    let Tag::Compound(fields) = nbt else {
        panic!("compound")
    };
    assert!(fields.contains(&("x".into(), Tag::Int(-49))));
    assert!(fields.contains(&(
        "SpawnData".into(),
        Tag::Compound(vec![(
            "entity".into(),
            Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:silverfish".into())
            )])
        )])
    )));
}

#[test]
fn stronghold_clipped_paste_matches_whole_plan_and_preserves_entities() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let origin = BlockPos {
        x: -16,
        y: -30,
        z: -32,
    };
    let mut chunks = BTreeMap::new();
    for x in -1..=2 {
        for z in -2..=0 {
            let mut chunk = Chunk::empty(
                ChunkPos { x, z },
                state(&registry, "minecraft:stone").unwrap(),
                id("minecraft:plains"),
            );
            paste_template(&mut chunk, &template, origin, 42);
            chunks.insert((x, z), chunk);
        }
    }
    for block in template.blocks() {
        let (x, y, z) = (
            origin.x + block.pos[0],
            origin.y + block.pos[1],
            origin.z + block.pos[2],
        );
        let chunk = &chunks[&(x.div_euclid(16), z.div_euclid(16))];
        assert_eq!(
            chunk.get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8),
            Some(block.state)
        );
    }
    assert_eq!(chunks.values().map(|c| c.chests.len()).sum::<usize>(), 2);
    assert_eq!(
        chunks
            .values()
            .map(|c| c.block_entities.len())
            .sum::<usize>(),
        1
    );
}

#[test]
fn vanilla_catalog_loads_and_located_stronghold_generates_portal() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = Arc::new(registry());
    let mut generator = crate::TerrainGenerator::new(42, Arc::clone(&registry));
    generator.load_vanilla_worldgen(&root()).unwrap();
    let structures = generator.structure_sites_in_region((-180, -180), (180, 180));
    let site = structures
        .iter()
        .find(|s| s.structure == id("minecraft:stronghold"))
        .unwrap();
    let chunk = generator.generate(ChunkPos {
        x: site.start_chunk.0,
        z: site.start_chunk.1,
    });
    let frame = registry
        .by_id(chunk.get_block(6, site.position.y + 3, 5).unwrap())
        .unwrap();
    assert_eq!(frame.block.id, id("minecraft:end_portal_frame"));
    assert!(
        frame
            .properties
            .iter()
            .any(|(key, value)| key == "facing" && value == "south")
    );
    assert!(chunk.block_entities.contains_key(&BlockPos {
        x: site.position.x + 7,
        y: site.position.y + 1,
        z: site.position.z + 14
    }));
    assert!(
        mc_world::structure_starts::read(&chunk.extras, chunk.pos, false)
            .unwrap()
            .iter()
            .any(|saved| saved == site)
    );
}

#[test]
fn mineshaft_has_walkable_crossing_rails_cave_spiders_and_loot() {
    let registry = registry();
    let template = mineshaft(&registry).unwrap();
    assert!(template.chests().is_empty());
    assert!(
        template
            .entities()
            .iter()
            .any(|entity| entity.entity_type == "minecraft:chest_minecart")
    );
    assert_eq!(template.block_entities().len(), 1);
    assert!(template.blocks().iter().any(|b| b.pos==[24,1,24] && b.state==state(&registry,"minecraft:rail").unwrap()));
}

#[test]
fn vanilla_shipwreck_markers_become_loot_not_authoring_blocks() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = registry();
    let template = StructureTemplate::from_nbt_file(
        root().join("data/minecraft/structure/shipwreck/with_mast.nbt"),
        &registry,
    )
    .unwrap();
    let template = clean_template(template, "shipwreck", &registry).unwrap();
    assert!(!template.blocks().iter().any(|block| {
        registry.by_id(block.state).unwrap().block.id == id("minecraft:structure_block")
    }));
    assert!(
        template
            .chests()
            .iter()
            .all(|chest| chest.loot_table.is_some())
    );
    assert!(
        template
            .chests()
            .iter()
            .any(|chest| chest.loot_table == Some(id("minecraft:chests/shipwreck_map")))
    );
}

#[test]
fn classic_catalog_contains_every_family_with_interactive_contents() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = registry();
    for name in [
        "mineshaft",
        "mineshaft_mesa",
        "igloo",
        "shipwreck",
        "shipwreck_beached",
        "ocean_ruin_warm",
        "ocean_ruin_cold",
        "pillager_outpost",
        "desert_pyramid",
        "jungle_pyramid",
        "swamp_hut",
        "buried_treasure",
        "monument",
        "mansion",
        "ruined_portal",
    ] {
        let (templates, _) = catalog::templates(&root(), name, &registry).unwrap();
        assert!(!templates.is_empty(), "{name}");
        for t in templates {
            assert!(!t.blocks().is_empty(), "{name}");
            assert!(
                t.blocks()
                    .iter()
                    .all(|b| registry.by_id(b.state).unwrap().block.id
                        != id("minecraft:structure_block")),
                "{name}"
            );
            assert!(t.chests().iter().all(|c| c.loot_table.is_some()), "{name}");
            if name == "mineshaft" || name == "mineshaft_mesa" {
                assert!(
                    t.entities()
                        .iter()
                        .any(|entity| entity.entity_type == "minecraft:chest_minecart"),
                    "{name}"
                );
            }
        }
    }
    let (temples, _) = catalog::templates(&root(), "jungle_pyramid", &registry).unwrap();
    assert_eq!(temples[0].block_entities().len(), 2);
    for entity in temples[0].block_entities() {
        assert!(
            entity
                .nbt
                .contains(&("id".into(), Tag::String("minecraft:dispenser".into())))
        );
        assert!(entity.nbt.iter().any(|(key, value)| key == "Items"
            && matches!(value,Tag::List(list) if !list.elements.is_empty())));
    }
    let (monuments, _) = catalog::templates(&root(), "monument", &registry).unwrap();
    assert_eq!(
        monuments[0]
            .entities()
            .iter()
            .filter(|e| e.entity_type == "minecraft:elder_guardian" && e.persistent)
            .count(),
        3
    );
    let (igloos, _) = catalog::templates(&root(), "igloo", &registry).unwrap();
    assert_eq!(igloos.len(), 2);
    assert!(
        igloos[1].bounds().0[1] < 0,
        "basement participates in saved bounds"
    );
}

#[test]
fn generated_shipwreck_map_chest_preserves_unopened_table_and_seed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    assert!(root.exists(), "local vanilla assets required");
    let registry = registry();
    let (templates, _) = catalog::templates(&root, "shipwreck", &registry).unwrap();
    let (template, chest) = templates
        .iter()
        .find_map(|template| {
            template
                .chests()
                .iter()
                .find(|chest| {
                    chest
                        .loot_table
                        .as_ref()
                        .is_some_and(|id| id.as_str() == "minecraft:chests/shipwreck_map")
                })
                .map(|chest| (template, chest))
        })
        .expect("shipwreck map chest asset");
    let position = BlockPos {
        x: chest.pos[0],
        y: 64 + chest.pos[1],
        z: chest.pos[2],
    };
    let mut chunk = Chunk::empty(
        ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        },
        state(&registry, "minecraft:air").unwrap(),
        id("minecraft:ocean"),
    );
    paste_template(&mut chunk, template, BlockPos { x: 0, y: 64, z: 0 }, 42);
    let generated = &chunk.chests[&position];
    assert_eq!(
        generated.loot_table,
        Some(id("minecraft:chests/shipwreck_map"))
    );
    assert_eq!(
        generated.loot_seed,
        (hash(42, position.x, position.z) ^ position.y as u64) as i64
    );
    assert!(
        generated.slots.iter().all(|slot| slot.count == 0),
        "unopened, not prematurely rolled without map context"
    );
}

#[test]
fn igloo_basement_has_continuous_ladder_and_pastes_north_of_start() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = registry();
    let mut structures = OverworldStructures::load(&root, 42, &registry).unwrap();
    structures.strongholds.clear();
    structures.features.clear();
    structures
        .families
        .retain(|family| family.id.path() == "igloo");
    let family = &mut structures.families[0];
    let basement = family.templates.remove(1);
    for y in -22..0 {
        let block = basement
            .blocks()
            .iter()
            .find(|block| block.pos == [3, y, 5])
            .expect("continuous basement-to-trapdoor shaft");
        assert_eq!(
            registry.by_id(block.state).unwrap().block.id.path(),
            "ladder"
        );
    }
    let (cx, cz) = family.placement.potential_chunk(42, 0, 0);
    family.templates = vec![basement.clone()];
    assert!(family.biomes.contains(&id("minecraft:snowy_plains")));
    let mut chunk = Chunk::empty(
        ChunkPos { x: cx, z: cz - 1 },
        state(&registry, "minecraft:air").unwrap(),
        id("minecraft:snowy_plains"),
    );
    structures.paste(
        &mut chunk,
        |_, _| 64,
        |_, _| 64,
        |_, _, _| id("minecraft:snowy_plains"),
    );
    let mut checked = 0;
    for block in basement.blocks().iter().filter(|block| block.pos[2] < 0) {
        assert_eq!(
            chunk.get_block(
                block.pos[0] as u8,
                65 + block.pos[1],
                (16 + block.pos[2]) as u8
            ),
            Some(block.state)
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "basement crosses the negative-Z chunk boundary"
    );
}

#[test]
fn outpost_exclusion_rejects_nearby_village_placement_before_terrain_sampling() {
    let registry = registry();
    let mut structures = OverworldStructures::load(&root(), 42, &registry).unwrap();
    structures.strongholds.clear();
    structures
        .families
        .retain(|family| family.id.path() == "pillager_outpost");
    let family = &mut structures.families[0];
    assert_eq!(
        family.exclusion.unwrap().1,
        10,
        "local vanilla exclusion radius"
    );
    let grid = RandomSpreadPlacement {
        spacing: 100,
        separation: 99,
        salt: 1,
        triangular: false,
    };
    family.placement = grid;
    family.frequency = 1.0;
    family.exclusion = Some((grid, 10));
    assert!(
        structures
            .sites(
                (0, 0),
                (0, 0),
                |_, _| panic!("excluded start must not sample terrain"),
                |_, _| panic!("excluded start must not sample terrain"),
                |_, _, _| id("minecraft:plains")
            )
            .is_empty()
    );
    structures.families[0].exclusion = None;
    assert_eq!(
        structures
            .sites(
                (0, 0),
                (0, 0),
                |_, _| 64,
                |_, _| 64,
                |_, _, _| id("minecraft:plains")
            )
            .len(),
        1
    );
    assert!(near_placement(42, grid, 10, -90, 190));

    assert!(!near_placement(42, grid, 10, -89, 189));
}
#[test]
fn ocean_monument_start_is_underwater() {
    let registry = registry();
    let mut structures = OverworldStructures::load(&root(), 42, &registry).unwrap();
    structures.strongholds.clear();
    structures
        .families
        .retain(|family| family.id.path() == "monument");
    let family = &mut structures.families[0];
    family.frequency = 1.0;
    family.exclusion = None;
    family.biomes.insert(id("minecraft:deep_ocean"));
    let (x, z) = family.placement.potential_chunk(42, 0, 0);
    let sites = structures.sites(
        (x, z),
        (x, z),
        |_, _| 62,
        |_, _| 51,
        |_, _, _| id("minecraft:deep_ocean"),
    );
    assert_eq!(sites.len(), 1);
    assert!(
        sites[0].position.y + 16 < 63,
        "monument roof must be underwater"
    );
}

#[test]
fn ocean_wrecks_and_ruins_use_seabed_and_beached_wrecks_use_surface() {
    let registry = registry();
    for (name, expected) in [
        ("shipwreck", 52),
        ("ocean_ruin_cold", 52),
        ("ocean_ruin_warm", 52),
        ("shipwreck_beached", 63),
    ] {
        let mut structures = OverworldStructures::load(&root(), 42, &registry).unwrap();
        structures.strongholds.clear();
        structures
            .families
            .retain(|family| family.id.path() == name);
        let family = &mut structures.families[0];
        family.frequency = 1.0;
        family.exclusion = None;
        family.biomes.insert(id("minecraft:ocean"));
        let (x, z) = family.placement.potential_chunk(42, 0, 0);
        let sites = structures.sites(
            (x, z),
            (x, z),
            |_, _| 62,
            |_, _| 51,
            |_, _, _| id("minecraft:ocean"),
        );
        assert_eq!(sites.len(), 1, "{name}");
        assert_eq!(sites[0].position.y, expected, "{name}");
        let mut chunk = Chunk::empty(
            ChunkPos { x, z },
            state(&registry, "minecraft:stone").unwrap(),
            id("minecraft:ocean"),
        );
        structures.paste(
            &mut chunk,
            |_, _| 62,
            |_, _| 51,
            |_, _, _| id("minecraft:ocean"),
        );
        let saved = mc_world::structure_starts::read(&chunk.extras, chunk.pos, false).unwrap();
        assert!(saved.iter().any(|site| site == &sites[0]), "{name}");
    }
}

#[test]
fn mineshaft_places_chest_minecart_on_rail_in_owner_chunk() {
    let registry = registry();
    let template = mineshaft(&registry).unwrap();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 2, z: 1 },
        state(&registry, "minecraft:stone").unwrap(),
        id("minecraft:plains"),
    );
    paste_template(&mut chunk, &template, BlockPos { x: 0, y: 32, z: 0 }, 42);
    let markers = chunk.generated_entities();
    let cart = markers
        .iter()
        .find(|m| m.entity_type == "minecraft:chest_minecart")
        .expect("mineshaft cart");
    assert_eq!(
        chunk.get_block(8, 33, 9),
        Some(state(&registry, "minecraft:rail").unwrap())
    );
    assert_eq!(cart.position, [40.5, 33.0, 25.5]);
    assert!(chunk.chests.is_empty());
}
