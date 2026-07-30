use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};

use crate::plant_rules_26_1_2::{
    PlantBlockEdit, PlantHorizontalDirection, PlantItemDrop, bonemeal_growth_edit,
    bonemeal_growth_edits, cocoa_state_for_use_on, next_crop_growth_state, plant_drop_stacks,
    sweet_berry_harvest, vertical_plant_growth_edits,
};
use crate::{BlockPos, BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

fn properties(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn property_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(name, values)| {
            (
                (*name).to_owned(),
                values.iter().map(|value| (*value).to_owned()).collect(),
            )
        })
        .collect()
}

fn state(id: u32, default: bool, entries: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: properties(entries),
    }
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).expect("test block identifier"),
        properties: BTreeMap::new(),
        states: vec![state(id, true, &[])],
    }
}

fn age_block(name: &str, states: &[(u32, u8, bool)]) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).expect("test age block identifier"),
        properties: property_schema(&[("age", &["0", "1", "2", "3", "7"])]),
        states: states
            .iter()
            .map(|&(id, age, default)| state(id, default, &[("age", &age.to_string())]))
            .collect(),
    }
}

fn in_memory_world(registry: Arc<BlockRegistry>) -> WorldStorage {
    let mut world = WorldStorage::in_memory(registry);
    let chunk = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            chunk,
            Chunk::empty(
                chunk,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").expect("test biome identifier"),
            ),
        )
        .expect("insert test chunk");
    world
}

#[test]
fn crop_harvest_and_drop_contracts_are_protocol_neutral() {
    let registry = BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        age_block(
            "minecraft:wheat",
            &[(1, 0, true), (2, 1, false), (3, 7, false)],
        ),
        age_block(
            "minecraft:sweet_berry_bush",
            &[(4, 1, true), (5, 2, false), (6, 3, false)],
        ),
    ])
    .expect("crop registry");
    let position = BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        next_crop_growth_state(&registry, BlockStateId(1)),
        Some(BlockStateId(2))
    );
    assert_eq!(
        bonemeal_growth_edit(&registry, position, BlockStateId(1)),
        Some(PlantBlockEdit {
            pos: position,
            new_state: BlockStateId(2),
        })
    );
    assert_eq!(
        sweet_berry_harvest(&registry, position, BlockStateId(6)),
        Some((
            PlantBlockEdit {
                pos: position,
                new_state: BlockStateId(4),
            },
            PlantItemDrop {
                item: Identifier::parse("minecraft:sweet_berries").expect("berry identifier"),
                count: 2,
            },
        ))
    );
    assert_eq!(
        plant_drop_stacks(registry.by_id(BlockStateId(3)).expect("mature wheat state")),
        Some(vec![
            PlantItemDrop {
                item: Identifier::parse("minecraft:wheat").expect("wheat identifier"),
                count: 1,
            },
            PlantItemDrop {
                item: Identifier::parse("minecraft:wheat_seeds").expect("seed identifier"),
                count: 1,
            },
        ])
    );
}

#[test]
fn vertical_growth_reads_loaded_world_without_mutating_it() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:sand"),
            simple_block(2, "minecraft:cactus"),
        ])
        .expect("vertical plant registry"),
    );
    let mut world = in_memory_world(Arc::clone(&registry));
    let support = BlockPos { x: 4, y: 63, z: 4 };
    let cactus = BlockPos { x: 4, y: 64, z: 4 };
    let above = BlockPos { x: 4, y: 65, z: 4 };
    world
        .set_block_at(support, BlockStateId(1))
        .expect("place sand");
    world
        .set_block_at(cactus, BlockStateId(2))
        .expect("place cactus");

    assert_eq!(
        vertical_plant_growth_edits(&registry, &world, cactus, BlockStateId(2), 0),
        Some(vec![PlantBlockEdit {
            pos: above,
            new_state: BlockStateId(2),
        }])
    );
    assert_eq!(
        world.get_cached_block(above),
        Some(BlockStateId(0)),
        "planning must not mutate the world"
    );
}

#[test]
fn sapling_planning_is_seeded_and_read_only() {
    let registry = Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            BlockReport {
                id: Identifier::parse("minecraft:oak_sapling").expect("sapling identifier"),
                properties: property_schema(&[("stage", &["0", "1"])]),
                states: vec![
                    state(1, true, &[("stage", "0")]),
                    state(2, false, &[("stage", "1")]),
                ],
            },
            BlockReport {
                id: Identifier::parse("minecraft:oak_log").expect("log identifier"),
                properties: property_schema(&[("axis", &["y"])]),
                states: vec![state(3, true, &[("axis", "y")])],
            },
            BlockReport {
                id: Identifier::parse("minecraft:oak_leaves").expect("leaves identifier"),
                properties: property_schema(&[
                    ("distance", &["1"]),
                    ("persistent", &["false"]),
                    ("waterlogged", &["false"]),
                ]),
                states: vec![state(
                    4,
                    true,
                    &[
                        ("distance", "1"),
                        ("persistent", "false"),
                        ("waterlogged", "false"),
                    ],
                )],
            },
        ])
        .expect("sapling registry"),
    );
    let mut world = in_memory_world(Arc::clone(&registry));
    let sapling = BlockPos { x: 4, y: 64, z: 4 };
    world
        .set_block_at(sapling, BlockStateId(1))
        .expect("place stage-zero sapling");

    assert_eq!(
        bonemeal_growth_edits(&registry, &world, sapling, BlockStateId(1), 0),
        Some(vec![PlantBlockEdit {
            pos: sapling,
            new_state: BlockStateId(2),
        }])
    );
    assert_eq!(world.get_cached_block(sapling), Some(BlockStateId(1)));

    world
        .set_block_at(sapling, BlockStateId(2))
        .expect("advance sapling fixture");
    let short = bonemeal_growth_edits(&registry, &world, sapling, BlockStateId(2), 0)
        .expect("short oak plan");
    let tall = bonemeal_growth_edits(&registry, &world, sapling, BlockStateId(2), 2)
        .expect("tall oak plan");
    assert!(short.len() < tall.len());
    assert_eq!(world.get_cached_block(sapling), Some(BlockStateId(2)));
}

#[test]
fn cocoa_uses_a_protocol_neutral_horizontal_direction() {
    let registry = BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:jungle_log"),
        BlockReport {
            id: Identifier::parse("minecraft:cocoa").expect("cocoa identifier"),
            properties: property_schema(&[("age", &["0"]), ("facing", &["north", "east"])]),
            states: vec![
                state(2, true, &[("age", "0"), ("facing", "north")]),
                state(3, false, &[("age", "0"), ("facing", "east")]),
            ],
        },
    ])
    .expect("cocoa registry");

    assert_eq!(
        cocoa_state_for_use_on(BlockStateId(1), PlantHorizontalDirection::East, &registry,),
        Some(BlockStateId(3))
    );
    assert_eq!(
        cocoa_state_for_use_on(BlockStateId(0), PlantHorizontalDirection::North, &registry,),
        None
    );
}
