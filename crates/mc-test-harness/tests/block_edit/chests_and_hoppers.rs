#[test]
fn two_clients_stale_chest_click_after_peer_update_resyncs() {
    let test = std::thread::Builder::new()
        .name("two_clients_stale_chest_click_after_peer_update_resyncs".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest integration runtime")
                .block_on(two_clients_stale_chest_click_after_peer_update_resyncs_inner());
        })
        .expect("spawn chest integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn two_clients_stale_chest_click_after_peer_update_resyncs_inner() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state = blocks.block(&chest_id).expect("chest block").default;
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let chest_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("chest column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(chest_pos, chest_state)
        .expect("seed chest block")
        .expect("chest chunk exists");
    storage
        .set_chest_block_entity(chest_pos, mc_world::ChestBlockEntity::default())
        .expect("seed chest entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 stale chest resync".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut actor, _) = connect_to_play(addr, "M100ChestActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;

    let (mut observer, _) = connect_to_play(addr, "M100ChestObs").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 195,
        })
        .await
        .expect("observer opens chest");
    let observer_opened = wait_for_open_screen(&mut observer, chest_menu_id).await;
    let observer_initial =
        wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
            pkt.items[0].is_empty() && pkt.carried_item.is_empty()
        })
        .await;

    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut actor, dirt_id, 1).await;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 196,
        })
        .await
        .expect("actor opens chest");
    let actor_opened = wait_for_open_screen(&mut actor, chest_menu_id).await;
    let actor_content =
        wait_for_furnace_content(&mut actor, actor_opened.container_id, |_| true).await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 54,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt");
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.carried_item.item_id == dirt_id && pkt.carried_item.count == 1
    })
    .await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place dirt in chest");
    wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id && pkt.items[0].count == 1 && pkt.carried_item.is_empty()
    })
    .await;
    let observer_slot =
        wait_for_container_slot(&mut observer, observer_opened.container_id, 0, |stack| {
            stack.item_id == dirt_id && stack.count == 1
        })
        .await;
    assert!(
        observer_slot.state_id > observer_initial.state_id,
        "peer chest update should advance the shared container state"
    );

    observer
        .write_packet(&ServerboundContainerClick {
            container_id: observer_opened.container_id,
            state_id: observer_initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("send stale observer chest click");
    let resync = wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.state_id == observer_slot.state_id
            && pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(resync.items[0].item_id, dirt_id);
    assert!(resync.carried_item.is_empty());
}

#[test]
fn server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp() {
    let test = std::thread::Builder::new()
        .name("server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest integration runtime")
                .block_on(server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp_inner());
        })
        .expect("spawn chest integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp_inner() {
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
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let hopper_id = mc_data::Identifier::parse("minecraft:hopper").unwrap();
    let comparator_id = mc_data::Identifier::parse("minecraft:comparator").unwrap();
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_state_id = blocks.block(&chest_id).expect("chest block").default;
    let hopper_state_id = blocks
        .by_name_and_props(
            &hopper_id,
            &[
                ("enabled".to_string(), "true".to_string()),
                ("facing".to_string(), "down".to_string()),
            ],
        )
        .expect("enabled down-facing hopper state");
    let comparator_off_state_id = blocks
        .by_name_and_props(
            &comparator_id,
            &[
                ("facing".to_string(), "west".to_string()),
                ("mode".to_string(), "compare".to_string()),
                ("powered".to_string(), "false".to_string()),
            ],
        )
        .expect("unpowered west-facing comparator state");
    let comparator_on_state_id = blocks
        .by_name_and_props(
            &comparator_id,
            &[
                ("facing".to_string(), "west".to_string()),
                ("mode".to_string(), "compare".to_string()),
                ("powered".to_string(), "true".to_string()),
            ],
        )
        .expect("powered west-facing comparator state");
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let hopper_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("hopper column terrain"),
        z: 2,
    };
    let source_pos = mc_world::BlockPos {
        y: hopper_pos.y + 1,
        ..hopper_pos
    };
    let target_pos = mc_world::BlockPos {
        y: hopper_pos.y - 1,
        ..hopper_pos
    };
    let comparator_pos = mc_world::BlockPos {
        x: target_pos.x + 1,
        ..target_pos
    };
    for (position, state, label) in [
        (source_pos, chest_state_id, "source chest"),
        (hopper_pos, hopper_state_id, "hopper"),
        (target_pos, chest_state_id, "target chest"),
        (comparator_pos, comparator_off_state_id, "comparator"),
    ] {
        storage
            .set_block_at(position, state)
            .unwrap_or_else(|error| panic!("seed {label} block: {error}"))
            .unwrap_or_else(|| panic!("{label} chunk exists"));
    }
    storage
        .set_chest_block_entity(source_pos, mc_world::ChestBlockEntity::default())
        .expect("seed source chest entity");
    storage
        .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
        .expect("seed target chest entity");
    storage
        .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
        .expect("seed hopper entity");
    assert!(
        storage
            .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                hopper_pos,
                hopper_id.clone(),
                0,
                0,
            ))
            .expect("schedule hopper tick"),
        "hopper tick should be newly scheduled before bind"
    );
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 hopper TCP chest and comparator updates".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut source_client, _) = connect_to_play(addr, "M100HopperSource").await;
    drain_until_chunk(&mut source_client, (0, 0)).await;
    source_client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give source dirt");
    wait_for_slot_stack(&mut source_client, dirt_id, 1).await;
    source_client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(source_pos.x, source_pos.y, source_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 230,
        })
        .await
        .expect("source viewer opens chest");
    let source_opened = wait_for_open_screen(&mut source_client, chest_menu_id).await;
    let source_initial =
        wait_for_furnace_content(&mut source_client, source_opened.container_id, |pkt| {
            pkt.items[0].is_empty()
                && pkt.items[54].item_id == dirt_id
                && pkt.items[54].count == 1
                && pkt.carried_item.is_empty()
        })
        .await;

    let (mut target_client, _) = connect_to_play(addr, "M100HopperTarget").await;
    drain_until_chunk(&mut target_client, (0, 0)).await;
    target_client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(target_pos.x, target_pos.y, target_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 231,
        })
        .await
        .expect("target viewer opens chest");
    let target_opened = wait_for_open_screen(&mut target_client, chest_menu_id).await;
    let target_initial =
        wait_for_furnace_content(&mut target_client, target_opened.container_id, |pkt| {
            pkt.items[0].is_empty() && pkt.carried_item.is_empty()
        })
        .await;

    source_client
        .write_packet(&ServerboundContainerClick {
            container_id: source_opened.container_id,
            state_id: source_initial.state_id,
            slot_num: 54,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up source dirt");
    let carrying =
        wait_for_furnace_content(&mut source_client, source_opened.container_id, |pkt| {
            pkt.carried_item.item_id == dirt_id && pkt.carried_item.count == 1
        })
        .await;
    source_client
        .write_packet(&ServerboundContainerClick {
            container_id: source_opened.container_id,
            state_id: carrying.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place source dirt");

    let source_empty = wait_for_container_slot(
        &mut source_client,
        source_opened.container_id,
        0,
        mc_protocol::packets::play::ItemStack::is_empty,
    )
    .await;
    assert!(
        source_empty.state_id > source_initial.state_id,
        "server-origin hopper pull should advance the source chest state"
    );

    let target_dirt =
        wait_for_container_slot(&mut target_client, target_opened.container_id, 0, |stack| {
            stack.item_id == dirt_id && stack.count == 1
        })
        .await;
    assert!(
        target_dirt.state_id > target_initial.state_id,
        "cooldown-delayed hopper eject should advance the target chest state"
    );
    wait_for_block_update(
        &mut target_client,
        (comparator_pos.x, comparator_pos.y, comparator_pos.z),
        comparator_on_state_id.0 as i32,
    )
    .await;
}

#[test]
fn chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots() {
    let test = std::thread::Builder::new()
        .name("chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest QuickCraft test runtime")
                .block_on(chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots_inner());
        })
        .expect("spawn chest QuickCraft test thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots_inner() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state = blocks.block(&chest_id).expect("chest block").default;
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let chest_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("chest column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(chest_pos, chest_state)
        .expect("seed chest block")
        .expect("chest chunk exists");
    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 5,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_chest_block_entity(chest_pos, chest)
        .expect("seed chest entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 chest quickcraft".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut client, _) = connect_to_play(addr, "M100QuickCraft").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 202,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let initial = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 5
            && pkt.items[1].is_empty()
            && pkt.items[2].is_empty()
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 5,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let carrying = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > initial.state_id
            && pkt.items[0].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 5
    })
    .await;

    for (slot_num, button_num, carried_count, changed_slots) in [
        (-999, 0, 5, Vec::new()),
        (
            1,
            1,
            5,
            vec![(
                1,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 2,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            2,
            1,
            5,
            vec![(
                2,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 2,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            -999,
            2,
            1,
            vec![
                (
                    1,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 2,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
                (
                    2,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 2,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
            ],
        ),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: carrying.state_id,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots,
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: carried_count,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send chest quickcraft stage");
    }

    let final_content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > carrying.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 2
            && pkt.items[2].item_id == dirt_id
            && pkt.items[2].count == 2
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 1
    })
    .await;
    assert!(
        final_content.state_id > carrying.state_id,
        "QuickCraft end should advance the chest state id after storage mutation"
    );
}

#[test]
fn chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack() {
    let test = std::thread::Builder::new()
        .name("chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest QuickCraft test runtime")
                .block_on(chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack_inner());
        })
        .expect("spawn chest QuickCraft test thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack_inner() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state = blocks.block(&chest_id).expect("chest block").default;
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let chest_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("chest column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(chest_pos, chest_state)
        .expect("seed chest block")
        .expect("chest chunk exists");
    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 5,
        damage: None,
        enchantments: Vec::new(),
    };
    chest.slots[1] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 63,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_chest_block_entity(chest_pos, chest)
        .expect("seed chest entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 chest right quickcraft".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut client, _) = connect_to_play(addr, "M100RightQC").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 204,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let initial = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 5
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 63
            && pkt.items[2].is_empty()
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 5,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let carrying = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > initial.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 63
            && pkt.items[2].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 5
    })
    .await;

    for (slot_num, button_num, carried_count, changed_slots) in [
        (-999, 4, 5, Vec::new()),
        (
            1,
            5,
            5,
            vec![(
                1,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 64,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            2,
            5,
            5,
            vec![(
                2,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 1,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            -999,
            6,
            3,
            vec![
                (
                    1,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 64,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
                (
                    2,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 1,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
            ],
        ),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: carrying.state_id,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots,
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: carried_count,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send chest right quickcraft stage");
    }

    let final_content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > carrying.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 64
            && pkt.items[2].item_id == dirt_id
            && pkt.items[2].count == 1
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 3
    })
    .await;
    assert!(
        final_content.state_id > carrying.state_id,
        "right QuickCraft end should advance the chest state id after storage mutation"
    );
}

#[test]
fn unsupported_chest_click_modes_resync_without_trusting_client_slots() {
    let test = std::thread::Builder::new()
        .name("unsupported_chest_click_modes_resync_without_trusting_client_slots".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest integration runtime")
                .block_on(unsupported_chest_click_modes_resync_without_trusting_client_slots_inner());
        })
        .expect("spawn chest integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn unsupported_chest_click_modes_resync_without_trusting_client_slots_inner() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state = blocks.block(&chest_id).expect("chest block").default;
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let chest_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("chest column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(chest_pos, chest_state)
        .expect("seed chest block")
        .expect("chest chunk exists");
    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 3,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_chest_block_entity(chest_pos, chest)
        .expect("seed chest entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed chest click".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut client, _) = connect_to_play(addr, "M100BadChest").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 198,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id && pkt.items[0].count == 3 && pkt.carried_item.is_empty()
    })
    .await;

    for (container_input, button_num) in [
        (ContainerInput::QuickCraft, 0),
        (ContainerInput::Clone, 2),
        (ContainerInput::PickupAll, 0),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: 0,
                button_num,
                container_input,
                changed_slots: vec![(0, HashedStack::empty())],
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: 3,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send unsupported chest click");
        wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
            pkt.state_id == content.state_id
                && pkt.items[0].item_id == dirt_id
                && pkt.items[0].count == 3
                && pkt.carried_item.is_empty()
        })
        .await;
    }
}


#[test]
fn chest_rejects_overstack_predictions_and_recovers_with_exact_item_limits() {
    let test = std::thread::Builder::new()
        .name("chest_rejects_overstack_predictions_and_recovers_with_exact_item_limits".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build chest item-limit replay runtime")
                .block_on(chest_rejects_overstack_predictions_and_recovers_with_exact_item_limits_inner());
        })
        .expect("spawn chest item-limit replay thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn chest_rejects_overstack_predictions_and_recovers_with_exact_item_limits_inner() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let chest_state = embedded_block_state(&data, "minecraft:chest");
    let bucket_id = embedded_item_id(&data, "minecraft:bucket");
    let snowball_id = embedded_item_id(&data, "minecraft:snowball");
    let mut world = embedded_world(&data);
    let chest_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut world, 2, 2, air_state).expect("chest limit column terrain") + 1,
        z: 2,
    };
    world
        .set_block_at(chest_pos, chest_state)
        .expect("seed chest limit block")
        .expect("replace chest limit target");
    let mut chest = mc_world::ChestBlockEntity::default();
    chest.slots[0] = mc_world::FurnaceSlot {
        item_id: bucket_id,
        count: 1,
        damage: None,
        enchantments: Vec::new(),
    };
    chest.slots[1] = mc_world::FurnaceSlot {
        item_id: snowball_id,
        count: 16,
        damage: None,
        enchantments: Vec::new(),
    };
    world
        .set_chest_block_entity(chest_pos, chest)
        .expect("seed chest limit entity");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "chest max-stack replay");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind chest limit server");
    let addr = bound.local_addr().expect("chest limit local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "ChestLimits").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 461,
        })
        .await
        .expect("open chest limit container");
    let opened = wait_for_open_screen(&mut client, 2).await;
    let initial = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].item_id == bucket_id
            && packet.items[0].count == 1
            && packet.items[1].item_id == snowball_id
            && packet.items[1].count == 16
            && packet.carried_item.is_empty()
    })
    .await;

    for (slot_num, item_id, impossible_count) in [(0, bucket_id, 2), (1, snowball_id, 17)] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: initial.state_id,
                slot_num,
                button_num: 0,
                container_input: ContainerInput::Pickup,
                changed_slots: vec![(slot_num, HashedStack::empty())],
                carried_item: HashedStack::Actual {
                    item_id,
                    count: impossible_count,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send impossible max-stack prediction");
        wait_for_furnace_content(&mut client, opened.container_id, |packet| {
            packet.state_id == initial.state_id
                && packet.items[0].item_id == bucket_id
                && packet.items[0].count == 1
                && packet.items[1].item_id == snowball_id
                && packet.items[1].count == 16
                && packet.carried_item.is_empty()
        })
        .await;
    }

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: bucket_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("recover with valid bucket pickup");
    let carrying_bucket = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.state_id == initial.state_id.wrapping_add(2)
            && packet.items[0].is_empty()
            && packet.items[1].item_id == snowball_id
            && packet.items[1].count == 16
            && packet.carried_item.item_id == bucket_id
            && packet.carried_item.count == 1
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: carrying_bucket.state_id,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(1, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: bucket_id,
                count: 2,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send impossible cursor prediction after recovery");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.state_id == carrying_bucket.state_id
            && packet.items[0].is_empty()
            && packet.items[1].item_id == snowball_id
            && packet.items[1].count == 16
            && packet.carried_item.item_id == bucket_id
            && packet.carried_item.count == 1
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: carrying_bucket.state_id,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(
                2,
                HashedStack::Actual {
                    item_id: bucket_id,
                    count: 1,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place recovered bucket into empty chest slot");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.state_id == carrying_bucket.state_id.wrapping_add(2)
            && packet.items[0].is_empty()
            && packet.items[1].item_id == snowball_id
            && packet.items[1].count == 16
            && packet.items[2].item_id == bucket_id
            && packet.items[2].count == 1
            && packet.carried_item.is_empty()
    })
    .await;

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("chest limit server shutdown")
        .expect("chest limit server join")
        .expect("chest limit server serve");
}
