use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};

use crate::plant_rules_26_1_2::{
    PlantBlockEdit, PlantHorizontalDirection, PlantItemDrop, bonemeal_growth_edit,
    bonemeal_growth_edits, cocoa_state_for_use_on, grass_block_bonemeal_edits,
    next_crop_growth_state, plant_drop_stacks, sweet_berry_harvest, vertical_plant_growth_edits,
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

fn grass_scatter_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:grass_block"),
            simple_block(2, "minecraft:dirt"),
            simple_block(3, "minecraft:stone"),
            simple_block(4, "minecraft:short_grass"),
            simple_block(5, "minecraft:poppy"),
            simple_block(6, "minecraft:dandelion"),
            BlockReport {
                id: Identifier::parse("minecraft:tall_grass").expect("tall grass identifier"),
                properties: property_schema(&[("half", &["lower", "upper"])]),
                states: vec![
                    state(7, true, &[("half", "lower")]),
                    state(8, false, &[("half", "upper")]),
                ],
            },
        ])
        .expect("grass scatter registry"),
    )
}

fn world_with_grass(registry: &Arc<BlockRegistry>) -> (WorldStorage, BlockPos) {
    let mut world = in_memory_world(Arc::clone(registry));
    let clicked = BlockPos { x: 4, y: 64, z: 4 };
    world
        .set_block_at(clicked, BlockStateId(1))
        .expect("place grass block");
    world
        .set_block_at(BlockPos { x: 6, y: 64, z: 6 }, BlockStateId(2))
        .expect("place dirt support");
    world
        .set_block_at(BlockPos { x: 2, y: 64, z: 2 }, BlockStateId(3))
        .expect("place stone");
    (world, clicked)
}

#[test]
fn grass_block_bonemeal_scatters_deterministically_onto_open_soil() {
    let registry = grass_scatter_registry();
    let (world, clicked) = world_with_grass(&registry);

    let growing_seed = (0..256u64)
        .find(|&seed| {
            grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), seed).is_some()
        })
        .expect("the attempt gate must pass for some seed");
    assert!(
        (0..256u64).any(|seed| {
            grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), seed).is_none()
        }),
        "the attempt gate must also fail for some seeds"
    );

    let plan =
        grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), growing_seed)
            .expect("scatter plan");
    assert!(
        !plan.is_empty(),
        "a passing seed must plan at least one plant"
    );
    let vegetation = [
        BlockStateId(4),
        BlockStateId(5),
        BlockStateId(6),
        BlockStateId(7),
        BlockStateId(8),
    ];
    for edit in &plan {
        assert_ne!(
            edit.pos, clicked,
            "the clicked grass block must not be edited"
        );
        assert!(
            vegetation.contains(&edit.new_state),
            "scatter places vegetation only"
        );
        let dx = (edit.pos.x - clicked.x).abs();
        let dz = (edit.pos.z - clicked.z).abs();
        let dy = edit.pos.y - clicked.y;
        assert!(
            dx <= 3 && dz <= 3 && (-2..=2).contains(&dy),
            "scatter stays in the clicked block's neighbourhood"
        );
        assert_eq!(
            world.get_cached_block(edit.pos),
            Some(BlockStateId(0)),
            "scatter only targets cells that were air"
        );
    }
    for lower in plan.iter().filter(|edit| edit.new_state == BlockStateId(7)) {
        assert!(
            plan.iter().any(|top| {
                top.pos.x == lower.pos.x
                    && top.pos.y == lower.pos.y + 1
                    && top.pos.z == lower.pos.z
                    && top.new_state == BlockStateId(8)
            }),
            "tall grass plans as a lower+upper pair"
        );
    }

    let replay =
        grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), growing_seed)
            .expect("replay scatter plan");
    assert_eq!(plan, replay, "same input must plan the same scatter");
    assert_eq!(
        world.get_cached_block(clicked),
        Some(BlockStateId(1)),
        "planning must not mutate the world"
    );
}

#[test]
fn grass_block_bonemeal_rejects_covered_and_nongrass_blocks() {
    let registry = grass_scatter_registry();
    let (mut world, clicked) = world_with_grass(&registry);

    world
        .set_block_at(BlockPos { x: 4, y: 65, z: 4 }, BlockStateId(3))
        .expect("cover the grass block");
    for seed in 0..16u64 {
        assert_eq!(
            grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), seed),
            None,
            "solid block above must suppress the scatter"
        );
    }

    world
        .set_block_at(BlockPos { x: 4, y: 65, z: 4 }, BlockStateId(0))
        .expect("uncover the grass block");
    world
        .set_block_at(clicked, BlockStateId(3))
        .expect("replace the grass block with stone");
    for seed in 0..16u64 {
        assert_eq!(
            grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(3), seed),
            None,
            "non-grass blocks must not scatter"
        );
    }
}

#[test]
fn bonemeal_dispatcher_routes_grass_block_to_the_scatter() {
    let registry = grass_scatter_registry();
    let (world, clicked) = world_with_grass(&registry);

    let seed = (0..256u64)
        .find(|&seed| {
            bonemeal_growth_edits(&registry, &world, clicked, BlockStateId(1), seed).is_some()
        })
        .expect("dispatcher must reach the grass scatter");
    assert_eq!(
        bonemeal_growth_edits(&registry, &world, clicked, BlockStateId(1), seed),
        grass_block_bonemeal_edits(&registry, &world, clicked, BlockStateId(1), seed),
        "dispatcher must route grass_block to the scatter unchanged"
    );
    assert_eq!(
        bonemeal_growth_edits(&registry, &world, clicked, BlockStateId(3), seed),
        None,
        "dispatcher must keep rejecting inert blocks"
    );
}
