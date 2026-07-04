use std::collections::BTreeSet;
use std::sync::Arc;

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkPos, MAX_Y};
use mc_worldgen::{BiomeRules, TerrainGenerator};

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

    for cx in -16..=16 {
        for cz in -16..=16 {
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
                            return;
                        }
                    }
                }
            }
        }
    }

    panic!("sampled land chunks should contain at least one log trunk with a leaf canopy");
}
