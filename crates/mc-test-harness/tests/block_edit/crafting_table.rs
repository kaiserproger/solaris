#[test]
#[ignore = "requires local data/vanilla sidecars"]
fn crafting_table_container_crafts_shapeless_and_shaped_results() {
    let test = std::thread::Builder::new()
        .name("crafting_table_container_crafts_shapeless_and_shaped_results".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build crafting table container runtime")
                .block_on(crafting_table_container_crafts_shapeless_and_shaped_results_inner());
        })
        .expect("spawn crafting table container runtime");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn crafting_table_container_crafts_shapeless_and_shaped_results_inner() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        panic!(
            "prerequisite failed: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let crafting_table_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .map(|b| b.default)
        .expect("crafting_table in registry");
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|block| block.default)
        .expect("air in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let table_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("table column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(table_pos, crafting_table_state)
        .expect("seed crafting table")
        .expect("table chunk exists");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );
    let crafting_table_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .expect("crafting_table item");
    let oak_log_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak_log item");
    let oak_planks_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak_planks item");
    let recipes = Arc::new(
        mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
            .expect("recipes load"),
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:oak_planks"),
        "oak planks recipe must come from sidecar"
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:crafting_table"),
        "crafting table recipe must come from sidecar"
    );
    let crafting_menu_id = 12;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M24 crafting table".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes,
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M24TableCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 1".into(),
        })
        .await
        .expect("give oak log");
    wait_for_slot_stack(&mut client, oak_log_id, 2).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(table_pos.x, table_pos.y, table_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 102,
        })
        .await
        .expect("open crafting table");
    let opened = wait_for_open_screen(&mut client, crafting_menu_id).await;
    let mut content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items.len() == 46 && pkt.items[0].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 38,
            button_num: 0,
            container_input: ContainerInput::Throw,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("throw one oak log from crafting window");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.is_empty()
            && pkt.items[38].item_id == oak_log_id
            && pkt.items[38].count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 1,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("swap oak log into crafting grid");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == oak_planks_id
            && pkt.items[0].count == 4
            && pkt.items[1].item_id == oak_log_id
            && pkt.items[38].is_empty()
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("shift-click planks result");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].is_empty()
            && pkt.items[1].is_empty()
            && pkt.items.iter().enumerate().any(|(slot, stack)| {
                slot >= 10 && stack.item_id == oak_planks_id && stack.count == 4
            })
    })
    .await;

    let planks_menu_slot = content
        .items
        .iter()
        .enumerate()
        .find_map(|(slot, stack)| {
            (slot >= 10 && stack.item_id == oak_planks_id && stack.count == 4)
                .then_some(slot as i16)
        })
        .expect("planks in table inventory");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: planks_menu_slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: oak_planks_id,
                count: 4,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up planks");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == oak_planks_id && pkt.carried_item.count == 4
    })
    .await;
    let hashed_planks = |count| HashedStack::Actual {
        item_id: oak_planks_id,
        count,
        components: HashedStackComponentHashes::empty(),
    };
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: -999,
            button_num: 4,
            container_input: ContainerInput::QuickCraft,
            changed_slots: Vec::new(),
            carried_item: hashed_planks(4),
        })
        .await
        .expect("start crafting-table quick craft");
    for grid_slot in [1, 2, 4, 5] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: grid_slot,
                button_num: 5,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item: hashed_planks(4),
            })
            .await
            .expect("add crafting-table quick craft slot");
    }
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: -999,
            button_num: 6,
            container_input: ContainerInput::QuickCraft,
            changed_slots: [1, 2, 4, 5]
                .into_iter()
                .map(|slot| (slot, hashed_planks(1)))
                .collect(),
            carried_item: HashedStack::Empty,
        })
        .await
        .expect("finish crafting-table quick craft");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == crafting_table_id
            && pkt.items[0].count == 1
            && [1, 2, 4, 5]
                .into_iter()
                .all(|slot| pkt.items[slot].item_id == oak_planks_id && pkt.items[slot].count == 1)
            && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(content.items[0].item_id, crafting_table_id);
    assert_eq!(content.items[0].count, 1);
    assert!(content.carried_item.is_empty());

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: crafting_table_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("take shaped crafting result");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == crafting_table_id
            && pkt.carried_item.count == 1
            && pkt.items[0].is_empty()
            && (1..=9).all(|slot| pkt.items[slot].is_empty())
    })
    .await;
}

#[test]
fn crafting_table_shift_click_max_crafts_every_matching_input() {
    let test = std::thread::Builder::new()
        .name("crafting_table_shift_click_max_crafts_every_matching_input".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build crafting table max-craft runtime")
                .block_on(crafting_table_shift_click_max_crafts_every_matching_input_inner());
        })
        .expect("spawn crafting table max-craft runtime");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn crafting_table_shift_click_max_crafts_every_matching_input_inner() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let crafting_table_state = embedded_block_state(&data, "minecraft:crafting_table");
    let oak_log_id = embedded_item_id(&data, "minecraft:oak_log");
    let oak_planks_id = embedded_item_id(&data, "minecraft:oak_planks");
    let mut world = embedded_world(&data);
    let support_y = top_non_air_y(&mut world, 2, 2, air_state).expect("table column terrain");
    let table_pos = mc_world::BlockPos {
        x: 2,
        y: support_y + 1,
        z: 2,
    };
    world
        .set_block_at(table_pos, crafting_table_state)
        .expect("seed max-craft table")
        .expect("replace table target");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "crafting max-craft wire");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind max-craft server");
    let addr = bound.local_addr().expect("max-craft local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "MaxCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 5 0".into(),
        })
        .await
        .expect("give max-craft logs");
    wait_for_slot_stack(&mut client, oak_log_id, 5).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(table_pos.x, table_pos.y, table_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 451,
        })
        .await
        .expect("open max-craft table");
    let opened = wait_for_open_screen(&mut client, 12).await;
    let mut content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items.len() == 46 && packet.items[0].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("swap all logs into crafting input");
    content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].item_id == oak_planks_id
            && packet.items[0].count == 4
            && packet.items[1].item_id == oak_log_id
            && packet.items[1].count == 5
            && packet.items[37].is_empty()
            && packet.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("max-craft all matching logs");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].is_empty()
            && packet.items[1].is_empty()
            && packet.carried_item.is_empty()
            && packet.items[10..=45]
                .iter()
                .filter(|stack| stack.item_id == oak_planks_id)
                .map(|stack| stack.count)
                .sum::<i32>()
                == 20
            && packet.items[10..=45]
                .iter()
                .all(|stack| stack.item_id != oak_log_id)
    })
    .await;

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("max-craft server shutdown")
        .expect("max-craft server join")
        .expect("max-craft server serve");
}
