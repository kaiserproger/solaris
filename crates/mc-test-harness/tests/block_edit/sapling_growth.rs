#[tokio::test]
async fn survival_bonemeal_grows_oak_sapling_into_tree() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let oak_sapling = sapling_test_state(&blocks, "minecraft:oak_sapling", &[]);
    let grown_oak_sapling = sapling_test_state(&blocks, "minecraft:oak_sapling", &[("stage", "1")]);
    let dirt = sapling_test_state(&blocks, "minecraft:dirt", &[]);
    let oak_log = sapling_test_state(&blocks, "minecraft:oak_log", &[("axis", "y")]);
    let oak_leaves = sapling_test_state(
        &blocks,
        "minecraft:oak_leaves",
        &[
            ("distance", "1"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    );
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M60 sapling growth".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M60SaplingGrowth").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let sapling_pos = (2, support_y + 1, 2);
    {
        let mut storage = world.lock().await;
        clear_loaded_mega_sapling_volume(&mut storage, &blocks, sapling_pos);
        sapling_test_set(
            &mut storage,
            (sapling_pos.0, support_y, sapling_pos.2),
            dirt,
        );
        sapling_test_set(&mut storage, sapling_pos, oak_sapling);
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bone_meal 3 0".into(),
        })
        .await
        .expect("give bone meal");
    let bone_meal_slot = wait_for_slot_stack_update(&mut client, bone_meal_item_id, 3)
        .await
        .slot;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 601,
        })
        .await
        .expect("bonemeal oak sapling");

    wait_for_sapling_stage_growth(
        &mut client,
        sapling_pos,
        grown_oak_sapling.0 as i32,
        bone_meal_slot,
        bone_meal_item_id,
        2,
        601,
    )
    .await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 602,
        })
        .await
        .expect("bonemeal grown oak sapling");

    wait_for_sapling_tree_growth(
        &mut client,
        (sapling_pos, oak_log.0 as i32),
        (
            (sapling_pos.0 + 1, sapling_pos.1 + 4, sapling_pos.2),
            oak_leaves.0 as i32,
        ),
        bone_meal_slot,
        bone_meal_item_id,
        1,
        602,
    )
    .await;
}

#[tokio::test]
async fn survival_bonemeal_stage_one_oak_replaces_existing_canopy_leaf() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let oak_sapling = sapling_test_state(&blocks, "minecraft:oak_sapling", &[]);
    let stage_one_oak_sapling =
        sapling_test_state(&blocks, "minecraft:oak_sapling", &[("stage", "1")]);
    let dirt = sapling_test_state(&blocks, "minecraft:dirt", &[]);
    let oak_log = sapling_test_state(&blocks, "minecraft:oak_log", &[("axis", "y")]);
    let oak_leaves = sapling_test_state(
        &blocks,
        "minecraft:oak_leaves",
        &[
            ("distance", "1"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    );
    let existing_oak_leaves = sapling_test_state(
        &blocks,
        "minecraft:oak_leaves",
        &[
            ("distance", "7"),
            ("persistent", "true"),
            ("waterlogged", "false"),
        ],
    );
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "sapling canopy replacement".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "SaplingCanopy").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let sapling_pos = (2, support_y + 1, 2);
    let canopy_pos = (sapling_pos.0 + 1, sapling_pos.1 + 4, sapling_pos.2);
    {
        let mut storage = world.lock().await;
        clear_loaded_mega_sapling_volume(&mut storage, &blocks, sapling_pos);
        sapling_test_set(
            &mut storage,
            (sapling_pos.0, support_y, sapling_pos.2),
            dirt,
        );
        sapling_test_set(&mut storage, sapling_pos, oak_sapling);
        sapling_test_set(&mut storage, canopy_pos, existing_oak_leaves);
    }

    let bone_meal_slot = give_and_select_bone_meal(&mut client, bone_meal_item_id).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 603,
        })
        .await
        .expect("bonemeal oak sapling to stage one");
    wait_for_sapling_stage_growth(
        &mut client,
        sapling_pos,
        stage_one_oak_sapling.0 as i32,
        bone_meal_slot,
        bone_meal_item_id,
        0,
        603,
    )
    .await;

    let bone_meal_slot = give_and_select_bone_meal(&mut client, bone_meal_item_id).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 604,
        })
        .await
        .expect("bonemeal stage-one oak sapling");

    wait_for_sapling_tree_growth(
        &mut client,
        (sapling_pos, oak_log.0 as i32),
        (canopy_pos, oak_leaves.0 as i32),
        bone_meal_slot,
        bone_meal_item_id,
        0,
        604,
    )
    .await;
}

#[tokio::test]
async fn survival_bonemeal_does_not_consume_on_single_dark_oak() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let dark_oak = sapling_test_state(&blocks, "minecraft:dark_oak_sapling", &[("stage", "1")]);
    let dirt = sapling_test_state(&blocks, "minecraft:dirt", &[]);
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "single dark oak gate".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "DarkOakGate").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let support_y = sync.y.floor() as i32 - 2;
    let sapling_pos = (2, support_y + 1, 2);
    {
        let mut storage = world.lock().await;
        sapling_test_set(&mut storage, (2, support_y, 2), dirt);
        sapling_test_set(&mut storage, sapling_pos, dark_oak);
    }

    let _bone_meal_slot = give_and_select_bone_meal(&mut client, bone_meal_item_id).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 605,
        })
        .await
        .expect("try bonemeal on single dark oak");
    wait_for_block_ack(&mut client, 605).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:bone_meal 1".into(),
        })
        .await
        .expect("add one bone meal after rejected growth");
    wait_for_slot_stack_update(&mut client, bone_meal_item_id, 2).await;

    let mut storage = world.lock().await;
    assert_eq!(
        storage
            .get_block(mc_world::BlockPos {
                x: sapling_pos.0,
                y: sapling_pos.1,
                z: sapling_pos.2,
            })
            .unwrap(),
        Some(dark_oak)
    );
}

#[derive(Clone, Copy)]
struct MegaSaplingWireCase {
    label: &'static str,
    sapling_name: &'static str,
    log_name: &'static str,
    leaves_name: &'static str,
    base_height: i32,
    first_random: u64,
    second_random: u64,
    canopy_y_from_top: i32,
}

const MEGA_SAPLING_WIRE_CASES: [MegaSaplingWireCase; 2] = [
    MegaSaplingWireCase {
        label: "spruce",
        sapling_name: "minecraft:spruce_sapling",
        log_name: "minecraft:spruce_log",
        leaves_name: "minecraft:spruce_leaves",
        base_height: 13,
        first_random: 2,
        second_random: 14,
        canopy_y_from_top: -1,
    },
    MegaSaplingWireCase {
        label: "jungle",
        sapling_name: "minecraft:jungle_sapling",
        log_name: "minecraft:jungle_log",
        leaves_name: "minecraft:jungle_leaves",
        base_height: 10,
        first_random: 2,
        second_random: 19,
        canopy_y_from_top: 1,
    },
];

struct MegaSaplingWireFixture {
    client: Client,
    world: Arc<tokio::sync::Mutex<mc_world::WorldStorage>>,
    blocks: Arc<mc_world::BlockRegistry>,
    bone_meal_item_id: u32,
    spawn_y: f64,
}

#[tokio::test]
async fn survival_bonemeal_emits_client_visible_atomic_batch_for_two_by_two_spruce_and_jungle() {
    for (index, case) in MEGA_SAPLING_WIRE_CASES.into_iter().enumerate() {
        let Some(mut fixture) = start_mega_sapling_wire_fixture(
            VIEW_DISTANCE,
            ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
            match case.label {
                "spruce" => "MegaSpruceGrowth",
                "jungle" => "MegaJungleGrowth",
                _ => unreachable!(),
            },
        )
        .await
        else {
            return;
        };
        let states = mega_sapling_states(&fixture.blocks, case);
        let northwest = (4, fixture.spawn_y.floor() as i32 - 1, 4);
        let saplings = mega_sapling_square(northwest);
        let clicked = saplings[3];
        let sequence = 610 + i32::try_from(index).expect("two mega sapling cases");
        let expected = {
            let mut storage = fixture.world.lock().await;
            clear_loaded_mega_sapling_volume(&mut storage, &fixture.blocks, northwest);
            set_mega_sapling_square(&mut storage, saplings, states.sapling, states.dirt);
            mega_sapling_expected_deltas(
                &storage,
                case,
                northwest,
                clicked,
                states.sapling,
                sequence,
            )
        };
        move_mega_sapling_client(&mut fixture.client, fixture.spawn_y, 4.5, 4.5).await;

        let bone_meal_slot =
            give_and_select_bone_meal(&mut fixture.client, fixture.bone_meal_item_id).await;
        use_bone_meal_on(&mut fixture.client, clicked, sequence).await;

        wait_for_mega_sapling_growth(
            &mut fixture.client,
            saplings,
            MegaSaplingGrowthWireExpected {
                deltas: expected,
                log_state_id: states.log.0 as i32,
                leaves_state_id: states.leaves.0 as i32,
                bone_meal_slot,
                sequence,
            },
            case.label,
        )
        .await;
        prove_consumed_bone_meal_authoritatively(
            &mut fixture.client,
            fixture.bone_meal_item_id,
            case.label,
        )
        .await;

        let storage = fixture.world.lock().await;
        for sapling in saplings {
            let base = mega_sapling_block_pos(sapling);
            assert_eq!(
                storage.get_cached_block(base),
                Some(states.log),
                "{} must replace all four saplings with trunk blocks",
                case.label
            );
        }
        assert_eq!(
            storage.get_cached_block(mega_sapling_block_pos(expected.high_trunk)),
            Some(states.log),
            "{} high trunk delta must match committed world state",
            case.label
        );
        let crown_state = storage
            .get_cached_block(mega_sapling_block_pos(expected.canopy))
            .expect("wire crown delta must remain loaded");
        assert_eq!(
            fixture
                .blocks
                .by_id(crown_state)
                .expect("committed crown state must be registered")
                .block
                .id
                .as_str(),
            case.leaves_name,
            "{} crown delta must remain the matching leaf block",
            case.label
        );
    }
}

#[tokio::test]
async fn survival_bonemeal_rejects_obstructed_two_by_two_spruce_and_jungle_without_client_visible_partial_batch()
 {
    for (index, case) in MEGA_SAPLING_WIRE_CASES.into_iter().enumerate() {
        let Some(mut fixture) = start_mega_sapling_wire_fixture(
            VIEW_DISTANCE,
            ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
            match case.label {
                "spruce" => "BlockMegaSpruce",
                "jungle" => "BlockMegaJungle",
                _ => unreachable!(),
            },
        )
        .await
        else {
            return;
        };
        let states = mega_sapling_states(&fixture.blocks, case);
        let northwest = (2, fixture.spawn_y.floor() as i32 - 1, 2);
        let saplings = mega_sapling_square(northwest);
        let obstruction = (northwest.0, northwest.1 + 1, northwest.2);
        {
            let mut storage = fixture.world.lock().await;
            clear_loaded_mega_sapling_volume(&mut storage, &fixture.blocks, northwest);
            set_mega_sapling_square(&mut storage, saplings, states.sapling, states.dirt);
            sapling_test_set(&mut storage, obstruction, states.stone);
        }

        let bone_meal_slot =
            give_and_select_bone_meal(&mut fixture.client, fixture.bone_meal_item_id).await;
        let sequence = 620 + i32::try_from(index).expect("two mega sapling cases");
        use_bone_meal_on(&mut fixture.client, saplings[3], sequence).await;
        wait_for_rejected_mega_sapling_growth(
            &mut fixture.client,
            fixture.bone_meal_item_id,
            bone_meal_slot,
            sequence,
            case.label,
        )
        .await;

        let storage = fixture.world.lock().await;
        assert_mega_saplings_unchanged(&storage, saplings, states.sapling, case.label);
        assert_eq!(
            storage.get_cached_block(mega_sapling_block_pos(obstruction)),
            Some(states.stone),
            "{} obstruction must survive rejected growth",
            case.label
        );
    }
}

#[tokio::test]
async fn survival_bonemeal_rejects_unloaded_two_by_two_spruce_and_jungle_canopy_without_client_visible_partial_batch()
 {
    for (index, case) in MEGA_SAPLING_WIRE_CASES.into_iter().enumerate() {
        let Some(mut fixture) = start_mega_sapling_wire_fixture(
            0,
            1,
            match case.label {
                "spruce" => "MissMegaSpruce",
                "jungle" => "MissMegaJungle",
                _ => unreachable!(),
            },
        )
        .await
        else {
            return;
        };
        let states = mega_sapling_states(&fixture.blocks, case);
        let northwest = (14, fixture.spawn_y.floor() as i32 - 1, 2);
        let saplings = mega_sapling_square(northwest);
        {
            let mut storage = fixture.world.lock().await;
            set_mega_sapling_square(&mut storage, saplings, states.sapling, states.dirt);
            assert!(
                storage
                    .cached_chunk(mc_world::ChunkPos { x: 1, z: 0 })
                    .is_none(),
                "capacity-one fixture must leave the eastern canopy chunk unloaded"
            );
        }
        move_mega_sapling_client(&mut fixture.client, fixture.spawn_y, 12.5, 2.5).await;

        let bone_meal_slot =
            give_and_select_bone_meal(&mut fixture.client, fixture.bone_meal_item_id).await;
        let sequence = 630 + i32::try_from(index).expect("two mega sapling cases");
        use_bone_meal_on(&mut fixture.client, saplings[3], sequence).await;
        wait_for_rejected_mega_sapling_growth(
            &mut fixture.client,
            fixture.bone_meal_item_id,
            bone_meal_slot,
            sequence,
            case.label,
        )
        .await;

        let storage = fixture.world.lock().await;
        assert_mega_saplings_unchanged(&storage, saplings, states.sapling, case.label);
        assert!(
            storage
                .cached_chunk(mc_world::ChunkPos { x: 1, z: 0 })
                .is_none(),
            "{} rejected growth must not load the missing canopy chunk",
            case.label
        );
    }
}

struct MegaSaplingStates {
    sapling: mc_world::BlockStateId,
    dirt: mc_world::BlockStateId,
    stone: mc_world::BlockStateId,
    log: mc_world::BlockStateId,
    leaves: mc_world::BlockStateId,
}

#[derive(Clone, Copy)]
struct MegaSaplingExpectedDeltas {
    high_trunk: (i32, i32, i32),
    canopy: (i32, i32, i32),
}

struct MegaSaplingGrowthWireExpected {
    deltas: MegaSaplingExpectedDeltas,
    log_state_id: i32,
    leaves_state_id: i32,
    bone_meal_slot: i16,
    sequence: i32,
}

fn mega_sapling_states(
    blocks: &mc_world::BlockRegistry,
    case: MegaSaplingWireCase,
) -> MegaSaplingStates {
    MegaSaplingStates {
        sapling: sapling_test_state(blocks, case.sapling_name, &[("stage", "1")]),
        dirt: sapling_test_state(blocks, "minecraft:dirt", &[]),
        stone: sapling_test_state(blocks, "minecraft:stone", &[]),
        log: sapling_test_state(blocks, case.log_name, &[("axis", "y")]),
        leaves: sapling_test_state(
            blocks,
            case.leaves_name,
            &[
                ("distance", "1"),
                ("persistent", "false"),
                ("waterlogged", "false"),
            ],
        ),
    }
}

fn mega_sapling_expected_deltas(
    storage: &mc_world::WorldStorage,
    case: MegaSaplingWireCase,
    northwest: (i32, i32, i32),
    clicked: (i32, i32, i32),
    sapling: mc_world::BlockStateId,
    sequence: i32,
) -> MegaSaplingExpectedDeltas {
    let clicked_pos = mega_sapling_block_pos(clicked);
    let token = storage
        .block_mutation_token(clicked_pos)
        .expect("clicked mega sapling must have a mutation token");
    let tree_seed = mega_sapling_tree_seed(clicked_pos, sapling, token, sequence);
    let tree_height = case.base_height
        + (tree_seed % (case.first_random + 1)) as i32
        + (sapling_splitmix64(tree_seed ^ 0x4d45_4741_5f48_4549) % (case.second_random + 1)) as i32;
    MegaSaplingExpectedDeltas {
        high_trunk: (
            northwest.0 + 1,
            northwest.1 + tree_height - 1,
            northwest.2 + 1,
        ),
        canopy: (
            northwest.0,
            northwest.1 + tree_height + case.canopy_y_from_top,
            northwest.2 - 1,
        ),
    }
}

fn mega_sapling_tree_seed(
    position: mc_world::BlockPos,
    state: mc_world::BlockStateId,
    token: mc_world::BlockMutationToken,
    sequence: i32,
) -> u64 {
    let mut seed = token.chunk_instance_id ^ token.version.rotate_left(29);
    seed = sapling_splitmix64(seed ^ u64::from(state.0));
    seed = sapling_splitmix64(seed ^ (position.x as i64 as u64).rotate_left(11));
    seed = sapling_splitmix64(seed ^ (position.y as i64 as u64).rotate_left(31));
    seed = sapling_splitmix64(seed ^ (position.z as i64 as u64).rotate_left(47));
    sapling_splitmix64(seed ^ (sequence as i64 as u64).rotate_left(23))
}

fn sapling_splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

async fn start_mega_sapling_wire_fixture(
    view_distance: i32,
    capacity: usize,
    username: &str,
) -> Option<MegaSaplingWireFixture> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return None;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), capacity)
        .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "mega sapling raw-wire proof".into(),
        max_players: 8,
        view_distance,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, username).await;
    drain_until_chunk(&mut client, (0, 0)).await;
    Some(MegaSaplingWireFixture {
        client,
        world,
        blocks,
        bone_meal_item_id,
        spawn_y: sync.y,
    })
}

fn mega_sapling_square(northwest: (i32, i32, i32)) -> [(i32, i32, i32); 4] {
    [
        northwest,
        (northwest.0 + 1, northwest.1, northwest.2),
        (northwest.0, northwest.1, northwest.2 + 1),
        (northwest.0 + 1, northwest.1, northwest.2 + 1),
    ]
}

fn set_mega_sapling_square(
    storage: &mut mc_world::WorldStorage,
    saplings: [(i32, i32, i32); 4],
    sapling: mc_world::BlockStateId,
    dirt: mc_world::BlockStateId,
) {
    for pos in saplings {
        sapling_test_set(storage, (pos.0, pos.1 - 1, pos.2), dirt);
        sapling_test_set(storage, pos, sapling);
    }
}

fn clear_loaded_mega_sapling_volume(
    storage: &mut mc_world::WorldStorage,
    blocks: &mc_world::BlockRegistry,
    northwest: (i32, i32, i32),
) {
    let air = sapling_test_state(blocks, "minecraft:air", &[]);
    for x in (northwest.0 - 3)..=(northwest.0 + 4) {
        for z in (northwest.2 - 3)..=(northwest.2 + 4) {
            for y in northwest.1..=(northwest.1 + 32) {
                let pos = mc_world::BlockPos { x, y, z };
                if storage.get_cached_block(pos).is_some() {
                    storage
                        .set_block_at(pos, air)
                        .expect("loaded mega-sapling fixture volume clears");
                }
            }
        }
    }
}

fn assert_mega_saplings_unchanged(
    storage: &mc_world::WorldStorage,
    saplings: [(i32, i32, i32); 4],
    sapling: mc_world::BlockStateId,
    label: &str,
) {
    for pos in saplings {
        assert_eq!(
            storage.get_cached_block(mega_sapling_block_pos(pos)),
            Some(sapling),
            "{label} rejection must preserve all four saplings"
        );
    }
}

fn mega_sapling_block_pos(pos: (i32, i32, i32)) -> mc_world::BlockPos {
    mc_world::BlockPos {
        x: pos.0,
        y: pos.1,
        z: pos.2,
    }
}

async fn use_bone_meal_on(client: &mut Client, pos: (i32, i32, i32), sequence: i32) {
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(pos.0, pos.1, pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("bonemeal mega sapling");
}

async fn wait_for_mega_sapling_growth(
    client: &mut Client,
    saplings: [(i32, i32, i32); 4],
    expected: MegaSaplingGrowthWireExpected,
    label: &str,
) {
    let MegaSaplingGrowthWireExpected {
        deltas,
        log_state_id,
        leaves_state_id,
        bone_meal_slot,
        sequence,
    } = expected;
    let expected_bases = saplings.into_iter().collect::<HashSet<_>>();
    let mut seen_bases = HashSet::new();
    let mut saw_high_trunk = false;
    let mut saw_canopy = false;
    let mut saw_atomic_base_packet = false;
    let mut saw_bonemeal_decrement = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while seen_bases.len() != 4 || !saw_atomic_base_packet || !saw_bonemeal_decrement || !saw_ack {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "{label} mega-tree response timed out: bases={} atomic={} bonemeal={} ack={}: {err}",
                    seen_bases.len(),
                    saw_atomic_base_packet,
                    saw_bonemeal_decrement,
                    saw_ack
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode mega sapling BlockUpdate");
            record_mega_sapling_delta(
                unpack_block_pos(packet.position),
                packet.state_id,
                &expected_bases,
                &mut seen_bases,
                &mut saw_high_trunk,
                &mut saw_canopy,
                deltas,
                log_state_id,
                leaves_state_id,
            );
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet = SectionBlocksUpdate::decode(&mut body)
                .expect("decode mega sapling SectionBlocksUpdate");
            let packet_base_logs = packet
                .changes
                .iter()
                .filter(|change| {
                    change.state_id == log_state_id
                        && expected_bases.contains(&sapling_section_change_pos(
                            packet.section_pos,
                            change.relative_pos,
                        ))
                })
                .count();
            saw_atomic_base_packet |= packet_base_logs == expected_bases.len();
            for change in packet.changes {
                record_mega_sapling_delta(
                    sapling_section_change_pos(packet.section_pos, change.relative_pos),
                    change.state_id,
                    &expected_bases,
                    &mut seen_bases,
                    &mut saw_high_trunk,
                    &mut saw_canopy,
                    deltas,
                    log_state_id,
                    leaves_state_id,
                );
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode mega sapling bone meal SetSlot");
            saw_bonemeal_decrement |= packet.container_id == 0
                && packet.slot == bone_meal_slot
                && packet.item_stack.is_empty();
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode mega sapling ack");
            saw_ack |= packet.sequence == sequence;
        }
    }
    assert!(
        saw_high_trunk,
        "{label} batch must contain exact high trunk delta {:?}",
        deltas.high_trunk
    );
    assert!(
        saw_canopy,
        "{label} batch must contain exact canopy delta {:?}",
        deltas.canopy
    );
}

#[allow(clippy::too_many_arguments)]
fn record_mega_sapling_delta(
    pos: (i32, i32, i32),
    state_id: i32,
    expected_bases: &HashSet<(i32, i32, i32)>,
    seen_bases: &mut HashSet<(i32, i32, i32)>,
    saw_high_trunk: &mut bool,
    saw_canopy: &mut bool,
    expected: MegaSaplingExpectedDeltas,
    log_state_id: i32,
    leaves_state_id: i32,
) {
    if state_id == log_state_id && expected_bases.contains(&pos) {
        seen_bases.insert(pos);
    }
    *saw_high_trunk |= state_id == log_state_id && pos == expected.high_trunk;
    *saw_canopy |= state_id == leaves_state_id && pos == expected.canopy;
}

async fn wait_for_rejected_mega_sapling_growth(
    client: &mut Client,
    bone_meal_item_id: u32,
    bone_meal_slot: i16,
    sequence: i32,
    label: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| panic!("{label} rejected growth ack timed out: {err}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        assert_ne!(
            frame.id,
            BlockUpdate::ID,
            "{label} rejected growth must not emit a partial BlockUpdate"
        );
        assert_ne!(
            frame.id,
            SectionBlocksUpdate::ID,
            "{label} rejected growth must not emit a partial SectionBlocksUpdate"
        );
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode rejected mega sapling SetSlot");
            panic!(
                "{label} rejected growth emitted inventory update before ack: container={} slot={} item={} count={}",
                packet.container_id,
                packet.slot,
                packet.item_stack.item_id,
                packet.item_stack.count
            );
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode rejected growth ack");
            if packet.sequence == sequence {
                break;
            }
        }
    }

    prove_rejected_bone_meal_unchanged_authoritatively(
        client,
        bone_meal_item_id,
        bone_meal_slot,
        label,
    )
    .await;
}

async fn prove_consumed_bone_meal_authoritatively(
    client: &mut Client,
    bone_meal_item_id: u32,
    label: &str,
) {
    send_followup_bone_meal_give(client, label).await;
    let mut saw_authoritative_count = false;
    let mut saw_feedback = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_authoritative_count && saw_feedback) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| panic!("{label} consumed bone meal proof timed out: {err}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode consumed bone meal authoritative SetSlot");
            if packet.container_id == 0 && packet.item_stack.item_id == bone_meal_item_id {
                assert_eq!(
                    packet.item_stack.count, 1,
                    "{label} follow-up give must observe authoritative prior consumption"
                );
                assert!(
                    !saw_authoritative_count,
                    "{label} follow-up give must produce one authoritative bone meal slot update"
                );
                saw_authoritative_count = true;
            }
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let _ = ClientboundSystemChat::decode(&mut body)
                .expect("decode consumed bone meal give feedback");
            saw_feedback = true;
        }
    }
}

async fn prove_rejected_bone_meal_unchanged_authoritatively(
    client: &mut Client,
    bone_meal_item_id: u32,
    bone_meal_slot: i16,
    label: &str,
) {
    send_followup_bone_meal_give(client, label).await;
    let mut saw_authoritative_count = false;
    let mut saw_feedback = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_authoritative_count && saw_feedback) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| panic!("{label} rejected bone meal proof timed out: {err}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode rejected bone meal authoritative SetSlot");
            assert_eq!(
                (packet.container_id, packet.slot),
                (0, bone_meal_slot),
                "{label} follow-up give must update the exact selected bone meal slot"
            );
            assert_eq!(
                (packet.item_stack.item_id, packet.item_stack.count),
                (bone_meal_item_id, 2),
                "{label} follow-up give must observe one authoritative retained bone meal"
            );
            assert!(
                !saw_authoritative_count,
                "{label} follow-up give must produce one selected-slot update"
            );
            saw_authoritative_count = true;
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let _ = ClientboundSystemChat::decode(&mut body)
                .expect("decode rejected bone meal give feedback");
            saw_feedback = true;
        }
    }
}

async fn send_followup_bone_meal_give(client: &mut Client, label: &str) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:bone_meal 1".into(),
        })
        .await
        .unwrap_or_else(|err| panic!("{label} follow-up bone meal give failed: {err}"));
}

fn sapling_section_change_pos(section_pos: i64, relative_pos: u16) -> (i32, i32, i32) {
    let section_x = sapling_unpack_signed_section_coord(section_pos >> 42, 22);
    let section_y = sapling_unpack_signed_section_coord(section_pos, 20);
    let section_z = sapling_unpack_signed_section_coord(section_pos >> 20, 22);
    (
        section_x * 16 + i32::from((relative_pos >> 8) & 15),
        section_y * 16 + i32::from(relative_pos & 15),
        section_z * 16 + i32::from((relative_pos >> 4) & 15),
    )
}

async fn move_mega_sapling_client(client: &mut Client, y: f64, x: f64, z: f64) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to unloaded-canopy fixture");
    client
        .write_packet(&ServerboundChatCommand {
            command: "time set 1000".into(),
        })
        .await
        .expect("send movement fence");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("movement fence response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let _ = ClientboundSystemChat::decode(&mut body).expect("decode movement fence");
            return;
        }
    }
}

fn sapling_test_state(
    blocks: &mc_world::BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> mc_world::BlockStateId {
    let id = mc_data::Identifier::parse(name).expect("static identifier");
    if props.is_empty() {
        return blocks
            .block(&id)
            .unwrap_or_else(|| panic!("missing block {name}"))
            .default;
    }
    let props = props
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .unwrap_or_else(|| panic!("missing block state {name} {props:?}"))
}

fn sapling_test_set(
    storage: &mut mc_world::WorldStorage,
    pos: (i32, i32, i32),
    state: mc_world::BlockStateId,
) {
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            state,
        )
        .expect("sapling fixture block edit succeeds");
}

async fn give_and_select_bone_meal(client: &mut Client, bone_meal_item_id: u32) -> i16 {
    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:bone_meal 1".into(),
        })
        .await
        .expect("give bone meal");
    let update = wait_for_slot_stack_update(client, bone_meal_item_id, 1).await;
    let hotbar_slot = update
        .slot
        .checked_sub(36)
        .filter(|slot| (0..=8).contains(slot));
    let hotbar_slot = match hotbar_slot {
        Some(slot) => slot,
        None => {
            let source_slot = usize::try_from(update.slot).expect("valid bone meal inventory slot");
            client
                .write_packet(&ServerboundContainerClick {
                    container_id: 0,
                    state_id: update.state_id,
                    slot_num: update.slot,
                    button_num: 0,
                    container_input: ContainerInput::Swap,
                    changed_slots: Vec::new(),
                    carried_item: HashedStack::empty(),
                })
                .await
                .expect("move bone meal to hotbar");
            wait_for_inventory_content(client, |packet| {
                packet.container_id == 0
                    && packet.items[source_slot].is_empty()
                    && packet.items[36].item_id == bone_meal_item_id
                    && packet.items[36].count == 1
            })
            .await;
            0
        }
    };
    client
        .write_packet(&ServerboundSetCarriedItem { slot: hotbar_slot })
        .await
        .expect("select bone meal");
    36 + hotbar_slot
}

async fn wait_for_sapling_tree_growth(
    client: &mut Client,
    log: ((i32, i32, i32), i32),
    leaf: ((i32, i32, i32), i32),
    bone_meal_slot: i16,
    bone_meal_item_id: u32,
    expected_bone_meal_count: i32,
    sequence: i32,
) {
    let (log_pos, log_state_id) = log;
    let (leaf_pos, leaf_state_id) = leaf;
    let mut saw_log = false;
    let mut saw_leaf = false;
    let mut saw_bonemeal_decrement = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_log && saw_leaf && saw_bonemeal_decrement && saw_ack) {
        let frame = match client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
        {
            Ok(frame) => frame,
            Err(err) => panic!(
                "sapling tree growth response timed out: log={saw_log} leaf={saw_leaf} \
                 bonemeal={saw_bonemeal_decrement} ack={saw_ack}: {err}"
            ),
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode sapling BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == log_pos && pkt.state_id == log_state_id {
                saw_log = true;
            } else if pos == leaf_pos && pkt.state_id == leaf_state_id {
                saw_leaf = true;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt =
                SectionBlocksUpdate::decode(&mut body).expect("decode sapling SectionBlocksUpdate");
            if sapling_section_pos_matches(pkt.section_pos, log_pos) {
                let relative = pack_section_relative_pos(log_pos.0, log_pos.1, log_pos.2);
                for change in &pkt.changes {
                    if change.relative_pos == relative && change.state_id == log_state_id {
                        saw_log = true;
                    }
                }
            }
            if sapling_section_pos_matches(pkt.section_pos, leaf_pos) {
                let relative = pack_section_relative_pos(leaf_pos.0, leaf_pos.1, leaf_pos.2);
                for change in &pkt.changes {
                    if change.relative_pos == relative && change.state_id == leaf_state_id {
                        saw_leaf = true;
                    }
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode sapling bonemeal SetSlot");
            saw_bonemeal_decrement |= pkt.container_id == 0
                && pkt.slot == bone_meal_slot
                && if expected_bone_meal_count == 0 {
                    pkt.item_stack.is_empty()
                } else {
                    pkt.item_stack.item_id == bone_meal_item_id
                        && pkt.item_stack.count == expected_bone_meal_count
                };
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode sapling ack");
            saw_ack |= pkt.sequence == sequence;
        }
    }
}

async fn wait_for_sapling_stage_growth(
    client: &mut Client,
    sapling_pos: (i32, i32, i32),
    stage_state_id: i32,
    bone_meal_slot: i16,
    bone_meal_item_id: u32,
    expected_bone_meal_count: i32,
    sequence: i32,
) {
    let mut saw_stage = false;
    let mut saw_bonemeal_decrement = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_stage && saw_bonemeal_decrement && saw_ack) {
        let frame = match client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
        {
            Ok(frame) => frame,
            Err(err) => panic!(
                "sapling stage response timed out: stage={saw_stage} \
                 bonemeal={saw_bonemeal_decrement} ack={saw_ack}: {err}"
            ),
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode staged sapling BlockUpdate");
            if unpack_block_pos(pkt.position) == sapling_pos && pkt.state_id == stage_state_id {
                saw_stage = true;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body)
                .expect("decode staged sapling SectionBlocksUpdate");
            if sapling_section_pos_matches(pkt.section_pos, sapling_pos) {
                let relative =
                    pack_section_relative_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2);
                for change in &pkt.changes {
                    if change.relative_pos == relative && change.state_id == stage_state_id {
                        saw_stage = true;
                    }
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode staged sapling bonemeal SetSlot");
            saw_bonemeal_decrement |= pkt.container_id == 0
                && pkt.slot == bone_meal_slot
                && if expected_bone_meal_count == 0 {
                    pkt.item_stack.is_empty()
                } else {
                    pkt.item_stack.item_id == bone_meal_item_id
                        && pkt.item_stack.count == expected_bone_meal_count
                };
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode staged sapling ack");
            saw_ack |= pkt.sequence == sequence;
        }
    }
}

fn sapling_section_pos_matches(section_pos: i64, target: (i32, i32, i32)) -> bool {
    let sx = sapling_unpack_signed_section_coord(section_pos >> 42, 22);
    let sy = sapling_unpack_signed_section_coord(section_pos, 20);
    let sz = sapling_unpack_signed_section_coord(section_pos >> 20, 22);
    sx == target.0.div_euclid(16) && sy == target.1.div_euclid(16) && sz == target.2.div_euclid(16)
}

fn sapling_unpack_signed_section_coord(value: i64, bits: u8) -> i32 {
    let mask = (1_i64 << bits) - 1;
    let sign = 1_i64 << (bits - 1);
    let value = value & mask;
    let signed = if value & sign == 0 {
        value
    } else {
        value - (1_i64 << bits)
    };
    signed as i32
}
