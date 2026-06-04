use super::*;
use std::collections::BTreeMap;

use crate::play::chunk_stream::{hostile_chunk_spawns, passive_chunk_spawns, prioritized_spiral};
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::ItemReport;

fn props(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[test]
fn player_pose_metadata_reports_swimming_and_shared_flags() {
    let mut pose = PlayerPose::new(0.5, 62.0, 0.5);
    pose.in_water = true;
    pose.swimming = true;
    pose.sprinting = true;

    assert_eq!(pose.entity_pose(), EntityPose::Swimming);
    assert_eq!(pose.shared_flags() & 0x08, 0x08);
}

#[test]
fn clientbound_session_world_time_uses_current_tick_with_saturation() {
    let sessions = SessionRegistry::new();
    sessions.set_world_time(12_345);
    assert_eq!(clientbound_session_world_time(&sessions).game_time, 12_345);

    assert_eq!(clientbound_world_time(u64::MAX).game_time, i64::MAX);
}

fn state(id: u32, default: bool, properties: &[(&str, &str)]) -> BlockStateReport {
    BlockStateReport {
        id,
        default,
        properties: props(properties),
    }
}

fn simple_block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(id, true, &[])],
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

fn prop_schema(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(key, values)| {
            (
                (*key).to_string(),
                values.iter().map(|value| (*value).to_string()).collect(),
            )
        })
        .collect()
}

fn crop_test_reports() -> Vec<BlockReport> {
    let mut farmland_properties = BTreeMap::new();
    farmland_properties.insert(
        "moisture".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );
    let mut crop_properties = BTreeMap::new();
    crop_properties.insert(
        "age".to_string(),
        (0..=7).map(|value| value.to_string()).collect(),
    );

    let mut reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        simple_block(2, "minecraft:water"),
        simple_block(19, "minecraft:soul_sand"),
        BlockReport {
            id: Identifier::parse("minecraft:farmland").unwrap(),
            properties: farmland_properties,
            states: (0..=7)
                .map(|moisture| {
                    state(
                        3 + moisture,
                        moisture == 0,
                        &[("moisture", &moisture.to_string())],
                    )
                })
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:wheat").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(11 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:carrots").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(20 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:potatoes").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(28 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:beetroots").unwrap(),
            properties: crop_properties.clone(),
            states: (0..=7)
                .map(|age| state(36 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:nether_wart").unwrap(),
            properties: crop_properties,
            states: (0..=7)
                .map(|age| state(44 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:pumpkin_stem").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: (0..=1)
                .map(|age| state(52 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:melon_stem").unwrap(),
            properties: prop_schema(&[("age", &["0", "1"])]),
            states: (0..=1)
                .map(|age| state(54 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:sweet_berry_bush").unwrap(),
            properties: prop_schema(&[("age", &["0", "1", "2", "3"])]),
            states: (0..=3)
                .map(|age| state(56 + age, age == 0, &[("age", &age.to_string())]))
                .collect(),
        },
        BlockReport {
            id: Identifier::parse("minecraft:cocoa").unwrap(),
            properties: prop_schema(&[("age", &["0", "1", "2"]), ("facing", &["north"])]),
            states: (0..=2)
                .map(|age| {
                    state(
                        60 + age,
                        age == 0,
                        &[("age", &age.to_string()), ("facing", "north")],
                    )
                })
                .collect(),
        },
        simple_block(63, "minecraft:melon"),
        simple_block(64, "minecraft:pumpkin"),
        attached_stem_block(65, "minecraft:attached_melon_stem"),
        attached_stem_block(69, "minecraft:attached_pumpkin_stem"),
        simple_block(73, "minecraft:jungle_log"),
    ];
    reports.sort_by_key(|block| block.states.first().map(|state| state.id).unwrap_or(0));
    reports
}

fn attached_stem_block(first_id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: prop_schema(&[("facing", &["north", "south", "west", "east"])]),
        states: ["north", "south", "west", "east"]
            .into_iter()
            .enumerate()
            .map(|(offset, facing)| {
                state(first_id + offset as u32, offset == 0, &[("facing", facing)])
            })
            .collect(),
    }
}

fn crop_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&crop_test_reports()).unwrap()
}

fn sapling_tree_test_reports() -> Vec<BlockReport> {
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_sapling"),
        axis_log_block(2, "minecraft:oak_log"),
        tree_leaves_block(4, "minecraft:oak_leaves"),
        simple_block(5, "minecraft:stone"),
        simple_block(6, "minecraft:cherry_sapling"),
        simple_block(7, "minecraft:birch_sapling"),
        axis_log_block(8, "minecraft:birch_log"),
        tree_leaves_block(10, "minecraft:birch_leaves"),
        simple_block(11, "minecraft:spruce_sapling"),
        axis_log_block(12, "minecraft:spruce_log"),
        tree_leaves_block(14, "minecraft:spruce_leaves"),
        simple_block(15, "minecraft:jungle_sapling"),
        axis_log_block(16, "minecraft:jungle_log"),
        tree_leaves_block(18, "minecraft:jungle_leaves"),
        simple_block(19, "minecraft:acacia_sapling"),
        axis_log_block(20, "minecraft:acacia_log"),
        tree_leaves_block(22, "minecraft:acacia_leaves"),
        simple_block(23, "minecraft:dark_oak_sapling"),
        axis_log_block(24, "minecraft:dark_oak_log"),
        tree_leaves_block(26, "minecraft:dark_oak_leaves"),
    ]
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
fn block_drop_configured_loot_count_reaches_runtime_stack() {
    let blocks = crop_test_registry();
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:carrot").unwrap(),
        protocol_id: 52,
    }]);
    let dirt = blocks
        .block(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap()
        .default;
    let loot = mc_data::loot::LootTables::from_drop_maps(
        BTreeMap::new(),
        BTreeMap::from([(
            Identifier::parse("minecraft:dirt").unwrap(),
            mc_data::loot::LootDrop {
                item: Identifier::parse("minecraft:carrot").unwrap(),
                count: 3,
            },
        )]),
    );

    let drops = block_drop_stacks_from(&loot, &items, &blocks, dirt);

    assert_eq!(drops, vec![ItemStack::new(52, 3)]);
}

#[test]
fn wheat_crop_drop_missing_item_ids_omit_unavailable_stacks() {
    let blocks = crop_test_registry();
    let wheat = test_crop_state_with_age(&blocks, "minecraft:wheat", 7);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:wheat_seeds").unwrap(),
        protocol_id: 51,
    }]);
    let missing_all = ItemRegistry::default();

    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            wheat,
        ),
        vec![ItemStack::new(51, 1)]
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
fn carrot_crop_drop_missing_item_id_omits_unavailable_stack() {
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
fn potato_crop_drop_missing_item_id_omits_unavailable_stack() {
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
fn beetroot_crop_drop_missing_item_ids_omit_unavailable_stacks() {
    let blocks = crop_test_registry();
    let beetroots = test_crop_state_with_age(&blocks, "minecraft:beetroots", 3);
    let seeds_only = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:beetroot_seeds").unwrap(),
        protocol_id: 56,
    }]);
    let missing_all = ItemRegistry::default();

    assert_eq!(
        block_drop_stacks_from(
            &mc_data::loot::LootTables::default(),
            &seeds_only,
            &blocks,
            beetroots,
        ),
        vec![ItemStack::new(56, 1)]
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
fn nether_wart_crop_drop_missing_item_id_omits_unavailable_stack() {
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
fn cocoa_crop_drop_missing_item_id_omits_unavailable_stack() {
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

fn fluid_block(first_id: u32, name: &str, max_level: u8) -> BlockReport {
    let mut properties = BTreeMap::new();
    properties.insert(
        "level".to_string(),
        (0..=max_level).map(|level| level.to_string()).collect(),
    );
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties,
        states: (0..=max_level)
            .map(|level| {
                state(
                    first_id + u32::from(level),
                    level == 0,
                    &[("level", &level.to_string())],
                )
            })
            .collect(),
    }
}

fn fluid_test_reports() -> Vec<BlockReport> {
    vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:stone"),
        fluid_block(2, "minecraft:water", 7),
        fluid_block(10, "minecraft:lava", 3),
        simple_block(14, "minecraft:obsidian"),
        simple_block(15, "minecraft:cobblestone"),
        simple_block(16, "minecraft:sand"),
        simple_block(17, "minecraft:gravel"),
        simple_block(18, "minecraft:anvil"),
        simple_block(19, "minecraft:cactus"),
        simple_block(20, "minecraft:bamboo"),
        simple_block(21, "minecraft:sugar_cane"),
    ]
}

fn fluid_test_registry() -> mc_world::BlockRegistry {
    mc_world::BlockRegistry::from_report(&fluid_test_reports()).unwrap()
}

fn fluid_test_facts() -> mc_data::block_facts::BlockFactsTable {
    mc_data::block_facts::BlockFactsTable::from_blocks_report(&fluid_test_reports())
}

fn interaction_state_for_blocks(blocks: Arc<mc_world::BlockRegistry>) -> InteractionState {
    let items = Arc::new(ItemRegistry::from_report(&[]));
    let world = Arc::new(tokio::sync::Mutex::new(mc_world::WorldStorage::in_memory(
        Arc::clone(&blocks),
    )));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    InteractionState {
        world,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        inventory_state_id: 1,
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(EntityTypeRegistry::default()),
        item_to_block,
        tags: Arc::new(TagsData::default()),
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_hostile_damage_at: None,
        last_entity_attack_at: None,
        fluid_schedule_tick: 0,
    }
}

async fn insert_fluid_test_chunk(state: &InteractionState) {
    let cpos = ChunkPos { x: 0, z: 0 };
    state
        .world
        .lock()
        .await
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();
}

#[test]
fn entity_tick_cadence_matches_vanilla_cow_tracking() {
    assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
    assert_eq!(mc_physics::TICK_SECONDS, 0.05);
    assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 1);
}

#[test]
fn arrow_launch_uses_player_look_direction_and_draw_power() {
    let pose = PlayerPose {
        yaw: 90.0,
        pitch: -30.0,
        ..PlayerPose::new(1.0, 64.0, 2.0)
    };

    let spawn = arrow_spawn_position(pose);
    let velocity = arrow_velocity(pose, 0.5);

    assert!((spawn.x - 1.0).abs() < 0.000_001);
    assert!((spawn.y - 65.62).abs() < 0.000_001);
    assert!((spawn.z - 2.0).abs() < 0.000_001);
    assert!((velocity.x + 1.299_038_105_676_658).abs() < 0.000_001);
    assert!((velocity.y - 0.75).abs() < 0.000_001);
    assert!(velocity.z.abs() < 0.000_001);
}

#[test]
fn gamemode_command_parses_names_and_numeric_modes() {
    assert_eq!(
        parse_gamemode_command("gamemode survival"),
        Some(GameMode::Survival)
    );
    assert_eq!(
        parse_gamemode_command("gamemode creative"),
        Some(GameMode::Creative)
    );
    assert_eq!(
        parse_gamemode_command("gamemode adventure"),
        Some(GameMode::Adventure)
    );
    assert_eq!(
        parse_gamemode_command("gamemode spectator"),
        Some(GameMode::Spectator)
    );
    assert_eq!(
        parse_gamemode_command("gamemode 1"),
        Some(GameMode::Creative)
    );
}

#[test]
fn gamemode_command_rejects_unknown_or_extra_args() {
    assert_eq!(parse_gamemode_command("time set day"), None);
    assert_eq!(parse_gamemode_command("gamemode nope"), None);
    assert_eq!(parse_gamemode_command("gamemode creative other"), None);
}

#[test]
fn client_view_distance_is_clamped_to_server_policy() {
    assert_eq!(clamp_client_view_distance(12, 8), 8);
    assert_eq!(clamp_client_view_distance(6, 10), 6);
    assert_eq!(clamp_client_view_distance(0, 10), 2);
    assert_eq!(clamp_client_view_distance(-8, 1), 2);
}

#[test]
fn debug_commands_parse_survival_mutations_and_give() {
    assert_eq!(
        parse_debug_command("debug survival damage 7.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Damage(7.5)))
    );
    assert_eq!(
        parse_debug_command("debug survival heal"),
        Some(DebugCommand::Survival(SurvivalCommand::Heal(20.0)))
    );
    assert_eq!(
        parse_debug_command("debug survival feed 2 0.5"),
        Some(DebugCommand::Survival(SurvivalCommand::Feed {
            food: 2,
            saturation: 0.5
        }))
    );
    assert_eq!(
        parse_debug_command("debug survival exhaust 4"),
        Some(DebugCommand::Survival(SurvivalCommand::Exhaust(4.0)))
    );
    assert_eq!(
        parse_debug_command("debug give minecraft:dirt 64 1"),
        Some(DebugCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 64,
            hotbar_slot: 1,
        })
    );
    assert_eq!(parse_debug_command("damage 7.5"), None);
    assert_eq!(parse_debug_command("debug survival damage bad"), None);
}

#[test]
fn admin_dispatcher_parses_slash_commands_and_permissions() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    assert_eq!(
        parse_admin_command("/gamemode creative", op),
        Ok(AdminCommand::GameMode(GameMode::Creative))
    );
    assert_eq!(
        parse_admin_command("give minecraft:dirt 12", op),
        Ok(AdminCommand::Give {
            item: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            count: 12,
        })
    );
    assert_eq!(
        parse_admin_command("/tp 1.5 70 -2", op),
        Ok(AdminCommand::Teleport {
            x: 1.5,
            y: 70.0,
            z: -2.0,
        })
    );
    assert_eq!(
        parse_admin_command("/summon minecraft:zombie", op),
        Ok(AdminCommand::Summon {
            entity: mc_data::Identifier::parse("minecraft:zombie").unwrap(),
            x: None,
            y: None,
            z: None,
        })
    );
    assert_eq!(parse_admin_command("/kill", op), Ok(AdminCommand::Kill));
    assert_eq!(
        parse_admin_command("/gamemode creative", not_op),
        Err(CommandError::PermissionDenied)
    );
    assert_eq!(
        parse_admin_command("/gamemode", op),
        Err(CommandError::Usage(
            "Usage: /gamemode <survival|creative|adventure|spectator>"
        ))
    );
    assert_eq!(
        parse_admin_command("/doesnotexist", op),
        Err(CommandError::Unknown)
    );
}

#[test]
fn command_tree_and_suggestions_are_permission_aware() {
    let op = CommandPermissions { op: true };
    let not_op = CommandPermissions { op: false };

    let tree = command_tree_packet(op);
    assert_eq!(tree.root_index, 0);
    assert_eq!(
        tree.nodes[0].children,
        vec![1, 6, 8, 10, 11, 12, 13, 15, 17]
    );
    assert_eq!(
        command_tree_packet(not_op).nodes[0].children,
        Vec::<i32>::new()
    );

    let root = command_suggestions("/g", op);
    assert_eq!(root.start, 1);
    assert_eq!(root.length, 1);
    assert_eq!(
        root.suggestions,
        vec!["gamemode".to_string(), "give".to_string()]
    );

    let modes = command_suggestions("/gamemode c", op);
    assert_eq!(modes.start, 10);
    assert_eq!(modes.length, 1);
    assert_eq!(modes.suggestions, vec!["creative".to_string()]);

    assert!(command_suggestions("/g", not_op).suggestions.is_empty());
}

#[test]
fn local_dev_profiles_are_op_capable_for_now() {
    let profile = LoggedInProfile {
        uuid: uuid::Uuid::nil(),
        name: "op_probe".to_string(),
    };

    let permissions = crate::server::CommandPermissionConfig::new(Vec::<String>::new(), true)
        .permissions_for(&profile);

    assert!(permissions.can_change_game_mode());
    assert!(permissions.can_use_admin_commands());
}

#[test]
fn item_to_block_table_is_registry_derived() {
    use std::collections::BTreeMap;

    use mc_data::blocks::{BlockReport, BlockStateReport};
    use mc_data::items::ItemReport;

    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            protocol_id: 42,
        },
        ItemReport {
            id: mc_data::Identifier::parse("minecraft:apple").unwrap(),
            protocol_id: 43,
        },
    ]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: mc_data::Identifier::parse("minecraft:dirt").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ])
    .unwrap();

    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.resolve(42), Some(mc_world::BlockStateId(1)));
    assert_eq!(table.resolve(43), None);
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
fn sign_items_choose_floor_or_wall_sign_for_clicked_face() {
    let items = ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:oak_sign").unwrap(),
        protocol_id: 70,
    }]);
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0"])]),
            states: vec![state(1, true, &[("rotation", "0")])],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(2, true, &[("facing", "north")])],
        },
    ])
    .unwrap();

    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Up,
            &blocks,
        ),
        Some(mc_world::BlockStateId(1))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::North,
            &blocks,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        ItemToBlockTable::build(&items, &blocks).resolve_for_use_on(
            &items,
            70,
            mc_world::BlockStateId(0),
            Direction::Down,
            &blocks,
        ),
        None
    );
}

#[test]
fn bucket_items_resolve_fluid_sources() {
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: Identifier::parse("minecraft:bucket").unwrap(),
            protocol_id: 60,
        },
        ItemReport {
            id: Identifier::parse("minecraft:water_bucket").unwrap(),
            protocol_id: 61,
        },
        ItemReport {
            id: Identifier::parse("minecraft:lava_bucket").unwrap(),
            protocol_id: 62,
        },
    ]);
    let blocks = fluid_test_registry();
    let table = ItemToBlockTable::build(&items, &blocks);

    assert_eq!(table.empty_bucket_item(), Some(60));
    assert_eq!(table.bucket_fluid_kind(61), Some(FluidKind::Water));
    assert_eq!(table.bucket_fluid_kind(62), Some(FluidKind::Lava));
    assert_eq!(
        table.fluid_source_state(FluidKind::Water),
        Some(BlockStateId(2))
    );
    assert_eq!(
        table.fluid_source_state(FluidKind::Lava),
        Some(BlockStateId(10))
    );
}

#[test]
fn bucket_replacement_updates_single_held_stack_only() {
    let mut inventory = PlayerInventory::empty();
    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 61,
            count: 1,
            damage: None,
        },
    );

    let (next, changed) = plan_bucket_replacement(&inventory, 0, 60, 16).unwrap();

    assert_eq!(next.held(0).item_id, 60);
    assert_eq!(next.held(0).count, 1);
    assert_eq!(
        changed,
        vec![(PlayerInventory::HOTBAR_BASE, next.held(0).clone())]
    );

    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 60,
            count: 2,
            damage: None,
        },
    );
    assert!(plan_bucket_replacement(&inventory, 0, 61, 1).is_none());
}

#[test]
fn fluid_tick_flows_sideways_when_blocked_below() {
    let facts = fluid_test_facts();
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
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );

    assert_eq!(edits.len(), 4);
    assert!(edits.iter().all(|edit| edit.new_state == BlockStateId(3)));
}

#[test]
fn unsupported_flow_decays_to_air() {
    let facts = fluid_test_facts();
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
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(4)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { y: 63, ..pos }, BlockStateId(1))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        pos,
        BlockStateId(4),
        facts.fluid(4).unwrap(),
    );

    assert_eq!(
        edits,
        vec![BlockEdit {
            pos,
            new_state: BlockStateId(0)
        }]
    );
}

#[test]
fn scheduling_fluid_edits_uses_current_tick_delay() {
    let facts = fluid_test_facts();
    let registry = Arc::new(fluid_test_registry());
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
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(2)).unwrap();

    schedule_fluid_ticks_near_applied(
        &mut world,
        &facts,
        100,
        &[AppliedBlockEdit {
            pos,
            previous: BlockStateId(0),
            new_state: BlockStateId(2),
        }],
    );

    let ticks = world.scheduled_fluid_ticks(cpos).unwrap().unwrap();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].pos, pos);
    assert_eq!(ticks[0].trigger_tick, 100 + WATER_FLOW_DELAY_TICKS);
}

#[test]
fn water_lava_interactions_make_solid_blocks() {
    let facts = fluid_test_facts();
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
    let water_pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    let lava_source_pos = mc_world::BlockPos { x: 5, y: 64, z: 4 };
    world.set_block_at(water_pos, BlockStateId(2)).unwrap();
    world
        .set_block_at(lava_source_pos, BlockStateId(10))
        .unwrap();

    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        water_pos,
        BlockStateId(2),
        facts.fluid(2).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_source_pos,
            new_state: BlockStateId(14),
        }]
    );

    world
        .set_block_at(lava_source_pos, BlockStateId(0))
        .unwrap();
    let lava_flow_pos = mc_world::BlockPos { x: 4, y: 63, z: 4 };
    world.set_block_at(lava_flow_pos, BlockStateId(11)).unwrap();
    let edits = fluid_tick_edits(
        blocks,
        &facts,
        &mut world,
        lava_flow_pos,
        BlockStateId(11),
        facts.fluid(11).unwrap(),
    );
    assert_eq!(
        edits,
        vec![BlockEdit {
            pos: lava_flow_pos,
            new_state: BlockStateId(1),
        }]
    );
}

#[test]
fn falling_block_starts_when_support_edit_becomes_replaceable() {
    let facts = fluid_test_facts();
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
    let sand = mc_world::BlockPos { x: 4, y: 65, z: 4 };
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(sand, BlockStateId(16)).unwrap();

    let starts = collect_falling_block_starts(
        blocks,
        &facts,
        &mut world,
        &[AppliedBlockEdit {
            pos: support,
            previous: BlockStateId(1),
            new_state: BlockStateId(0),
        }],
        BlockStateId(0),
    );

    assert_eq!(
        starts,
        vec![FallingBlockStart {
            pos: sand,
            state: BlockStateId(16),
        }]
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
        &mut world,
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

    append_cactus_side_neighbor_cascades(
        blocks,
        &mut world,
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

    append_cactus_side_neighbor_cascades(
        blocks,
        &mut world,
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

    let edits = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(1),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
    )
    .await;

    assert_eq!(
        edits,
        Some(vec![
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
        ])
    );
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
    }

    let edits = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(20),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
    )
    .await;

    assert_eq!(
        edits,
        Some(vec![BlockEdit {
            pos: placed,
            new_state: BlockStateId(20),
        }])
    );
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

    let edits = plan_place_block_edits(
        &state,
        placed,
        BlockStateId(19),
        PlayerPose::new(0.5, 64.0, 0.5),
        Direction::Up,
    )
    .await;

    assert_eq!(edits, None);
}

#[test]
fn cactus_random_tick_grows_supported_column_to_height_three() {
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
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(cactus_1, BlockStateId(19)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
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
            &mut world,
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
            &mut world,
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
            &mut world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
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
            &mut world,
            cactus,
            BlockStateId(19),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );
}

#[test]
fn sugar_cane_random_tick_grows_supported_column_to_height_three() {
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
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(water, BlockStateId(2)).unwrap();
    world.set_block_at(cane_1, BlockStateId(21)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
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
            &mut world,
            cane_1,
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
            &mut world,
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
            &mut world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(support, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
            cane,
            BlockStateId(21),
            mc_data::block_facts::RandomTickFamily::Crop,
        ),
        None
    );

    world.set_block_at(water, BlockStateId(2)).unwrap();
    world.set_block_at(above, BlockStateId(1)).unwrap();
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
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
        &mut world,
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
fn bamboo_random_tick_grows_supported_column_to_height_three() {
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
    world.set_block_at(support, BlockStateId(1)).unwrap();
    world.set_block_at(bamboo_1, BlockStateId(20)).unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
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
            &mut world,
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
            &mut world,
            bamboo_1,
            BlockStateId(20),
            mc_data::block_facts::RandomTickFamily::Crop,
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
            &mut world,
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
        bonemeal_growth_edits(blocks, &mut world, pumpkin_stem, BlockStateId(53)),
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
        ("minecraft:nether_wart", 44),
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
}

#[test]
fn sapling_bonemeal_grows_small_oak_tree_and_consumes_one_item() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let edits = bonemeal_growth_edits(registry.as_ref(), &mut world, pos, BlockStateId(1)).unwrap();
    assert_eq!(edits.len(), 17);
    assert_eq!(
        edits[0],
        BlockEdit {
            pos,
            new_state: BlockStateId(3)
        }
    );
    assert!(edits.iter().any(|edit| {
        edit.pos == mc_world::BlockPos { x: 4, y: 68, z: 4 } && edit.new_state == BlockStateId(4)
    }));

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &edits {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert!(!outcome.applied.is_empty());
    for dy in 0..=3 {
        assert_eq!(
            world
                .get_block(mc_world::BlockPos {
                    y: pos.y + dy,
                    ..pos
                })
                .unwrap(),
            Some(BlockStateId(3))
        );
    }
    assert_eq!(
        world
            .get_block(mc_world::BlockPos { x: 4, y: 68, z: 4 })
            .unwrap(),
        Some(BlockStateId(4))
    );

    let mut inventory = PlayerInventory::empty();
    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 99,
            count: 2,
            damage: None,
        },
    );
    let synced =
        consume_bonemeal_after_growth(&mut inventory, 0, !outcome.applied.is_empty()).unwrap();
    assert_eq!(synced.count, 1);
    assert_eq!(inventory.held(0).count, 1);
}

#[test]
fn common_sapling_bonemeal_uses_matching_log_and_leaves() {
    let registry = sapling_tree_test_registry();

    for (sapling_state, log_state, leaves_state) in [
        (7, 9, 10),
        (11, 13, 14),
        (15, 17, 18),
        (19, 21, 22),
        (23, 25, 26),
    ] {
        let mut world = in_memory_tree_world(Arc::clone(&registry));
        let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
        world
            .set_block_at(pos, BlockStateId(sapling_state))
            .unwrap();

        let edits = bonemeal_growth_edits(
            registry.as_ref(),
            &mut world,
            pos,
            BlockStateId(sapling_state),
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
            edits.iter().any(|edit| {
                edit.pos == mc_world::BlockPos { x: 4, y: 68, z: 4 }
                    && edit.new_state == BlockStateId(leaves_state)
            }),
            "sapling state {sapling_state} should use its matching leaves"
        );
    }
}

#[test]
fn sapling_bonemeal_blocked_space_does_not_edit_or_consume() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &mut world, pos, BlockStateId(1)),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(1)));

    let mut inventory = PlayerInventory::empty();
    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 99,
            count: 2,
            damage: None,
        },
    );
    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).count, 2);
}

#[test]
fn sapling_bonemeal_unsupported_and_missing_tree_states_are_noop() {
    let registry = sapling_tree_test_registry();
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    assert_eq!(
        bonemeal_growth_edits(registry.as_ref(), &mut world, pos, BlockStateId(6)),
        None
    );

    let reports = vec![
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:oak_sapling"),
        simple_block(2, "minecraft:oak_leaves"),
    ];
    let missing_registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let mut world = in_memory_tree_world(Arc::clone(&missing_registry));
    assert_eq!(
        bonemeal_growth_edits(missing_registry.as_ref(), &mut world, pos, BlockStateId(1)),
        None
    );
}

#[test]
fn bonemeal_consumes_exactly_one_item_only_after_successful_growth() {
    let mut inventory = PlayerInventory::empty();
    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 99,
            count: 3,
            damage: None,
        },
    );

    assert_eq!(
        consume_bonemeal_after_growth(&mut inventory, 0, false),
        None
    );
    assert_eq!(inventory.held(0).count, 3);

    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert_eq!(synced.count, 2);
    assert_eq!(inventory.held(0).count, 2);

    inventory.set_hotbar(
        0,
        ItemStack {
            item_id: 99,
            count: 1,
            damage: None,
        },
    );
    let synced = consume_bonemeal_after_growth(&mut inventory, 0, true).unwrap();
    assert!(synced.is_empty());
    assert!(inventory.held(0).is_empty());
}

#[test]
fn sapling_random_tick_grows_clear_oak_tree_without_item_consumption() {
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
    let edits = random_tick_edit(
        registry.as_ref(),
        &facts,
        &mut world,
        pos,
        BlockStateId(1),
        mc_data::block_facts::RandomTickFamily::Sapling,
    )
    .unwrap();
    assert_eq!(edits.len(), 17);
    assert_eq!(
        edits[0],
        BlockEdit {
            pos,
            new_state: BlockStateId(3)
        }
    );

    let mut outcome = BlockEditBatchOutcome::default();
    for edit in &edits {
        apply_block_edit_to_storage(&mut world, None, edit, &mut outcome);
    }
    assert!(!outcome.applied.is_empty());
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(3)));
    assert_eq!(
        world
            .get_block(mc_world::BlockPos { x: 4, y: 68, z: 4 })
            .unwrap(),
        Some(BlockStateId(4))
    );
}

#[test]
fn sapling_random_tick_obstructed_or_unsupported_targets_are_noop() {
    let reports = sapling_tree_test_reports();
    let registry = Arc::new(mc_world::BlockRegistry::from_report(&reports).unwrap());
    let facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&reports);
    let mut world = in_memory_tree_world(Arc::clone(&registry));
    let pos = mc_world::BlockPos { x: 4, y: 64, z: 4 };
    world.set_block_at(pos, BlockStateId(1)).unwrap();
    world
        .set_block_at(mc_world::BlockPos { x: 4, y: 68, z: 4 }, BlockStateId(5))
        .unwrap();

    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
            pos,
            BlockStateId(1),
            mc_data::block_facts::RandomTickFamily::Sapling,
        ),
        None
    );
    assert_eq!(world.get_block(pos).unwrap(), Some(BlockStateId(1)));
    assert_eq!(
        random_tick_edit(
            registry.as_ref(),
            &facts,
            &mut world,
            pos,
            BlockStateId(6),
            mc_data::block_facts::RandomTickFamily::Sapling,
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

#[test]
fn natural_random_tick_helpers_cover_leaves_grass_and_fire() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        simple_block(1, "minecraft:dirt"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_leaves").unwrap(),
            properties: prop_schema(&[
                ("distance", &["6", "7"]),
                ("persistent", &["false", "true"]),
            ]),
            states: vec![
                state(2, true, &[("distance", "7"), ("persistent", "false")]),
                state(3, false, &[("distance", "7"), ("persistent", "true")]),
                state(4, false, &[("distance", "6"), ("persistent", "false")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:fire").unwrap(),
            properties: prop_schema(&[("age", &["14", "15"])]),
            states: vec![
                state(5, true, &[("age", "14")]),
                state(6, false, &[("age", "15")]),
            ],
        },
        simple_block(7, "minecraft:grass_block"),
    ])
    .unwrap();

    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(2)),
        Some(mc_world::BlockStateId(0))
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(3)),
        None
    );
    assert_eq!(
        next_leaf_decay_state(&blocks, mc_world::BlockStateId(4)),
        None
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(5)),
        Some(mc_world::BlockStateId(6))
    );
    assert_eq!(
        next_fire_state(&blocks, mc_world::BlockStateId(6)),
        Some(mc_world::BlockStateId(0))
    );
}

#[test]
fn interactive_toggle_helpers_preserve_other_properties() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_trapdoor").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north"]),
                ("open", &["false", "true"]),
                ("waterlogged", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("open", "false"),
                        ("waterlogged", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("open", "true"),
                        ("waterlogged", "false"),
                    ],
                ),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:lever").unwrap(),
            properties: prop_schema(&[("facing", &["north"]), ("powered", &["false", "true"])]),
            states: vec![
                state(3, true, &[("facing", "north"), ("powered", "false")]),
                state(4, false, &[("facing", "north"), ("powered", "true")]),
            ],
        },
    ])
    .unwrap();

    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            "open"
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        toggled_bool_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            "powered"
        ),
        Some(mc_world::BlockStateId(4))
    );
}

#[test]
fn door_half_state_builds_two_block_placement_states() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_door").unwrap(),
            properties: prop_schema(&[
                ("facing", &["north", "south"]),
                ("half", &["lower", "upper"]),
                ("open", &["false"]),
                ("powered", &["false"]),
            ]),
            states: vec![
                state(
                    1,
                    true,
                    &[
                        ("facing", "north"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    2,
                    false,
                    &[
                        ("facing", "north"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    3,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "lower"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
                state(
                    4,
                    false,
                    &[
                        ("facing", "south"),
                        ("half", "upper"),
                        ("open", "false"),
                        ("powered", "false"),
                    ],
                ),
            ],
        },
    ])
    .unwrap();
    let default = blocks.by_id(mc_world::BlockStateId(1)).unwrap();

    assert_eq!(
        door_half_state(&blocks, default, "lower", "south"),
        Some(mc_world::BlockStateId(3))
    );
    assert_eq!(
        door_half_state(&blocks, default, "upper", "south"),
        Some(mc_world::BlockStateId(4))
    );
    assert_eq!(horizontal_facing_from_yaw(180.0), "north");
}

#[test]
fn sign_placement_sets_wall_facing_and_floor_rotation() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:oak_sign").unwrap(),
            properties: prop_schema(&[("rotation", &["0", "4"])]),
            states: vec![
                state(1, true, &[("rotation", "0")]),
                state(2, false, &[("rotation", "4")]),
            ],
        },
        BlockReport {
            id: Identifier::parse("minecraft:oak_wall_sign").unwrap(),
            properties: prop_schema(&[("facing", &["north", "east"])]),
            states: vec![
                state(3, true, &[("facing", "north")]),
                state(4, false, &[("facing", "east")]),
            ],
        },
    ])
    .unwrap();
    let mut pose = PlayerPose::new(0.5, 64.0, 0.5);
    pose.yaw = 90.0;

    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
            pose,
            Direction::Up,
        ),
        Some(mc_world::BlockStateId(2))
    );
    assert_eq!(
        sign_placement_state(
            &blocks,
            blocks.by_id(mc_world::BlockStateId(3)).unwrap(),
            pose,
            Direction::East,
        ),
        Some(mc_world::BlockStateId(4))
    );

    assert_eq!(
        placed_sign_edit_position(
            &blocks,
            &[AppliedBlockEdit {
                pos: mc_world::BlockPos { x: 1, y: 2, z: 3 },
                previous: mc_world::BlockStateId(0),
                new_state: mc_world::BlockStateId(2),
            }],
        ),
        Some(mc_world::BlockPos { x: 1, y: 2, z: 3 })
    );
}

#[test]
fn sign_update_nbt_matches_vanilla_plain_text_shape() {
    let tag = sign_block_entity_update_nbt(
        &[
            "Hello".to_string(),
            "World".to_string(),
            String::new(),
            "!".to_string(),
        ],
        true,
    );

    assert_eq!(
        tag,
        Tag::Compound(vec![
            (
                "front_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String("Hello".into()),
                                Tag::String("World".into()),
                                Tag::String(String::new()),
                                Tag::String("!".into()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            (
                "back_text".into(),
                Tag::Compound(vec![
                    (
                        "messages".into(),
                        Tag::List(ListTag {
                            element_type: mc_nbt::tag_type::STRING,
                            elements: vec![
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                                Tag::String(String::new()),
                            ],
                        }),
                    ),
                    ("color".into(), Tag::String("black".into())),
                    ("has_glowing_text".into(), Tag::Byte(0)),
                ]),
            ),
            ("is_waxed".into(), Tag::Byte(0)),
        ])
    );
}

#[test]
fn campfire_update_nbt_contains_visible_cooking_items_only() {
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: cooked_porkchop,
            protocol_id: 11,
        },
    ]);
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(10, 1), ItemStack::new(11, 1), 2));

    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: vec![Tag::Compound(vec![
                    ("Slot".into(), Tag::Int(0)),
                    ("id".into(), Tag::String(porkchop.as_str().to_string())),
                    ("count".into(), Tag::Int(1)),
                ])],
            }),
        )]))
    );

    assert!(!cooking.tick().changed);
    let tick = cooking.tick();
    assert!(tick.changed);
    assert_eq!(tick.completed, vec![ItemStack::new(11, 1)]);
    assert_eq!(
        campfire_block_entity_update_nbt(&items, &cooking),
        Some(Tag::Compound(vec![(
            "Items".into(),
            Tag::List(ListTag::empty()),
        )]))
    );
}

#[test]
fn bed_respawn_pose_uses_block_above_bed() {
    let blocks = mc_world::BlockRegistry::from_report(&[
        simple_block(0, "minecraft:air"),
        BlockReport {
            id: Identifier::parse("minecraft:red_bed").unwrap(),
            properties: prop_schema(&[("facing", &["north"])]),
            states: vec![state(1, true, &[("facing", "north")])],
        },
    ])
    .unwrap();
    let pose = bed_respawn_pose(
        mc_world::BlockPos { x: 3, y: 64, z: -2 },
        blocks.by_id(mc_world::BlockStateId(1)).unwrap(),
    );

    assert_eq!((pose.x, pose.y, pose.z, pose.yaw), (3.5, 65.0, -1.5, 180.0));
}

#[test]
fn single_player_sleep_skips_to_next_morning_at_night() {
    assert_eq!(plan_sleep_skip(12_542, 1), SleepPlan::SkipTo(24_000));
    assert_eq!(plan_sleep_skip(47_999, 1), SleepPlan::SkipTo(48_000));
}

#[test]
fn sleep_policy_keeps_daytime_and_multiplayer_bounded() {
    assert_eq!(plan_sleep_skip(1_000, 1), SleepPlan::Daytime);
    assert_eq!(plan_sleep_skip(12_542, 2), SleepPlan::MultiplayerDeferred);
}

#[test]
fn common_container_paper_cuts_resolve_to_existing_menus() {
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:furnace"),
        Some("Furnace")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:smoker"),
        Some("Smoker")
    );
    assert_eq!(
        super::containers::furnace_menu_title_for_block_id("minecraft:blast_furnace"),
        Some("Blast Furnace")
    );
    assert_eq!(FurnaceKind::Furnace.menu_type(), FURNACE_MENU_TYPE_ID);
    assert_eq!(FurnaceKind::Smoker.menu_type(), SMOKER_MENU_TYPE_ID);
    assert_eq!(
        FurnaceKind::BlastFurnace.menu_type(),
        BLAST_FURNACE_MENU_TYPE_ID
    );
}

fn interaction_state_for_items(items: Arc<ItemRegistry>) -> InteractionState {
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
    let world = Arc::new(tokio::sync::Mutex::new(mc_world::WorldStorage::in_memory(
        Arc::clone(&blocks),
    )));
    let item_to_block = ItemToBlockTable::build(&items, &blocks);
    InteractionState {
        world,
        blocks,
        block_light: None,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        water: None,
        sessions: Arc::new(SessionRegistry::new()),
        session_id: 1,
        workspace: LightWorkspace::new(),
        light_cache: LightCache::new(),
        compression: Compression::Disabled,
        selected_hotbar_slot: 0,
        inventory: PlayerInventory::empty(),
        carried_item: ItemStack::EMPTY,
        inventory_state_id: 1,
        items,
        item_facts: Arc::new(ItemFactsTable::default()),
        entity_types: Arc::new(EntityTypeRegistry::default()),
        item_to_block,
        tags: Arc::new(TagsData::default()),
        recipes: Vec::new(),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        next_container_id: FURNACE_CONTAINER_ID_MIN,
        active_container: None,
        pending_break: None,
        pending_use: None,
        pending_sign_edit: None,
        shield_use: None,
        last_hostile_damage_at: None,
        last_entity_attack_at: None,
        fluid_schedule_tick: 0,
    }
}

#[test]
fn furnace_window_swap_and_throw_mutate_menu_slots() {
    let coal = Identifier::parse("minecraft:coal").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[ItemReport {
        id: coal,
        protocol_id: 10,
    }]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let coal_id = items
        .id_of(&Identifier::parse("minecraft:coal").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(coal_id, 4);
    let mut furnace = FurnaceBlockEntity::default();

    assert!(apply_furnace_swap_click(
        &mut state,
        &mut furnace,
        FurnaceKind::Furnace,
        1,
        0,
    ));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 4)
    );
    assert!(state.inventory.slots[PlayerInventory::HOTBAR_BASE].is_empty());

    let dropped = apply_furnace_throw_click(&mut state, &mut furnace, 1, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(coal_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&furnace.slots[1]),
        ItemStack::new(coal_id, 3)
    );
}

#[test]
fn chest_window_swap_and_throw_mutate_storage_slots() {
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let items = Arc::new(ItemRegistry::from_report(&[
        ItemReport {
            id: dirt,
            protocol_id: 10,
        },
        ItemReport {
            id: stone,
            protocol_id: 11,
        },
    ]));
    let mut state = interaction_state_for_items(Arc::clone(&items));
    let dirt_id = items
        .id_of(&Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let stone_id = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(stone_id, 2);
    let mut view = ChestView {
        chests: vec![ChestBlockEntity::default()],
    };
    view.chests[0].slots[0] = stack_to_furnace_slot(&ItemStack::new(dirt_id, 5));

    assert!(apply_chest_swap_click(&mut state, &mut view, 0, 0));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 2)
    );
    assert_eq!(
        state.inventory.slots[PlayerInventory::HOTBAR_BASE],
        ItemStack::new(dirt_id, 5)
    );

    let dropped = apply_chest_throw_click(&mut state, &mut view, 0, 0).unwrap();
    assert_eq!(dropped, ItemStack::new(stone_id, 1));
    assert_eq!(
        furnace_slot_to_stack(&view.chests[0].slots[0]),
        ItemStack::new(stone_id, 1)
    );
}

#[test]
fn furnace_like_recipe_lookup_uses_matching_cooking_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let iron_ore = Identifier::parse("minecraft:iron_ore").unwrap();
    let raw_iron = Identifier::parse("minecraft:raw_iron").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let iron_ingot = Identifier::parse("minecraft:iron_ingot").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: iron_ore.clone(),
            protocol_id: 10,
        },
        ItemReport {
            id: raw_iron.clone(),
            protocol_id: 11,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 12,
        },
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: iron_ingot.clone(),
            protocol_id: 20,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 21,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
    };
    let result = |item: Identifier| RecipeResult { item, count: 1 };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smelting").unwrap(),
            kind: RecipeKind::Smelting(cooking(iron_ore, 200)),
            result: result(iron_ingot.clone()),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_blasting").unwrap(),
            kind: RecipeKind::Blasting(cooking(raw_iron.clone(), 100)),
            result: result(iron_ingot),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop.clone(), 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 10)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smelting").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 11)
            .is_none()
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(
            &recipes,
            &items,
            &tags,
            FurnaceKind::BlastFurnace,
            11
        )
        .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_blasting").unwrap())
    );
    assert_eq!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Smoker, 12)
            .map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_smoking").unwrap())
    );
    assert!(
        containers::find_cooking_recipe_for_item(&recipes, &items, &tags, FurnaceKind::Furnace, 13)
            .is_none()
    );
}

#[test]
fn campfire_recipe_lookup_uses_campfire_category() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, SmeltingRecipe,
    };

    let porkchop = Identifier::parse("minecraft:porkchop").unwrap();
    let beef = Identifier::parse("minecraft:beef").unwrap();
    let cooked_porkchop = Identifier::parse("minecraft:cooked_porkchop").unwrap();
    let cooked_beef = Identifier::parse("minecraft:cooked_beef").unwrap();
    let items = ItemRegistry::from_report(&[
        ItemReport {
            id: porkchop.clone(),
            protocol_id: 13,
        },
        ItemReport {
            id: beef.clone(),
            protocol_id: 14,
        },
        ItemReport {
            id: cooked_porkchop.clone(),
            protocol_id: 22,
        },
        ItemReport {
            id: cooked_beef.clone(),
            protocol_id: 23,
        },
    ]);
    let ingredient = |item: Identifier| Ingredient {
        alternatives: vec![IngredientAlternative::Item(item)],
    };
    let cooking = |item: Identifier, cooking_time| SmeltingRecipe {
        ingredient: ingredient(item),
        cooking_time,
    };
    let result = |item: Identifier| RecipeResult { item, count: 1 };
    let recipes = vec![
        Recipe {
            id: Identifier::parse("minecraft:test_smoking").unwrap(),
            kind: RecipeKind::Smoking(cooking(beef.clone(), 100)),
            result: result(cooked_beef),
        },
        Recipe {
            id: Identifier::parse("minecraft:test_campfire").unwrap(),
            kind: RecipeKind::CampfireCooking(cooking(porkchop, 600)),
            result: result(cooked_porkchop),
        },
    ];
    let tags = TagsData::default();

    assert_eq!(
        containers::find_campfire_recipe_in(&recipes, &items, &tags, 13).map(|recipe| recipe.id),
        Some(Identifier::parse("minecraft:test_campfire").unwrap())
    );
    assert!(containers::find_campfire_recipe_in(&recipes, &items, &tags, 14).is_none());
}

#[test]
fn campfire_cooking_rejects_invalid_when_full() {
    let mut cooking = CampfireCookingState::default();

    for item_id in 1..=CAMPFIRE_COOKING_SLOT_COUNT as u32 {
        assert!(cooking.insert(ItemStack::new(item_id, 1), ItemStack::new(item_id, 1), 5));
    }
    assert!(!cooking.insert(ItemStack::new(99, 1), ItemStack::new(99, 1), 5));
}

#[test]
fn campfire_cooking_outputs_after_cooking_time() {
    let mut cooking = CampfireCookingState::default();
    assert!(cooking.insert(ItemStack::new(41, 1), ItemStack::new(42, 1), 2));

    assert!(cooking.tick().completed.is_empty());
    assert_eq!(cooking.tick().completed, vec![ItemStack::new(42, 1)]);
    assert!(cooking.is_empty());
}

#[test]
fn hostile_melee_requires_moving_toward_player() {
    let hostile = |velocity: Vec3| ServerEntitySnapshot {
        id: mc_entity::EntityId(7),
        uuid: uuid::Uuid::nil(),
        type_id: 1,
        type_name: "minecraft:zombie".into(),
        position: Vec3::ZERO,
        rotation: mc_entity::Rotation::ZERO,
        velocity,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        attack_damage: Some(3.0),
    };
    let player = Vec3::new(1.0, 0.0, 0.0);

    assert!(hostile_can_melee_player(
        &hostile(Vec3::new(0.2, 0.0, 0.0)),
        player
    ));
    assert!(!hostile_can_melee_player(
        &hostile(Vec3::new(-0.2, 0.0, 0.0)),
        player
    ));
    assert!(!hostile_can_melee_player(&hostile(Vec3::ZERO), player));
}

#[test]
fn hostile_melee_reaches_player_one_block_above() {
    let hostile = ServerEntitySnapshot {
        id: mc_entity::EntityId(7),
        uuid: uuid::Uuid::nil(),
        type_id: 1,
        type_name: "minecraft:zombie".into(),
        position: Vec3::new(0.0, 64.0, 0.0),
        rotation: mc_entity::Rotation::ZERO,
        velocity: Vec3::new(0.2, 0.0, 0.0),
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        attack_damage: Some(3.0),
    };

    assert!(hostile_can_melee_player(
        &hostile,
        Vec3::new(1.0, 65.0, 0.0)
    ));
    assert!(!hostile_can_melee_player(
        &hostile,
        Vec3::new(1.0, 67.0, 0.0)
    ));
}

#[test]
fn shield_use_starts_blocking_state_for_shield_stack() {
    let stack = ItemStack::new(77, 1);

    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        stack.clone(),
        12,
        true,
    )
    .expect("shield stack should start shield use");

    assert_eq!(shield_use.started_tick, 12);
    assert_eq!(shield_use.slot, PlayerInventory::HOTBAR_BASE);
    assert_eq!(shield_use.stack, stack);
}

#[test]
fn shield_use_metadata_uses_vanilla_living_entity_flags() {
    let main_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };
    let off_hand = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert_eq!(shield_use_flags(None), 0);
    assert_eq!(
        shield_use_flags(Some(&main_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM
    );
    assert_eq!(
        shield_use_flags(Some(&off_hand)),
        LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND
    );
    assert_eq!(
        shield_use_entity_data_value(Some(&off_hand)),
        EntityDataValue::Byte {
            index: LIVING_ENTITY_DATA_FLAGS_INDEX,
            value: LIVING_ENTITY_FLAG_USING_ITEM | LIVING_ENTITY_FLAG_OFF_HAND,
        }
    );
}

#[test]
fn shield_non_shield_use_does_not_block() {
    let shield_use = shield_use_from_stack(
        mc_protocol::packets::play::InteractionHand::MainHand,
        PlayerInventory::HOTBAR_BASE,
        ItemStack::new(77, 1),
        12,
        false,
    );

    assert!(shield_use.is_none());
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        20,
        SHIELD_ACTIVATION_DELAY_TICKS,
        shield_use.as_ref(),
    ));
}

#[test]
fn shield_activation_delay_gates_damage() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 10,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        14,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 1.0)),
        15,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
}

#[test]
fn shield_blocks_frontal_mob_and_arrow_sources() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::OffHand,
        started_tick: 1,
        slot: 45,
        stack: ItemStack::new(77, 1),
    };

    assert!(shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, 2.0)),
        10,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
    assert!(shield_blocks_damage(
        Vec3::ZERO,
        90.0,
        Some(Vec3::new(-2.0, 0.0, 0.0)),
        10,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
}

#[test]
fn shield_side_back_and_unknown_sources_are_not_blocked() {
    let shield_use = ShieldUseState {
        hand: mc_protocol::packets::play::InteractionHand::MainHand,
        started_tick: 1,
        slot: PlayerInventory::HOTBAR_BASE,
        stack: ItemStack::new(77, 1),
    };

    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(2.0, 0.0, 0.0)),
        10,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        Some(Vec3::new(0.0, 0.0, -2.0)),
        10,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
    assert!(!shield_blocks_damage(
        Vec3::ZERO,
        0.0,
        None,
        10,
        SHIELD_ACTIVATION_DELAY_TICKS,
        Some(&shield_use),
    ));
}

include!("tests/inventory_and_survival.rs");
include!("tests/spawning_and_world.rs");
