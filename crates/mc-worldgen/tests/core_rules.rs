use std::collections::BTreeSet;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::worldgen_ores::{
    HeightAnchor, HeightRange, OreFeature, OrePlacement, OrePlacementCount, OreTarget,
};
use mc_world::{
    BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkPos, MAX_Y, OVERWORLD_GEOMETRY,
};
use mc_worldgen::{
    BiomeRules, BiomeScope, OreRule, OreRules, OreSpacing, TerrainGenerator, YRange,
};

fn generator() -> (TerrainGenerator, Arc<BlockRegistry>, BiomeRules) {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let biomes = BiomeRules::vanilla_overworld();
    let generator =
        TerrainGenerator::try_with_biome_rules(42, Arc::clone(&registry), biomes.clone()).unwrap();
    (generator, registry, biomes)
}

fn default_state(registry: &BlockRegistry, name: &str) -> BlockStateId {
    registry
        .block(&Identifier::parse(name).unwrap())
        .unwrap_or_else(|| panic!("missing required block {name}"))
        .default
}

fn optional_default_state(registry: &BlockRegistry, name: &str) -> Option<BlockStateId> {
    registry
        .block(&Identifier::parse(name).unwrap())
        .map(|block| block.default)
}

fn block_name(registry: &BlockRegistry, state: BlockStateId) -> &str {
    registry.by_id(state).unwrap().block.id.as_str()
}

fn is_log(registry: &BlockRegistry, state: BlockStateId) -> bool {
    block_name(registry, state).ends_with("_log")
}

fn is_leaves(registry: &BlockRegistry, state: BlockStateId) -> bool {
    block_name(registry, state).ends_with("_leaves")
}

fn chunk_has_ocean_biome(chunk: &Chunk) -> bool {
    chunk
        .biomes
        .iter()
        .flat_map(|section| section.palette())
        .any(|biome| biome.as_str().contains("ocean"))
}

#[test]
fn ocean_water_columns_contain_aquatic_vegetation_without_land_debris() {
    let (generator, registry, _) = generator();
    let air = default_state(&registry, "minecraft:air");
    let water = default_state(&registry, "minecraft:water");
    let aquatic_vegetation = [
        optional_default_state(&registry, "minecraft:seagrass"),
        optional_default_state(&registry, "minecraft:tall_seagrass"),
        optional_default_state(&registry, "minecraft:kelp_plant"),
        optional_default_state(&registry, "minecraft:kelp"),
    ]
    .into_iter()
    .flatten()
    .collect::<BTreeSet<_>>();

    let mut ocean_chunks = 0;
    let mut water_columns = 0;
    let mut saw_aquatic_vegetation = false;

    for cx in -24..=24 {
        for cz in -24..=24 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            if !chunk_has_ocean_biome(&chunk) {
                continue;
            }
            ocean_chunks += 1;

            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    let wx = cx * 16 + i32::from(lx);
                    let wz = cz * 16 + i32::from(lz);
                    let floor_y = generator.surface_height(wx, wz);
                    let mut saw_water_in_column = false;

                    for y in (floor_y + 1)..=(floor_y + 24).min(MAX_Y - 1) {
                        let state = chunk.get_block(lx, y, lz).unwrap_or(air);
                        if state == air {
                            if saw_water_in_column {
                                break;
                            }
                            continue;
                        }
                        if state == water {
                            saw_water_in_column = true;
                            continue;
                        }
                        if aquatic_vegetation.contains(&state) {
                            saw_aquatic_vegetation = true;
                            continue;
                        }
                        if saw_water_in_column {
                            panic!(
                                "unexpected block {} inside ocean water column at ({},{},{})",
                                block_name(&registry, state),
                                wx,
                                y,
                                wz
                            );
                        }
                    }

                    if saw_water_in_column {
                        water_columns += 1;
                    }
                    if saw_aquatic_vegetation && water_columns >= 32 {
                        return;
                    }
                }
            }
        }
    }

    assert!(
        ocean_chunks > 0,
        "sample window should contain ocean chunks"
    );
    assert!(
        water_columns > 0,
        "ocean chunks should contain water columns"
    );
    assert!(
        saw_aquatic_vegetation,
        "sampled ocean water should contain seagrass or kelp"
    );
}

#[test]
fn sampled_land_contains_tree_shapes() {
    let (generator, registry, _) = generator();
    let air = default_state(&registry, "minecraft:air");
    let mut found_tree = false;

    'chunks: for cx in -4..=4 {
        for cz in -4..=4 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            for lx in 2..=13u8 {
                for lz in 2..=13u8 {
                    let wx = cx * 16 + i32::from(lx);
                    let wz = cz * 16 + i32::from(lz);
                    let surface_y = generator.surface_height(wx, wz);
                    for base_y in (surface_y + 1)..=(surface_y + 8).min(MAX_Y - 1) {
                        let Some(base) = chunk.get_block(lx, base_y, lz) else {
                            continue;
                        };
                        if !is_log(&registry, base) {
                            continue;
                        }

                        let trunk_height = (0..8)
                            .take_while(|dy| {
                                chunk
                                    .get_block(lx, base_y + dy, lz)
                                    .is_some_and(|state| is_log(&registry, state))
                            })
                            .count();
                        if trunk_height < 3 {
                            continue;
                        }

                        let canopy_y = base_y + trunk_height as i32;
                        let has_canopy = (-2..=2).any(|dx| {
                            (-2..=2).any(|dz| {
                                (0..=2).any(|dy| {
                                    let x = i32::from(lx) + dx;
                                    let z = i32::from(lz) + dz;
                                    if !(0..16).contains(&x) || !(0..16).contains(&z) {
                                        return false;
                                    }
                                    chunk
                                        .get_block(x as u8, canopy_y + dy, z as u8)
                                        .is_some_and(|state| is_leaves(&registry, state))
                                })
                            })
                        });
                        if has_canopy {
                            assert_ne!(
                                chunk.get_block(lx, base_y - 1, lz),
                                Some(air),
                                "tree trunk is unsupported at {wx},{base_y},{wz}"
                            );
                            found_tree = true;
                            break 'chunks;
                        }
                    }
                }
            }
        }
    }

    assert!(
        found_tree,
        "sampled land chunks should contain at least one log trunk with a leaf canopy"
    );
}

#[test]
fn default_seed_spawn_window_contains_basic_playable_resources() {
    let registry = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let generator = TerrainGenerator::try_with_biome_rules(
        0,
        Arc::clone(&registry),
        BiomeRules::vanilla_overworld(),
    )
    .unwrap();
    let air = default_state(&registry, "minecraft:air");
    let stone = default_state(&registry, "minecraft:stone");
    let iron_ore = default_state(&registry, "minecraft:iron_ore");
    let deepslate_iron_ore = default_state(&registry, "minecraft:deepslate_iron_ore");
    let mut found_tree = false;
    let mut found_shallow_stone = false;
    let mut found_exposed_stone = false;
    let mut starter_stone_count = 0;
    let mut exposed_iron_count = 0;

    for cx in -4..=4 {
        for cz in -4..=4 {
            let chunk = generator.generate(ChunkPos { x: cx, z: cz });
            for lx in 2..=13u8 {
                for lz in 2..=13u8 {
                    let wx = cx * 16 + i32::from(lx);
                    let wz = cz * 16 + i32::from(lz);
                    if (wx * wx + wz * wz) > 64 * 64 {
                        continue;
                    }
                    let surface_y = generator.surface_height(wx, wz);
                    for y in (surface_y - 8).max(mc_world::MIN_Y)..=surface_y {
                        found_shallow_stone |= chunk.get_block(lx, y, lz) == Some(stone);
                        let exposed_stone = chunk.get_block(lx, y, lz) == Some(stone)
                            && y + 1 < MAX_Y
                            && chunk.get_block(lx, y + 1, lz).unwrap_or(air) == air;
                        found_exposed_stone |= exposed_stone;
                        if exposed_stone && (8..=11).contains(&wx) && (4..=8).contains(&wz) {
                            starter_stone_count += 1;
                        }
                        if matches!(
                            chunk.get_block(lx, y, lz),
                            Some(state) if state == iron_ore || state == deepslate_iron_ore
                        ) && y + 1 < MAX_Y
                            && chunk.get_block(lx, y + 1, lz).unwrap_or(air) == air
                        {
                            exposed_iron_count += 1;
                        }
                    }
                    for base_y in (surface_y + 1)..=(surface_y + 8).min(MAX_Y - 1) {
                        let Some(base) = chunk.get_block(lx, base_y, lz) else {
                            continue;
                        };
                        if !is_log(&registry, base) {
                            continue;
                        }

                        let trunk_height = (0..8)
                            .take_while(|dy| {
                                chunk
                                    .get_block(lx, base_y + dy, lz)
                                    .is_some_and(|state| is_log(&registry, state))
                            })
                            .count();
                        if trunk_height >= 3 {
                            found_tree = true;
                        }
                    }
                    if found_tree
                        && found_shallow_stone
                        && found_exposed_stone
                        && starter_stone_count >= 11
                        && exposed_iron_count >= 10
                    {
                        return;
                    }
                }
            }
        }
    }

    assert!(
        found_tree,
        "default playable seed should spawn with a harvestable tree within 64 blocks"
    );
    assert!(
        found_shallow_stone,
        "default playable seed should spawn with shallow stone within 64 blocks"
    );
    assert!(
        found_exposed_stone,
        "default playable seed should spawn with exposed mineable stone within 64 blocks"
    );
    assert!(
        starter_stone_count >= 11,
        "default playable seed should spawn with at least eleven exposed near-side starter stone blocks for wooden-pickaxe progression; found {starter_stone_count}"
    );
    assert!(
        exposed_iron_count >= 10,
        "default playable seed should spawn with at least ten exposed mineable iron ore blocks within 64 blocks for armor-tier progression retries; found {exposed_iron_count}"
    );
}

#[test]
fn sidecar_ore_rules_preserve_vein_shape_parameters() {
    let (_, registry, biomes) = generator();
    let feature = OreFeature {
        placed_feature: Identifier::parse("minecraft:ore_iron_test").unwrap(),
        configured_feature: Identifier::parse("minecraft:ore_iron_test").unwrap(),
        placement: OrePlacement {
            count: Some(OrePlacementCount::Constant(4)),
            rarity_chance: None,
            height: Some(HeightRange {
                kind: Identifier::parse("minecraft:uniform").unwrap(),
                min: HeightAnchor::Absolute(-32),
                max: HeightAnchor::Absolute(32),
            }),
        },
        size: 11,
        discard_chance_on_air_exposure: 0.75,
        targets: vec![
            OreTarget {
                state: Identifier::parse("minecraft:iron_ore").unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:stone_ore_replaceables").unwrap(),
                ),
            },
            OreTarget {
                state: Identifier::parse("minecraft:deepslate_iron_ore").unwrap(),
                replaceable_tag: Some(
                    Identifier::parse("minecraft:deepslate_ore_replaceables").unwrap(),
                ),
            },
        ],
    };

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
    assert_eq!(rule.size, 11);
    assert_eq!(rule.discard_chance_on_air_exposure, 0.75);
}

fn ore_positions(
    generator: &TerrainGenerator,
    positions: &[ChunkPos],
    ore: BlockStateId,
    min_y: i32,
    max_y: i32,
) -> BTreeSet<(i32, i32, i32)> {
    let mut found = BTreeSet::new();
    for &pos in positions {
        let chunk = generator.generate(pos);
        for y in min_y..=max_y {
            for lz in 0..16u8 {
                for lx in 0..16u8 {
                    if chunk.get_block(lx, y, lz) == Some(ore) {
                        found.insert((pos.x * 16 + i32::from(lx), y, pos.z * 16 + i32::from(lz)));
                    }
                }
            }
        }
    }
    found
}

fn largest_connected_component(mut positions: BTreeSet<(i32, i32, i32)>) -> usize {
    let mut largest = 0;
    while let Some(&start) = positions.iter().next() {
        positions.remove(&start);
        let mut stack = vec![start];
        let mut size = 0;
        while let Some((x, y, z)) = stack.pop() {
            size += 1;
            for neighbour in [
                (x - 1, y, z),
                (x + 1, y, z),
                (x, y - 1, z),
                (x, y + 1, z),
                (x, y, z - 1),
                (x, y, z + 1),
            ] {
                if positions.remove(&neighbour) {
                    stack.push(neighbour);
                }
            }
        }
        largest = largest.max(size);
    }
    largest
}

#[test]
fn configured_ore_size_forms_bounded_chunk_order_independent_veins() {
    let (_, registry, biomes) = generator();
    let ore = default_state(&registry, "minecraft:diamond_ore");
    let rule = OreRule {
        normal: ore,
        deepslate: ore,
        y: YRange::new(-48, 48),
        spacing: OreSpacing::Fixed(4096),
        biomes: BiomeScope::Any,
        size: 12,
        discard_chance_on_air_exposure: 0.0,
    };
    let generator = TerrainGenerator::with_rules(
        7,
        registry,
        biomes,
        OreRules::new(vec![rule]).expect("single ore rule fits the admission budget"),
    );
    let forward = (-4..=4)
        .flat_map(|z| (-4..=4).map(move |x| ChunkPos { x, z }))
        .collect::<Vec<_>>();
    let reverse = forward.iter().copied().rev().collect::<Vec<_>>();

    let forward_ores = ore_positions(&generator, &forward, ore, -48, 48);
    let reverse_ores = ore_positions(&generator, &reverse, ore, -48, 48);
    assert_eq!(forward_ores, reverse_ores);
    let largest = largest_connected_component(forward_ores);
    assert!(
        (6..=12).contains(&largest),
        "size-12 rules should produce connected, bounded veins; largest component was {largest}"
    );
}

#[test]
fn discard_chance_removes_exposed_cells_without_moving_veins() {
    let (_, registry, biomes) = generator();
    let ore = default_state(&registry, "minecraft:diamond_ore");
    let make_generator = |discard_chance_on_air_exposure| {
        TerrainGenerator::with_rules(
            19,
            Arc::clone(&registry),
            biomes.clone(),
            OreRules::new(vec![OreRule {
                normal: ore,
                deepslate: ore,
                y: YRange::new(-48, 48),
                spacing: OreSpacing::Fixed(64),
                biomes: BiomeScope::Any,
                size: 8,
                discard_chance_on_air_exposure,
            }])
            .expect("single ore rule fits the admission budget"),
        )
    };
    let positions = (-2..=2)
        .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
        .collect::<Vec<_>>();

    let retained = ore_positions(&make_generator(0.0), &positions, ore, -48, 48);
    let discarded = ore_positions(&make_generator(1.0), &positions, ore, -48, 48);
    assert!(discarded.is_subset(&retained));
    assert!(
        discarded.len() < retained.len(),
        "full discard chance should remove cave-exposed ore cells"
    );
}

#[test]
fn ore_generation_handles_extreme_valid_chunk_coordinates() {
    let (generator, _, _) = generator();

    let west = generator.generate(ChunkPos {
        x: i32::MIN.div_euclid(16),
        z: 0,
    });
    let east = generator.generate(ChunkPos {
        x: i32::MAX.div_euclid(16),
        z: 0,
    });

    assert_eq!(west.pos.x, i32::MIN.div_euclid(16));
    assert_eq!(east.pos.x, i32::MAX.div_euclid(16));
}

#[test]
fn one_connected_ore_component_crosses_a_specific_chunk_boundary() {
    let (_, registry, biomes) = generator();
    let ore = default_state(&registry, "minecraft:diamond_ore");
    let generator = TerrainGenerator::with_rules(
        7,
        registry,
        biomes,
        OreRules::new(vec![OreRule {
            normal: ore,
            deepslate: ore,
            y: YRange::new(-48, 48),
            spacing: OreSpacing::Fixed(32),
            biomes: BiomeScope::Any,
            size: 12,
            discard_chance_on_air_exposure: 0.0,
        }])
        .expect("single ore rule fits the admission budget"),
    );
    let positions = ore_positions(
        &generator,
        &[ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }],
        ore,
        -48,
        48,
    );

    assert!(
        positions
            .iter()
            .any(|&(x, y, z)| { x == 15 && positions.contains(&(16, y, z)) })
    );
}

#[test]
fn fractional_discard_removes_a_strict_subset_of_exposed_ore() {
    let (_, registry, biomes) = generator();
    let ore = default_state(&registry, "minecraft:diamond_ore");
    let make_generator = |discard_chance_on_air_exposure| {
        TerrainGenerator::with_rules(
            19,
            Arc::clone(&registry),
            biomes.clone(),
            OreRules::new(vec![OreRule {
                normal: ore,
                deepslate: ore,
                y: YRange::new(-48, 48),
                spacing: OreSpacing::Fixed(64),
                biomes: BiomeScope::Any,
                size: 8,
                discard_chance_on_air_exposure,
            }])
            .expect("single ore rule fits the admission budget"),
        )
    };
    let chunks = (-2..=2)
        .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
        .collect::<Vec<_>>();

    let retained = ore_positions(&make_generator(0.0), &chunks, ore, -48, 48);
    let half = ore_positions(&make_generator(0.5), &chunks, ore, -48, 48);
    let discarded = ore_positions(&make_generator(1.0), &chunks, ore, -48, 48);

    assert!(discarded.is_subset(&half));
    assert!(half.is_subset(&retained));
    assert!(discarded.len() < half.len());
    assert!(half.len() < retained.len());
}
