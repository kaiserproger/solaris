use super::{
    BlockChangedAck, BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition, BlockReport,
    BlockStateId, BlockUpdate, Chunk, ChunkPos, ClientboundContainerSetSlot, Compression,
    Direction, GameMode, Identifier, ItemRegistry, ItemReport, ItemStack, ItemToBlockTable,
    MovePlayerFlags, PlayerInventory, PlayerPose, SessionRegistry,
    append_cactus_side_neighbor_cascades, apply_block_edit_batch_to_storage_conditionally,
    apply_block_edit_to_storage, block_drop_stacks_from, block_state_property,
    bonemeal_growth_edit, bonemeal_growth_edits, consume_bonemeal_after_growth, crop_test_registry,
    farmland_trample_pos, fluid_test_facts, fluid_test_registry, handle_block_item_placement,
    insert_fluid_test_chunk, interaction_state_for_blocks, interaction_state_for_items,
    maybe_trample_farmland, next_crop_growth_state, pack_block_pos, plan_break_block_edits,
    plan_hoe_tilling, plan_loaded_bonemeal_growth, plan_loaded_plant_harvest,
    plan_place_block_edits, player_pose_collides_with_solid, prop_schema, random_tick_edit,
    random_tick_edit_seeded, register_ticketed_button_session, simple_block, simulation_channel,
    state, sweet_berry_harvest, test_use_item_on, unpack_block_pos,
};
use mc_protocol::Packet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

fn staged_sapling_block(stage_zero_id: u32, stage_one_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("stage", &["0", "1"])]),
        states: vec![
            state(stage_zero_id, true, &[("stage", "0")]),
            state(stage_one_id, false, &[("stage", "1")]),
        ],
    }
}

fn axis_log_block(id_base: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("axis", &["x", "y"])]),
        states: vec![
            state(id_base, true, &[("axis", "x")]),
            state(id_base + 1, false, &[("axis", "y")]),
        ],
    }
}

fn tree_leaves_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("distance", &["1"]), ("persistent", &["false"])]),
        states: vec![state(
            id,
            true,
            &[("distance", "1"), ("persistent", "false")],
        )],
    }
}

const VANILLA_26_1_2_TREE_REPLACEABLES: [&str; 53] = [
    "minecraft:acacia_leaves",
    "minecraft:allium",
    "minecraft:azalea_leaves",
    "minecraft:azure_bluet",
    "minecraft:birch_leaves",
    "minecraft:blue_orchid",
    "minecraft:bush",
    "minecraft:cherry_leaves",
    "minecraft:closed_eyeblossom",
    "minecraft:cornflower",
    "minecraft:crimson_roots",
    "minecraft:dandelion",
    "minecraft:dark_oak_leaves",
    "minecraft:dead_bush",
    "minecraft:fern",
    "minecraft:firefly_bush",
    "minecraft:flowering_azalea_leaves",
    "minecraft:glow_lichen",
    "minecraft:golden_dandelion",
    "minecraft:hanging_roots",
    "minecraft:jungle_leaves",
    "minecraft:large_fern",
    "minecraft:leaf_litter",
    "minecraft:lilac",
    "minecraft:lily_of_the_valley",
    "minecraft:mangrove_leaves",
    "minecraft:nether_sprouts",
    "minecraft:oak_leaves",
    "minecraft:open_eyeblossom",
    "minecraft:orange_tulip",
    "minecraft:oxeye_daisy",
    "minecraft:pale_moss_carpet",
    "minecraft:pale_oak_leaves",
    "minecraft:peony",
    "minecraft:pink_tulip",
    "minecraft:pitcher_plant",
    "minecraft:poppy",
    "minecraft:red_tulip",
    "minecraft:rose_bush",
    "minecraft:seagrass",
    "minecraft:short_dry_grass",
    "minecraft:short_grass",
    "minecraft:spruce_leaves",
    "minecraft:sunflower",
    "minecraft:tall_dry_grass",
    "minecraft:tall_grass",
    "minecraft:tall_seagrass",
    "minecraft:torchflower",
    "minecraft:vine",
    "minecraft:warped_roots",
    "minecraft:water",
    "minecraft:white_tulip",
    "minecraft:wither_rose",
];

fn sapling_tree_test_reports() -> Vec<BlockReport> {
    let mut reports = vec![
        simple_block(0, "minecraft:air"),
        staged_sapling_block(1, 27, "minecraft:oak_sapling"),
        axis_log_block(2, "minecraft:oak_log"),
        tree_leaves_block(4, "minecraft:oak_leaves"),
        simple_block(5, "minecraft:stone"),
        staged_sapling_block(6, 28, "minecraft:cherry_sapling"),
        staged_sapling_block(7, 29, "minecraft:birch_sapling"),
        axis_log_block(8, "minecraft:birch_log"),
        tree_leaves_block(10, "minecraft:birch_leaves"),
        staged_sapling_block(11, 30, "minecraft:spruce_sapling"),
        axis_log_block(12, "minecraft:spruce_log"),
        tree_leaves_block(14, "minecraft:spruce_leaves"),
        staged_sapling_block(15, 31, "minecraft:jungle_sapling"),
        axis_log_block(16, "minecraft:jungle_log"),
        tree_leaves_block(18, "minecraft:jungle_leaves"),
        staged_sapling_block(19, 32, "minecraft:acacia_sapling"),
        axis_log_block(20, "minecraft:acacia_log"),
        tree_leaves_block(22, "minecraft:acacia_leaves"),
        staged_sapling_block(23, 33, "minecraft:dark_oak_sapling"),
        axis_log_block(24, "minecraft:dark_oak_log"),
        tree_leaves_block(26, "minecraft:dark_oak_leaves"),
        simple_block(34, "minecraft:short_grass"),
        simple_block(35, "minecraft:vine"),
    ];
    let mut next_state_id = 36;
    for name in VANILLA_26_1_2_TREE_REPLACEABLES {
        if reports.iter().any(|report| report.id.as_str() == name) {
            continue;
        }
        reports.push(simple_block(next_state_id, name));
        next_state_id += 1;
    }
    reports
}

fn sapling_tree_test_registry() -> Arc<mc_world::BlockRegistry> {
    Arc::new(mc_world::BlockRegistry::from_report(&sapling_tree_test_reports()).unwrap())
}

fn in_memory_tree_world(registry: Arc<mc_world::BlockRegistry>) -> mc_world::WorldStorage {
    let mut world = mc_world::WorldStorage::in_memory(registry);
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    world
}

fn wheat_drop_items() -> ItemRegistry {
    ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            protocol_id: 50,
        },
        ItemReport {
            id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
            protocol_id: 51,
        },
    ])
}

fn carrot_slice_drop_items() -> ItemRegistry {
    ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            protocol_id: 50,
        },
        ItemReport {
            id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
            protocol_id: 51,
        },
        ItemReport {
            id: Identifier::parse("minecraft:carrot").unwrap(),
            protocol_id: 52,
        },
        ItemReport {
            id: Identifier::parse("minecraft:pumpkin_stem").unwrap(),
            protocol_id: 53,
        },
        ItemReport {
            id: Identifier::parse("minecraft:potato").unwrap(),
            protocol_id: 54,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot").unwrap(),
            protocol_id: 55,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
            protocol_id: 56,
        },
        ItemReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            protocol_id: 57,
        },
    ])
}

fn test_crop_state_with_age(blocks: &mc_world::BlockRegistry, crop: &str, age: u8) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse(crop).unwrap(),
            &[("age".to_string(), age.to_string())],
        )
        .unwrap()
}

#[test]
fn wheat_crop_drop_mature_returns_wheat_and_seeds() {
    let blocks = crop_test_registry();
    let items = wheat_drop_items();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        wheat,
    );

    assert_eq!(drops, vec![ItemStack::new(50, 1), ItemStack::new(51, 1)]);
}

#[test]
fn wheat_crop_drop_young_returns_seeds_only() {
    let blocks = crop_test_registry();
    let items = wheat_drop_items();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        wheat,
    );

    assert_eq!(drops, vec![ItemStack::new(51, 1)]);
}

#[test]
fn block_drop_generic_non_crop_fallback_still_returns_block_item() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:dirt").unwrap(),
        protocol_id: 52,
    }]);
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    let drops =
        block_drop_stacks_from(&mc_data::loot::LootTables::default(), &items, &blocks, dirt);

    assert_eq!(drops, vec![ItemStack::new(52, 1)]);
}

#[test]
fn wheat_crop_drop_rejects_incomplete_logical_drop() {
    let blocks = crop_test_registry();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let missing_all = ItemRegistry::default();

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            wheat,
        )
        .is_empty()
    );
    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_all,
            &blocks,
            wheat,
        )
        .is_empty()
    );
}

#[test]
fn carrot_crop_drop_mature_returns_two_carrots() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        carrots,
    );

    assert_eq!(drops, vec![ItemStack::new(52, 2)]);
}

#[test]
fn carrot_crop_drop_immature_returns_one_carrot() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=6 {
        let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            carrots,
        );

        assert_eq!(drops, vec![ItemStack::new(52, 1)], "age {age}");
    }
}

#[test]
fn carrot_slice_preserves_wheat_crop_drop_behavior() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let mature_wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);
    let young_wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 3);

    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            mature_wheat,
        ),
        vec![ItemStack::new(50, 1), ItemStack::new(51, 1)]
    );
    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            young_wheat,
        ),
        vec![ItemStack::new(51, 1)]
    );
}

#[test]
fn carrot_slice_unsupported_crop_state_uses_generic_fallback() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let pumpkin_stem = test_crop_state_with_age(&blocks, "minecraft:pumpkin_stem", 1);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        pumpkin_stem,
    );

    assert_eq!(drops, vec![ItemStack::new(53, 1)]);
}

#[test]
fn carrot_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let carrots = test_crop_state_with_age(&blocks, "minecraft:carrots", 7);
    let missing_carrot = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat").unwrap(),
        protocol_id: 50,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_carrot,
            &blocks,
            carrots,
        )
        .is_empty()
    );
}

#[test]
fn potato_crop_drop_mature_returns_two_potatoes() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", 7);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        potatoes,
    );

    assert_eq!(drops, vec![ItemStack::new(54, 2)]);
}

#[test]
fn potato_crop_drop_immature_returns_one_potato() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=6 {
        let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            potatoes,
        );

        assert_eq!(drops, vec![ItemStack::new(54, 1)], "age {age}");
    }
}

#[test]
fn potato_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let potatoes = test_crop_state_with_age(&blocks, "minecraft:potatoes", 7);
    let missing_potato = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:carrot").unwrap(),
        protocol_id: 52,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_potato,
            &blocks,
            potatoes,
        )
        .is_empty()
    );
}

#[test]
fn beetroot_crop_drop_mature_returns_beetroot_and_seeds() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        beetroots,
    );

    assert_eq!(drops, vec![ItemStack::new(55, 1), ItemStack::new(56, 1)]);
}

#[test]
fn beetroot_crop_drop_immature_returns_seeds_only() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=2 {
        let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            beetroots,
        );

        assert_eq!(drops, vec![ItemStack::new(56, 1)], "age {age}");
    }
}

#[test]
fn beetroot_crop_drop_rejects_incomplete_logical_drop() {
    let blocks = crop_test_registry();
    let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", 3);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
        protocol_id: 56,
    }]);
    let missing_all = ItemRegistry::default();

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            beetroots,
        )
        .is_empty()
    );
    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_all,
            &blocks,
            beetroots,
        )
        .is_empty()
    );
}

#[test]
fn nether_wart_crop_drop_mature_returns_two_warts() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();
    let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", 3);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        nether_wart,
    );

    assert_eq!(drops, vec![ItemStack::new(57, 2)]);
}

#[test]
fn nether_wart_crop_drop_immature_returns_one_wart() {
    let blocks = crop_test_registry();
    let items = carrot_slice_drop_items();

    for age in 0..=2 {
        let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", age);

        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            nether_wart,
        );

        assert_eq!(drops, vec![ItemStack::new(57, 1)], "age {age}");
    }
}

#[test]
fn nether_wart_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let nether_wart = test_crop_state_with_age(&blocks, "minecraft:nether_wart", 3);
    let missing_wart = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot").unwrap(),
        protocol_id: 55,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_wart,
            &blocks,
            nether_wart,
        )
        .is_empty()
    );
}

#[test]
fn cocoa_crop_drop_mature_returns_three_beans() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);

    let drops = block_drop_stacks_from(
        &mc_data::loot::LootTables::default(),
        &items,
        &blocks,
        mc_world::BlockStateId(62),
    );

    assert_eq!(drops, vec![ItemStack::new(58, 3)]);
}

#[test]
fn cocoa_crop_drop_immature_returns_one_bean() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);

    for state in [mc_world::BlockStateId(60), mc_world::BlockStateId(61)] {
        let drops = block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &items,
            &blocks,
            state,
        );

        assert_eq!(drops, vec![ItemStack::new(58, 1)], "state {state:?}");
    }
}

#[test]
fn cocoa_crop_drop_rejects_missing_item_id() {
    let blocks = crop_test_registry();
    let missing_beans = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot").unwrap(),
        protocol_id: 55,
    }]);

    assert!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &missing_beans,
            &blocks,
            mc_world::BlockStateId(62),
        )
        .is_empty()
    );
}

fn bamboo_test_registry() -> mc_world::BlockRegistry {
    let mut bamboo_states = Vec::new();
    let mut next_id = 3;
    for age in ["0", "1"] {
        for leaves in ["none", "small", "large"] {
            for stage in ["0", "1"] {
                bamboo_states.push(state(
                    next_id,
                    age == "0" && leaves == "none" && stage == "0",
                    &[("age", age), ("leaves", leaves), ("stage", stage)],
                ));
                next_id += 1;
            }
        }
    }
    mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:sand"),
        simple_block(2, "minecraft:bamboo_sapling"),
        BlockReport {
            id: Identifier::parse("minecraft:bamboo").unwrap(),
            properties: prop_schema(&[
                ("age", &["0", "1"]),
                ("leaves", &["none", "small", "large"]),
                ("stage", &["0", "1"]),
            ]),
            states: bamboo_states,
        },
    ])
    .unwrap()
}

fn bamboo_state(
    blocks: &mc_world::BlockRegistry,
    age: &str,
    leaves: &str,
    stage: &str,
) -> BlockStateId {
    blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:bamboo").unwrap(),
            &[
                ("age".into(), age.into()),
                ("leaves".into(), leaves.into()),
                ("stage".into(), stage.into()),
            ],
        )
        .unwrap()
}

#[tokio::test]
async fn player_collision_uses_farmland_height_and_allows_wheat_overlap() {
    let state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    insert_fluid_test_chunk(&state).await;
    {
        let mut world = state.world.lock().await;
        world
            .set_block_at(mc_world::BlockPos { x: 0, y: 64, z: 0 }, BlockStateId(3))
            .unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 0, y: 65, z: 0 }, BlockStateId(18))
            .unwrap();
    }

    assert!(
        !player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.9375, 0.5),).await,
        "the vanilla client stands at 15/16 block height and overlaps non-colliding wheat"
    );
    assert!(
        player_pose_collides_with_solid(Some(&state), PlayerPose::new(0.5, 64.90, 0.5)).await,
        "the farmland collision shape must still reject movement through its top surface"
    );
}

#[test]
fn wheat_seeds_place_wheat_on_farmland_only() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 50,
    }]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let farmland = Identifier::parse("minecraft:farmland").unwrap();
    let farmland_state = blocks
        .by_name_and_props(&farmland, &[("moisture".to_string(), "0".to_string())])
        .unwrap();
    let dirt_state = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(table.resolve(50), None);
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(11))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, farmland_state, Direction::North, &blocks),
        None
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 50, dirt_state, Direction::Up, &blocks),
        None
    );
}

#[test]
fn common_crop_items_place_on_their_required_soil_only() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:carrot").unwrap(),
            protocol_id: 51,
        },
        ItemReport {
            id: Identifier::parse("minecraft:potato").unwrap(),
            protocol_id: 52,
        },
        ItemReport {
            id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
            protocol_id: 53,
        },
        ItemReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            protocol_id: 54,
        },
    ]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let farmland = Identifier::parse("minecraft:farmland").unwrap();
    let farmland_state = blocks
        .by_name_and_props(&farmland, &[("moisture".to_string(), "0".to_string())])
        .unwrap();
    let soul_sand = blocks
        .block(&Identifier::parse("minecraft:soul_sand").unwrap())
        .unwrap()
        .default;

    assert_eq!(
        table.resolve_for_use_on(&items, 51, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(20))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 52, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(28))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 53, farmland_state, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(36))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 54, soul_sand, Direction::Up, &blocks),
        Some(mc_world::BlockStateId(44))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 54, farmland_state, Direction::Up, &blocks),
        None
    );
}

#[test]
fn cocoa_beans_place_cocoa_on_jungle_log_sides() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:cocoa_beans").unwrap(),
        protocol_id: 58,
    }]);
    let blocks = crop_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);
    let jungle_log = blocks
        .block(&Identifier::parse("minecraft:jungle_log").unwrap())
        .unwrap()
        .default;
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;

    assert_eq!(
        table.resolve_for_use_on(&items, 58, jungle_log, Direction::North, &blocks),
        Some(mc_world::BlockStateId(60))
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 58, jungle_log, Direction::Up, &blocks),
        None
    );
    assert_eq!(
        table.resolve_for_use_on(&items, 58, dirt, Direction::North, &blocks),
        None
    );
}

#[test]
fn cactus_column_cascades_when_support_breaks() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();

    let edits = plan_break_block_edits(
        blocks,
        &world,
        support,
        BlockStateId(1),
        BlockStateId(0),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: support,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn cactus_column_cascades_when_solid_side_neighbor_is_placed() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    let mut edits = vec![BlockEdit {
        pos: placed,
        new_state: BlockStateId(1),
    }];
    let snapshot = world.read_view().snapshot_chunks(&[cpos]);

    append_cactus_side_neighbor_cascades(
        blocks,
        &snapshot,
        &mut edits,
        placed,
        BlockStateId(1),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: placed,
                new_state: BlockStateId(1),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn cactus_column_does_not_cascade_when_cactus_side_neighbor_is_placed() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    let mut edits = vec![BlockEdit {
        pos: placed,
        new_state: BlockStateId(19),
    }];
    let snapshot = world.read_view().snapshot_chunks(&[cpos]);

    append_cactus_side_neighbor_cascades(
        blocks,
        &snapshot,
        &mut edits,
        placed,
        BlockStateId(19),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: placed,
            new_state: BlockStateId(19),
        }]
    );
}

#[tokio::test]
async fn cactus_placement_path_cascades_when_solid_side_neighbor_is_placed() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
        world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(1),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    let plan = plan.expect("dirt placement plan");
    assert_eq!(
        plan.edits,
        vec![
            BlockEdit {
                pos: placed,
                new_state: BlockStateId(1),
            },
            BlockEdit {
                pos: cactus_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: cactus_2,
                new_state: BlockStateId(0),
            },
        ]
    );
    for cactus in [cactus_1, cactus_2] {
        let precondition = plan
            .additional_preconditions
            .iter()
            .find(|precondition| precondition.pos == cactus)
            .expect("every cascaded cactus is fenced by its exact source state");
        assert_eq!(precondition.expected_state, BlockStateId(19));
    }
}

#[tokio::test]
async fn cactus_placement_path_does_not_cascade_for_non_solid_side_neighbor() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    {
        let mut world = state.world.lock().await;
        world.set_block_at(cactus_1, BlockStateId(19)).unwrap();
        world.set_block_at(cactus_2, BlockStateId(19)).unwrap();
        world
            .set_block_at(mc_world::BlockPos { x: 5, y: 64, z: 4 }, BlockStateId(16))
            .unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(20),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    let plan = plan.expect("non-solid placement plan");
    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos: placed,
            new_state: BlockStateId(20),
        }]
    );
}

#[tokio::test]
async fn vertical_plant_placement_rejects_stone_support() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let targets = [
        (mc_world::BlockPos { x: 3, y: 65, z: 4 }, BlockStateId(19)),
        (mc_world::BlockPos { x: 6, y: 65, z: 4 }, BlockStateId(20)),
        (mc_world::BlockPos { x: 9, y: 65, z: 4 }, BlockStateId(21)),
    ];
    {
        let mut world = state.world.lock().await;
        for (target, _) in targets {
            world
                .set_block_at(mc_world::BlockPos { y: 64, ..target }, BlockStateId(1))
                .unwrap();
        }
        world
            .set_block_at(mc_world::BlockPos { x: 10, y: 64, z: 4 }, BlockStateId(2))
            .unwrap();
    }

    for (target, plant) in targets {
        assert_eq!(
            plan_place_block_edits(
                &state,
                target,
                plant,
                PlayerPose::new(0.5, 64.0, 0.5),
                Direction::Up,
                0.5,
            )
            .await,
            None
        );
    }
}

#[tokio::test]
async fn invalid_support_placement_resyncs_without_mutating_or_debiting_inventory() {
    let blocks = Arc::new(fluid_test_registry());
    let cactus = Identifier::parse("minecraft:cactus").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: cactus,
        protocol_id: 42,
    }]));
    let mut state = interaction_state_for_blocks(Arc::clone(&blocks));
    state.items = Arc::clone(&items);
    state.item_to_block = ItemToBlockTable::build(&items, &blocks);
    *state.inventory.held_mut(0).unwrap() = ItemStack::new(42, 1);
    insert_fluid_test_chunk(&state).await;

    let clicked = mc_world::BlockPos { x: 3, y: 64, z: 4 };
    let target = mc_world::BlockPos { y: 65, ..clicked };
    state
        .world
        .lock()
        .await
        .set_block_at(clicked, BlockStateId(1))
        .unwrap();
    let action = test_use_item_on(pack_block_pos(clicked.x, clicked.y, clicked.z));
    let mut writer = Vec::new();

    handle_block_item_placement(
        &mut state,
        &mut writer,
        None,
        GameMode::Survival,
        PlayerPose::new(3.5, 64.0, 4.5),
        clicked,
        &action,
        (clicked.x, clicked.y, clicked.z),
    )
    .await
    .unwrap();

    assert_eq!(state.inventory.held(0), Some(&ItemStack::new(42, 1)));
    assert_eq!(
        state.world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );

    let mut frames = bytes::BytesMut::from(writer.as_slice());
    let mut updates = Vec::new();
    let mut saw_held_resync = false;
    let mut saw_ack = false;
    while let Some(mut frame) =
        mc_protocol::frame::try_decode_frame(&mut frames, Compression::Disabled).unwrap()
    {
        if frame.id == BlockUpdate::ID {
            updates.push(BlockUpdate::decode(&mut frame.body).unwrap());
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let packet = ClientboundContainerSetSlot::decode(&mut frame.body).unwrap();
            assert_eq!(packet.container_id, 0);
            assert_eq!(packet.slot, PlayerInventory::HOTBAR_BASE as i16);
            assert_eq!(packet.item_stack, ItemStack::new(42, 1));
            saw_held_resync = true;
        } else if frame.id == BlockChangedAck::ID {
            assert!(!updates.is_empty(), "ack must follow block resyncs");
            assert!(
                saw_held_resync,
                "ack must follow the unchanged held-stack resync"
            );
            assert_eq!(
                BlockChangedAck::decode(&mut frame.body).unwrap().sequence,
                action.sequence
            );
            saw_ack = true;
        } else {
            panic!("unexpected packet during invalid support placement rejection");
        }
    }

    assert!(
        saw_ack,
        "invalid support placement must acknowledge the action"
    );
    assert!(
        saw_held_resync,
        "invalid support placement must resync the unchanged held stack"
    );
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().any(|update| {
        unpack_block_pos(update.position) == (clicked.x, clicked.y, clicked.z)
            && update.state_id == 1
    }));
    assert!(updates.iter().any(|update| {
        unpack_block_pos(update.position) == (target.x, target.y, target.z) && update.state_id == 0
    }));
}

#[tokio::test]
async fn cactus_placement_path_rejects_adjacent_cactus() {
    let state = interaction_state_for_blocks(Arc::new(fluid_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let placed = mc_world::BlockPos { x: 5, y: 65, z: 4 };
    {
        let mut world = state.world.lock().await;
        world
            .set_block_at(mc_world::BlockPos { x: 4, y: 65, z: 4 }, BlockStateId(19))
            .unwrap();
    }

    let plan = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(19),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
        0.5,
    )
    .await;

    assert_eq!(plan, None);
}

#[test]
fn cactus_random_tick_grows_on_sand_to_height_three() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:desert").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cactus_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let cactus_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_1,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cactus_2,
            new_state: BlockStateId(19),
        }])
    );
    world.set_block_at(cactus_2, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_2,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cactus_3,
            new_state: BlockStateId(19),
        }])
    );
    world.set_block_at(cactus_3, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus_1,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn cactus_random_tick_unsupported_or_obstructed_columns_are_noop() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:desert").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let cactus = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let above = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let side = mc_world::BlockPos { x: 5, y: 66, z: 4 };
    world.set_block_at(cactus, BlockStateId(19)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None,
        "stone is not a vanilla cactus support"
    );
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(above, BlockStateId(0)).unwrap();
    world.set_block_at(side, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn sugar_cane_random_tick_grows_on_sand_beside_water_to_height_three() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let water = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    let cane_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let cane_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let cane_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(water, BlockStateId(2)).unwrap();
    world.set_block_at(cane_1, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_1,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cane_2,
            new_state: BlockStateId(21),
        }])
    );
    world.set_block_at(cane_2, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_2,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: cane_3,
            new_state: BlockStateId(21),
        }])
    );
    world.set_block_at(cane_3, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane_2,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn sugar_cane_random_tick_unsupported_or_obstructed_columns_are_noop() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let water = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    let cane = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let above = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(cane, BlockStateId(21)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(water, BlockStateId(2)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None,
        "water does not make stone a vanilla sugar-cane support"
    );

    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn bamboo_column_cascades_when_support_breaks() {
    let registry = Arc::new(fluid_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let bamboo_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(bamboo_1, BlockStateId(20)).unwrap();
    world.set_block_at(bamboo_2, BlockStateId(20)).unwrap();

    let edits = plan_break_block_edits(
        blocks,
        &world,
        support,
        BlockStateId(1),
        BlockStateId(0),
        BlockStateId(0),
    );

    assert_eq!(
        edits,
        vec![
            BlockEdit {
                pos: support,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: bamboo_1,
                new_state: BlockStateId(0),
            },
            BlockEdit {
                pos: bamboo_2,
                new_state: BlockStateId(0),
            },
        ]
    );
}

#[test]
fn bamboo_random_tick_grows_on_sand_until_height_sixteen() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();

    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo_1 = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let bamboo_2 = mc_world::BlockPos { x: 4, y: 66, z: 4 };
    let bamboo_3 = mc_world::BlockPos { x: 4, y: 67, z: 4 };
    let bamboo_4 = mc_world::BlockPos { x: 4, y: 68, z: 4 };
    world.set_block_at(support, BlockStateId(16)).unwrap();
    world.set_block_at(bamboo_1, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_1,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_2,
            new_state: BlockStateId(20),
        }])
    );
    world.set_block_at(bamboo_2, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_2,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_3,
            new_state: BlockStateId(20),
        }])
    );
    world.set_block_at(bamboo_3, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_3,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![BlockEdit {
            pos: bamboo_4,
            new_state: BlockStateId(20),
        }])
    );

    for y in bamboo_4.y..=80 {
        world
            .set_block_at(mc_world::BlockPos { y, ..bamboo_4 }, BlockStateId(20))
            .unwrap();
    }
    let bamboo_16 = mc_world::BlockPos { y: 80, ..bamboo_4 };
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo_16,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn bamboo_random_tick_rejects_stone_support() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bamboo = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(bamboo, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            bamboo,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn lower_vertical_plant_segments_do_not_grow_the_column_top() {
    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    for (state, support_state) in [
        (BlockStateId(19), BlockStateId(16)),
        (BlockStateId(20), BlockStateId(16)),
        (BlockStateId(21), BlockStateId(16)),
    ] {
        let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
        let cpos = ChunkPos { x: 0, z: 0 };
        world
            .insert_generated_chunk(
                cpos,
                Chunk::empty(
                    cpos,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();
        let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
        let top = mc_world::BlockPos { x: 4, y: 66, z: 4 };
        world.set_block_at(support, support_state).unwrap();
        if state == BlockStateId(21) {
            world
                .set_block_at(mc_world::BlockPos { x: 5, ..support }, BlockStateId(2))
                .unwrap();
        }
        world.set_block_at(bottom, state).unwrap();
        world.set_block_at(top, state).unwrap();

        assert_eq!(
            random_tick_edit(
                registry.as_ref(),
                &facts,
                &world,
                bottom,
                state,
                mc_data::block_facts::RandomTickFamily::Crop,
            ),
            None
        );
    }
}

#[test]
fn bamboo_random_tick_builds_vanilla_age_and_leaf_crown() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world
        .set_block_at(bottom, bamboo_state(&registry, "0", "none", "0"))
        .unwrap();

    for top_y in 65..=67 {
        let top = mc_world::BlockPos { y: top_y, ..bottom };
        let state = world.get_block(top).unwrap().unwrap();
        let edits = random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            top,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            0,
        )
        .expect("successful bamboo growth");
        for edit in edits {
            world.set_block_at(edit.pos, edit.new_state).unwrap();
        }
    }

    assert_eq!(
        (65..=68)
            .map(|y| {
                let state = world
                    .get_block(mc_world::BlockPos { y, ..bottom })
                    .unwrap()
                    .unwrap();
                let state = registry.by_id(state).unwrap();
                (
                    block_state_property(state, "age").unwrap().to_string(),
                    block_state_property(state, "leaves").unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("1".into(), "none".into()),
            ("1".into(), "small".into()),
            ("1".into(), "small".into()),
            ("1".into(), "large".into()),
        ]
    );
}

#[test]
fn bamboo_random_tick_uses_one_in_three_growth_chance() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world
        .set_block_at(mc_world::BlockPos { y: 64, ..pos }, BlockStateId(1))
        .unwrap();
    let state = bamboo_state(&registry, "0", "none", "0");
    world.set_block_at(pos, state).unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            1,
        ),
        None
    );
    assert!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            state,
            mc_data::block_facts::RandomTickFamily::Crop,
            3,
        )
        .is_some()
    );
}

#[test]
fn bamboo_sapling_random_tick_creates_two_exact_segments() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let pos = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Sapling,
            1,
        ),
        None
    );

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        Some(vec![
            BlockEdit {
                pos,
                new_state: bamboo_state(&registry, "0", "none", "0"),
            },
            BlockEdit {
                pos: mc_world::BlockPos { y: 66, ..pos },
                new_state: bamboo_state(&registry, "0", "small", "0"),
            },
        ])
    );
}

#[test]
fn bamboo_random_tick_marks_the_sixteenth_segment_terminal() {
    let registry = Arc::new(bamboo_test_registry());
    let facts = mc_data::block_facts::BlockFactsTable::default();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:bamboo_jungle").unwrap(),
            ),
        )
        .unwrap();
    let bottom = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    let growing = bamboo_state(&registry, "0", "none", "0");
    for y in 65..=79 {
        world
            .set_block_at(mc_world::BlockPos { y, ..bottom }, growing)
            .unwrap();
    }

    let top = mc_world::BlockPos { y: 79, ..bottom };
    let edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        top,
        growing,
        mc_data::block_facts::RandomTickFamily::Crop,
        0,
    )
    .expect("height-fifteen bamboo growth");
    let terminal_pos = mc_world::BlockPos { y: 80, ..bottom };
    assert_eq!(
        edits.last(),
        Some(&BlockEdit {
            pos: terminal_pos,
            new_state: bamboo_state(&registry, "1", "small", "1"),
        })
    );
    for edit in edits {
        world.set_block_at(edit.pos, edit.new_state).unwrap();
    }
    let terminal = world.get_block(terminal_pos).unwrap().unwrap();
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            terminal_pos,
            terminal,
            mc_data::block_facts::RandomTickFamily::Crop,
            3,
        ),
        None
    );
}

#[test]
fn crop_random_tick_advances_supported_age_crops_until_mature() {
    let blocks = crop_test_registry();

    for (crop, first_state) in [
        ("minecraft:wheat", 11),
        ("minecraft:carrots", 20),
        ("minecraft:potatoes", 28),
        ("minecraft:beetroots", 36),
        ("minecraft:nether_wart", 44),
    ] {
        let crop = Identifier::parse(crop).unwrap();
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state)),
            Some(mc_world::BlockStateId(first_state + 1)),
            "{crop} age 0 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 6)),
            Some(mc_world::BlockStateId(first_state + 7)),
            "{crop} age 6 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 7)),
            None,
            "{crop} max age should not advance"
        );
    }

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(1)),
        None
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(0)),
        None
    );
}

#[test]
fn farmland_random_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let reports = vec![
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: prop_schema(&[("moisture", &["0", "1"])]),
            states: vec![
                state(1, true, &[("moisture", "0")]),
                state(2, false, &[("moisture", "1")]),
            ],
        },
        simple_block(3, "minecraft:water"),
    ];
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let edge_farmland = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    world.set_block_at(edge_farmland, BlockStateId(2)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            edge_farmland,
            BlockStateId(2),
            mc_data::block_facts::RandomTickFamily::Farmland,
        ),
        Some(vec![BlockEdit {
            pos: edge_farmland,
            new_state: BlockStateId(1),
        }])
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn vertical_plant_random_tick_does_not_materialize_neighbour_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let registry = Arc::new(fluid_test_registry());
    let facts = fluid_test_facts();
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 16)
        .with_generator(Arc::new(CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        }));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    let support = mc_world::BlockPos {
        x: 15,
        y: 64,
        z: 15,
    };
    let cactus = mc_world::BlockPos {
        x: 15,
        y: 65,
        z: 15,
    };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(cactus, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn stem_crop_growth_advances_melon_and_pumpkin_stems_once() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    for (stem, first_state) in [("minecraft:pumpkin_stem", 52), ("minecraft:melon_stem", 54)] {
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state)),
            Some(mc_world::BlockStateId(first_state + 1)),
            "{stem} age 0 should advance"
        );
        assert_eq!(
            next_crop_growth_state(&blocks, mc_world::BlockStateId(first_state + 1)),
            None,
            "{stem} max fixture age should not advance"
        );
        assert_eq!(
            bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(first_state)),
            Some(BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(first_state + 1),
            }),
            "{stem} bonemeal should advance by one age"
        );
    }
}

#[test]
fn mature_stem_growth_places_fruit_and_attaches_stem() {
    let registry = Arc::new(crop_test_registry());
    let blocks = registry.as_ref();
    let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let melon_stem = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let melon_fruit = mc_world::BlockPos { x: 4, y: 64, z: 3 };
    world.set_block_at(melon_stem, BlockStateId(55)).unwrap();
    assert_eq!(
        random_tick_edit(
            blocks,
            &mc_data::block_facts::BlockFactsTable::default(),
            &world,
            melon_stem,
            BlockStateId(55),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        Some(vec![
            BlockEdit {
                pos: melon_stem,
                new_state: BlockStateId(65),
            },
            BlockEdit {
                pos: melon_fruit,
                new_state: BlockStateId(63),
            },
        ])
    );

    let pumpkin_stem = mc_world::BlockPos { x: 8, y: 64, z: 8 };
    let blocked_north = mc_world::BlockPos { x: 8, y: 64, z: 7 };
    let pumpkin_fruit = mc_world::BlockPos { x: 8, y: 64, z: 9 };
    world.set_block_at(pumpkin_stem, BlockStateId(53)).unwrap();
    world.set_block_at(blocked_north, BlockStateId(1)).unwrap();
    assert_eq!(
        bonemeal_growth_edits(blocks, &world, pumpkin_stem, BlockStateId(53), 0),
        Some(vec![
            BlockEdit {
                pos: pumpkin_stem,
                new_state: BlockStateId(70),
            },
            BlockEdit {
                pos: pumpkin_fruit,
                new_state: BlockStateId(64),
            },
        ])
    );
}

#[test]
fn sweet_berry_bush_growth_advances_until_mature() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(56)),
        Some(mc_world::BlockStateId(57))
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(57)),
        Some(BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(58),
        })
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(58)),
        Some(mc_world::BlockStateId(59))
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(59)),
        None
    );
}

#[test]
fn sweet_berry_harvest_resets_mature_bush_and_drops_berries() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sweet_berries").unwrap(),
        protocol_id: 88,
    }]);
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(58)),
        Some((
            BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(57),
            },
            ItemStack::new(88, 1),
        ))
    );
    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(59)),
        Some((
            BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(57),
            },
            ItemStack::new(88, 2),
        ))
    );
    assert_eq!(
        sweet_berry_harvest(&blocks, &items, pos, mc_world::BlockStateId(57)),
        None
    );

    let missing_berries = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat").unwrap(),
        protocol_id: 50,
    }]);
    assert_eq!(
        sweet_berry_harvest(&blocks, &missing_berries, pos, mc_world::BlockStateId(59)),
        None
    );
}

#[tokio::test]
async fn sweet_berry_harvest_planning_does_not_wait_for_world_writer() {
    let mut state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    state.items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:sweet_berries").unwrap(),
        protocol_id: 88,
    }]));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, mc_world::BlockStateId(59))
            .expect("place mature berry bush");
        storage
            .block_mutation_token(position)
            .expect("berry bush mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (edit, dropped_stack, precondition) = plan_loaded_plant_harvest(&state, position)
        .expect("published mature berry bush should be harvestable");

    assert_eq!(
        edit,
        BlockEdit {
            pos: position,
            new_state: mc_world::BlockStateId(57),
        }
    );
    assert_eq!(dropped_stack, ItemStack::new(88, 2));
    assert_eq!(precondition.expected_state, mc_world::BlockStateId(59));
    assert_eq!(precondition.expected_token, expected_token);
    drop(world_writer);
}

#[test]
fn cocoa_growth_advances_age_without_losing_facing() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(60)),
        Some(mc_world::BlockStateId(61))
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(61)),
        Some(BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(62),
        })
    );
    assert_eq!(
        blocks
            .by_id(mc_world::BlockStateId(62))
            .and_then(|state| block_state_property(state, "facing")),
        Some("north")
    );
    assert_eq!(
        next_crop_growth_state(&blocks, mc_world::BlockStateId(62)),
        None
    );
}

#[test]
fn bonemeal_growth_edit_advances_supported_crop_one_age() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    for (crop, first_state) in [
        ("minecraft:wheat", 11),
        ("minecraft:carrots", 20),
        ("minecraft:potatoes", 28),
        ("minecraft:beetroots", 36),
    ] {
        assert_eq!(
            bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(first_state)),
            Some(BlockEdit {
                pos,
                new_state: mc_world::BlockStateId(first_state + 1),
            }),
            "{crop} should advance by one registered age state"
        );
    }
}

#[test]
fn bonemeal_growth_edit_ignores_mature_and_invalid_targets() {
    let blocks = crop_test_registry();
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 2 };

    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(18)),
        None
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(1)),
        None
    );
    assert_eq!(
        bonemeal_growth_edit(&blocks, pos, mc_world::BlockStateId(44)),
        None,
        "nether wart grows through random ticks but must reject bonemeal"
    );
}

#[tokio::test]
async fn bonemeal_planning_does_not_wait_for_world_writer() {
    let state = interaction_state_for_blocks(Arc::new(crop_test_registry()));
    insert_fluid_test_chunk(&state).await;
    let position = mc_world::BlockPos { x: 1, y: 64, z: 2 };
    let expected_token = {
        let mut storage = state.world.lock().await;
        storage
            .set_block_at(position, mc_world::BlockStateId(11))
            .expect("place young wheat");
        storage
            .block_mutation_token(position)
            .expect("wheat mutation token")
    };

    let world = Arc::clone(&state.world);
    let world_writer = world.lock().await;
    let (edits, preconditions) = plan_loaded_bonemeal_growth(&state, position, 0)
        .expect("published young wheat should accept bonemeal");

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: position,
            new_state: mc_world::BlockStateId(12),
        }]
    );
    assert_eq!(
        preconditions,
        vec![BlockEditPrecondition {
            pos: position,
            expected_state: mc_world::BlockStateId(11),
            expected_token,
        }]
    );
    drop(world_writer);
}

#[test]
fn sapling_bonemeal_advances_stage_before_growing_a_varied_oak_tree() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let stage_edit =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(1), 0).unwrap();
    assert_eq!(
        stage_edit,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(27)
        }]
    );

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &stage_edit {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let synced =
        consume_bonemeal_after_growth(&mut inventory, 0, !outcome.applied.is_empty()).unwrap();
    assert_eq!(synced.count, 1);
    assert_eq!(inventory.held(0).unwrap().count, 1);

    let short_tree =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0).unwrap();
    let tall_tree =
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 2).unwrap();
    let trunk_height = |edits: &[BlockEdit]| {
        edits
            .iter()
            .filter(|edit| {
                edit.pos.x == pos.x && edit.pos.z == pos.z && edit.new_state == BlockStateId(3)
            })
            .count()
    };
    assert_eq!(trunk_height(&short_tree), 4);
    assert_eq!(trunk_height(&tall_tree), 6);
    assert!(short_tree.iter().any(|edit| {
        edit.pos == mc_world::BlockPos { x: 4, y: 68, z: 4 } && edit.new_state == BlockStateId(4)
    }));
}

#[test]
fn single_sapling_bonemeal_uses_matching_log_and_leaves() {
    let registry = sapling_tree_test_registry();

    for (sapling_state, log_state, leaves_state) in
        [(29, 9, 10), (30, 13, 14), (31, 17, 18), (32, 21, 22)]
    {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        world
            .set_block_at(pos, BlockStateId(sapling_state))
            .unwrap();

        let edits = bonemeal_growth_edits(
            registry.as_ref(),
            &world,
            pos,
            BlockStateId(sapling_state),
            0,
        )
        .unwrap();

        assert_eq!(
            edits[0],
            BlockEdit {
                pos,
                new_state: BlockStateId(log_state)
            },
            "sapling state {sapling_state} should use its matching log"
        );
        assert!(
            edits
                .iter()
                .any(|edit| edit.new_state == BlockStateId(leaves_state)),
            "sapling state {sapling_state} should use its matching leaves"
        );
    }
}

#[test]
fn dark_oak_bonemeal_requires_a_complete_two_by_two_square() {
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };

    for (state, neighbors) in [
        (BlockStateId(23), 0),
        (BlockStateId(33), 0),
        (BlockStateId(33), 2),
    ] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, state).unwrap();
        for offset in [(1, 0), (0, 1)].into_iter().take(neighbors) {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x: pos.x + offset.0,
                        z: pos.z + offset.1,
                        ..pos
                    },
                    BlockStateId(23),
                )
                .unwrap();
        }

        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &world, pos, state, 0),
            None,
            "dark oak must not consume bone meal without all four saplings"
        );
        assert_eq!(world.get_block(pos).unwrap(), Some(state));
    }
}

#[test]
fn dark_oak_two_by_two_uses_one_anchor_and_replaces_all_four_saplings() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        world.set_block_at(pos, BlockStateId(33)).unwrap();
    }

    let expected = bonemeal_growth_edits(registry.as_ref(), &world, northwest, BlockStateId(33), 0)
        .expect("complete dark oak square grows");
    for clicked in saplings {
        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &world, clicked, BlockStateId(33), 0,),
            Some(expected.clone()),
            "each sapling must resolve the same northwest corner"
        );
    }
    for pos in saplings {
        assert!(expected.contains(&BlockEdit {
            pos,
            new_state: BlockStateId(25),
        }));
    }
}

#[test]
fn dark_oak_two_by_two_rejects_unloaded_canopy_without_partial_edits() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 14, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 15, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 15,
            z: 5,
            ..northwest
        },
    ];
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        world.set_block_at(pos, BlockStateId(33)).unwrap();
    }

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, northwest, BlockStateId(33), 0,),
        None
    );
    for pos in saplings {
        assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(33)));
    }
}

#[test]
fn spruce_and_jungle_two_by_two_use_one_anchor_and_replace_all_four_saplings() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];

    for (state, log, leaves) in [
        (BlockStateId(30), BlockStateId(13), BlockStateId(14)),
        (BlockStateId(31), BlockStateId(17), BlockStateId(18)),
    ] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        for pos in saplings {
            world.set_block_at(pos, state).unwrap();
        }

        let expected = bonemeal_growth_edits(registry.as_ref(), &world, northwest, state, 0)
            .expect("complete spruce and jungle squares grow");
        for clicked in saplings {
            assert_eq!(
                bonemeal_growth_edits(registry.as_ref(), &world, clicked, state, 0),
                Some(expected.clone()),
                "each sapling must resolve the same northwest corner"
            );
        }
        for pos in saplings {
            assert!(expected.contains(&BlockEdit {
                pos,
                new_state: log,
            }));
        }
        assert!(expected.iter().any(|edit| edit.new_state == leaves));
    }
}

#[test]
fn spruce_and_jungle_two_by_two_reject_obstruction_or_unloaded_canopy_atomically() {
    let registry = sapling_tree_test_registry();

    for (state, leaves) in [
        (BlockStateId(30), BlockStateId(14)),
        (BlockStateId(31), BlockStateId(18)),
    ] {
        let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        let saplings = [
            northwest,
            mc_world::BlockPos { x: 5, ..northwest },
            mc_world::BlockPos { z: 5, ..northwest },
            mc_world::BlockPos {
                x: 5,
                z: 5,
                ..northwest
            },
        ];
        let mut clear_world = in_memory_tree_world(Arc::clone(&registry));
        for pos in saplings {
            clear_world.set_block_at(pos, state).unwrap();
        }
        let clear_edits =
            bonemeal_growth_edits(registry.as_ref(), &clear_world, northwest, state, 0)
                .expect("clear mega-tree space grows");
        let blocked = clear_edits
            .iter()
            .find(|edit| !saplings.contains(&edit.pos) && edit.new_state == leaves)
            .expect("mega-tree template has an obstruction target")
            .pos;
        clear_world.set_block_at(blocked, BlockStateId(5)).unwrap();

        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &clear_world, northwest, state, 0),
            None
        );
        for pos in saplings {
            assert_eq!(clear_world.get_block(pos).unwrap(), Some(state));
        }

        let edge = mc_world::BlockPos { x: 14, ..northwest };
        let edge_saplings = [
            edge,
            mc_world::BlockPos { x: 15, ..edge },
            mc_world::BlockPos { z: 5, ..edge },
            mc_world::BlockPos {
                x: 15,
                z: 5,
                ..edge
            },
        ];
        let mut edge_world = in_memory_tree_world(Arc::clone(&registry));
        for pos in edge_saplings {
            edge_world.set_block_at(pos, state).unwrap();
        }
        assert_eq!(
            bonemeal_growth_edits(registry.as_ref(), &edge_world, edge, state, 0),
            None
        );
        for pos in edge_saplings {
            assert_eq!(edge_world.get_block(pos).unwrap(), Some(state));
        }
    }
}

#[tokio::test]
async fn dark_oak_two_by_two_stale_sapling_token_rejects_the_whole_edit_set() {
    let registry = sapling_tree_test_registry();
    let northwest = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let saplings = [
        northwest,
        mc_world::BlockPos { x: 5, ..northwest },
        mc_world::BlockPos { z: 5, ..northwest },
        mc_world::BlockPos {
            x: 5,
            z: 5,
            ..northwest
        },
    ];
    let mut storage = in_memory_tree_world(Arc::clone(&registry));
    for pos in saplings {
        storage.set_block_at(pos, BlockStateId(33)).unwrap();
    }
    let edits = bonemeal_growth_edits(registry.as_ref(), &storage, northwest, BlockStateId(33), 0)
        .expect("complete dark oak square plans one edit set");
    let preconditions = edits
        .iter()
        .map(|edit| BlockEditPrecondition {
            pos: edit.pos,
            expected_state: storage.get_block(edit.pos).unwrap().unwrap(),
            expected_token: storage.block_mutation_token(edit.pos).unwrap(),
        })
        .collect::<Vec<_>>();

    storage.set_block_at(saplings[3], BlockStateId(0)).unwrap();
    storage.set_block_at(saplings[3], BlockStateId(33)).unwrap();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let sessions = SessionRegistry::new();
    let session = register_ticketed_button_session(&sessions, "DarkOakStaleToken");
    let _ = sessions.mark_loaded(session, (0, 0));
    let (handle, mut owner) = simulation_channel();
    let session_handle = handle.for_session(session);
    let mut growth = Box::pin(session_handle.apply_block_edits(edits, preconditions));
    let waker = std::task::Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(
        std::future::Future::poll(growth.as_mut(), &mut context),
        Poll::Pending
    ));
    assert_eq!(
        owner
            .process_tick_with_world(&sessions, Some(&world), None, 1)
            .processed,
        1
    );
    assert!(growth.await.unwrap().is_none());

    let mut storage = world.lock().await;
    for pos in saplings {
        assert_eq!(storage.get_block(pos).unwrap(), Some(BlockStateId(33)));
    }
    assert_eq!(
        storage
            .get_block(mc_world::BlockPos {
                y: northwest.y + 1,
                ..northwest
            })
            .unwrap(),
        Some(BlockStateId(0))
    );
}

#[test]
fn stage_one_oak_sapling_replaces_leaves_and_supported_canopy_vegetation() {
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let canopy_pos = mc_world::BlockPos { x: 5, y: 68, z: 4 };

    for existing in [BlockStateId(4), BlockStateId(34), BlockStateId(35)] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, BlockStateId(27)).unwrap();
        world.set_block_at(canopy_pos, existing).unwrap();

        let edits = bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0)
            .expect("replaceable canopy vegetation permits oak growth");
        assert!(
            edits
                .iter()
                .any(|edit| { edit.pos == canopy_pos && edit.new_state == BlockStateId(4) })
        );
    }
}

#[test]
fn stage_one_oak_sapling_accepts_exact_vanilla_tree_replaceable_membership() {
    assert_eq!(
        VANILLA_26_1_2_TREE_REPLACEABLES.len() + 2,
        55,
        "53 concrete blocks plus the leaves and small_flowers tag members"
    );
    let registry = sapling_tree_test_registry();
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let canopy_pos = mc_world::BlockPos { x: 5, y: 68, z: 4 };

    for name in VANILLA_26_1_2_TREE_REPLACEABLES {
        let state = registry
            .block(&Identifier::parse(name).unwrap())
            .unwrap_or_else(|| panic!("missing tree-replaceable fixture {name}"))
            .default;
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        world.set_block_at(pos, BlockStateId(27)).unwrap();
        world.set_block_at(canopy_pos, state).unwrap();

        let edits = bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0)
            .unwrap_or_else(|| panic!("tree planner rejected vanilla replaceable {name}"));
        assert!(edits.iter().any(|edit| edit.pos == canopy_pos));
    }
}

#[test]
fn stage_one_oak_sapling_rejects_unloaded_canopy_atomically() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 14, y: 64, z: 4 };
    let loaded_trunk_cell = mc_world::BlockPos { y: 65, ..pos };
    world.set_block_at(pos, BlockStateId(27)).unwrap();

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));
    assert_eq!(
        world.get_block(loaded_trunk_cell).unwrap(),
        Some(BlockStateId(0))
    );
}

#[test]
fn stage_zero_sapling_advances_even_when_tree_space_is_blocked() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(1), 0),
        Some(vec![BlockEdit {
            pos,
            new_state: BlockStateId(27),
        }])
    );
    world.set_block_at(pos, BlockStateId(27)).unwrap();
    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(27), 0),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 2,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).unwrap().count, 2);
}

#[test]
fn sapling_bonemeal_unsupported_and_missing_tree_states_are_noop() {
    let registry = sapling_tree_test_registry();
    let world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &world, pos, BlockStateId(6), 0),
        None
    );

    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_sapling"),
        simple_block(2, "minecraft:oak_leaves"),
    ];
    let missing_registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let world = in_memory_tree_world(Arc::clone(&missing_registry));
    assert_eq!(
        bonemeal_growth_edits(missing_registry.as_ref(), &world, pos, BlockStateId(1), 0,),
        None
    );
}

#[test]
fn bonemeal_consumes_exactly_one_item_only_after_successful_growth() {
    let mut inventory = PlayerInventory::empty();
    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 3,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();

    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).unwrap().count, 3);

    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert_eq!(synced.count, 2);
    assert_eq!(inventory.held(0).unwrap().count, 2);

    inventory
        .set_hotbar(
            0,
            ItemStack {
                item_id: 99,
                count: 1,
                damage: None,
                enchantments: Vec::new(),
                custom_name: None,
                item_model: None,
            },
        )
        .unwrap();
    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert!(synced.is_empty());
    assert!(inventory.held(0).unwrap().is_empty());
}

#[test]
fn sapling_random_tick_uses_one_in_seven_chance_and_two_growth_stages() {
    let reports = sapling_tree_test_reports();
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    assert_eq!(
        facts.random_tick_family(1),
        Some(mc_data::block_facts::RandomTickFamily::Sapling)
    );
    let edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        pos,
        BlockStateId(1),
        mc_data::block_facts::RandomTickFamily::Sapling,
        0,
    )
    .unwrap();
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(27)
        }]
    );
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(1),
            mc_data::block_facts::RandomTickFamily::Sapling,
            1,
        ),
        None,
        "six of seven selected random ticks must leave the sapling unchanged"
    );

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &edits {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert!(!outcome.applied.is_empty());
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));

    let tree_edits = random_tick_edit_seeded(
        registry.as_ref(),
        &facts,
        &world,
        pos,
        BlockStateId(27),
        mc_data::block_facts::RandomTickFamily::Sapling,
        0,
    )
    .unwrap();
    assert!(
        tree_edits
            .iter()
            .any(|edit| edit.new_state == BlockStateId(3))
    );
    assert!(
        tree_edits
            .iter()
            .any(|edit| edit.new_state == BlockStateId(4))
    );
}

#[test]
fn sapling_random_tick_obstructed_or_unsupported_targets_are_noop() {
    let reports = sapling_tree_test_reports();
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(27)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(27),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(27)));
    assert_eq!(
        random_tick_edit_seeded(
            registry.as_ref(),
            &facts,
            &world,
            pos,
            BlockStateId(6),
            mc_data::block_facts::RandomTickFamily::Sapling,
            0,
        ),
        None
    );
}

#[test]
fn farmland_trample_requires_landing_on_block() {
    let old_pose = PlayerPose::new(2.7, 3.0, -1.2);
    let landed = PlayerPose {
        y: 1.0,
        flags: MovePlayerFlags::new(true, false),
        ..old_pose
    };
    let hovering = PlayerPose {
        flags: MovePlayerFlags::new(false, false),
        ..landed
    };

    assert_eq!(
        farmland_trample_pos(old_pose, landed),
        Some(mc_world::BlockPos { x: 2, y: 0, z: -2 })
    );
    assert_eq!(farmland_trample_pos(old_pose, hovering), None);
    assert_eq!(farmland_trample_pos(landed, landed), None);
}

#[tokio::test]
async fn farmland_trample_does_not_overwrite_a_newer_block_state() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:farmland"),
            simple_block(2, "minecraft:dirt"),
            simple_block(3, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = blocks;
    state.world = Arc::clone(&world);
    state.world_read = world_read;
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let mut storage = world.lock().await;
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(pos, BlockStateId(1)).unwrap();

    let old_pose = PlayerPose::new(1.5, 66.0, 1.5);
    let new_pose = PlayerPose {
        y: 65.0,
        flags: MovePlayerFlags::new(true, false),
        ..old_pose
    };
    let mut writer = Vec::new();
    let mut trample = Box::pin(maybe_trample_farmland(
        &mut state,
        &mut writer,
        old_pose,
        new_pose,
    ));
    std::future::poll_fn(|cx| {
        assert!(
            std::future::Future::poll(trample.as_mut(), cx).is_pending(),
            "trample must wait for the held world writer"
        );
        Poll::Ready(())
    })
    .await;

    storage.set_block_at(pos, BlockStateId(3)).unwrap();
    drop(storage);
    trample.await.unwrap();

    let storage = world.lock().await;
    assert_eq!(storage.get_cached_block(pos), Some(BlockStateId(3)));
}

#[tokio::test]
async fn hoe_tilling_plan_does_not_wait_for_writer_and_guards_the_block_above() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&[
            simple_block(0, "minecraft:air"),
            simple_block(1, "minecraft:dirt"),
            simple_block(2, "minecraft:farmland"),
            simple_block(3, "minecraft:stone"),
        ])
        .unwrap(),
    );
    let storage = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
    let world_read = storage.read_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let mut state = interaction_state_for_items(Arc::new(ItemRegistry::default()));
    state.blocks = Arc::clone(&blocks);
    state.world = Arc::clone(&world);
    state.world_read = world_read;

    let clicked = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    let above = mc_world::BlockPos { y: 65, ..clicked };
    let mut storage = world.lock().await;
    let cpos = ChunkPos { x: 0, z: 0 };
    storage
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
    storage.set_block_at(clicked, BlockStateId(1)).unwrap();

    let plan = plan_hoe_tilling(&state, clicked, BlockStateId(2)).expect("tillable dirt plan");
    assert_eq!(plan.edits.len(), 1);
    assert_eq!(plan.preconditions.len(), 2);
    assert!(
        plan.preconditions
            .iter()
            .any(|guard| { guard.pos == clicked && guard.expected_state == BlockStateId(1) })
    );
    assert!(
        plan.preconditions
            .iter()
            .any(|guard| { guard.pos == above && guard.expected_state == BlockStateId(0) })
    );

    storage.set_block_at(above, BlockStateId(3)).unwrap();
    assert!(
        apply_block_edit_batch_to_storage_conditionally(
            &mut storage,
            None,
            &plan.edits,
            &plan.preconditions,
        )
        .is_none(),
        "a block placed above after planning must reject tilling"
    );
    assert_eq!(storage.get_cached_block(clicked), Some(BlockStateId(1)));
}
