#[tokio::test]
async fn two_clients_stale_chest_click_after_peer_update_resyncs() {
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
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut actor, sync) = connect_to_play(addr, "M100ChestActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut actor, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 194,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut actor, (0, chest_y, 0), chest_state_id).await;

    let (mut observer, _) = connect_to_play(addr, "M100ChestObs").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, chest_y, 0),
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
    let observer_initial = wait_for_furnace_content(
        &mut observer,
        observer_opened.container_id,
        |pkt| pkt.items[0].is_empty() && pkt.carried_item.is_empty(),
    )
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
            position: pack_block_pos(0, chest_y, 0),
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
    let observer_slot = wait_for_container_slot(
        &mut observer,
        observer_opened.container_id,
        0,
        |stack| stack.item_id == dirt_id && stack.count == 1,
    )
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

#[tokio::test]
async fn server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp() {
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
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut source_client, sync) = connect_to_play(addr, "M100HopperSource").await;
    drain_until_chunk(&mut source_client, (0, 0)).await;
    let hopper_pos = mc_world::BlockPos {
        x: 1,
        y: sync.y.floor() as i32 - 2,
        z: 0,
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
    {
        let mut world = world_handle.lock().await;
        world
            .set_block_at(source_pos, chest_state_id)
            .expect("set source chest block")
            .expect("source chunk exists");
        world
            .set_block_at(hopper_pos, hopper_state_id)
            .expect("set hopper block")
            .expect("hopper chunk exists");
        world
            .set_block_at(target_pos, chest_state_id)
            .expect("set target chest block")
            .expect("target chunk exists");
        world
            .set_block_at(comparator_pos, comparator_off_state_id)
            .expect("set comparator block")
            .expect("comparator chunk exists");

        let mut source_chest = mc_world::ChestBlockEntity::default();
        source_chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 1,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(source_pos, source_chest)
            .expect("seed source chest entity");
        world
            .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
            .expect("seed target chest entity");
    }

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
            pkt.items[0].item_id == dirt_id
                && pkt.items[0].count == 1
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

    {
        let mut world = world_handle.lock().await;
        world
            .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
            .expect("seed hopper entity");
        assert!(
            world
                .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                    hopper_pos,
                    hopper_id.clone(),
                    0,
                    0,
                ))
                .expect("schedule hopper tick"),
            "hopper tick should be newly scheduled after viewers open"
        );
    }

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

    let target_dirt = wait_for_container_slot(
        &mut target_client,
        target_opened.container_id,
        0,
        |stack| stack.item_id == dirt_id && stack.count == 1,
    )
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

#[tokio::test]
async fn chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots() {
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
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100QuickCraft").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 201,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 5,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

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

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert!(chest.slots[0].is_empty());
    assert_eq!(chest.slots[1].item_id, dirt_id);
    assert_eq!(chest.slots[1].count, 2);
    assert_eq!(chest.slots[2].item_id, dirt_id);
    assert_eq!(chest.slots[2].count, 2);
}

#[tokio::test]
async fn chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack() {
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
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100RightQC").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 203,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
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
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

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

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert!(chest.slots[0].is_empty());
    assert_eq!(chest.slots[1].item_id, dirt_id);
    assert_eq!(chest.slots[1].count, 64);
    assert_eq!(chest.slots[2].item_id, dirt_id);
    assert_eq!(chest.slots[2].count, 1);
}

#[tokio::test]
async fn unsupported_chest_click_modes_resync_without_trusting_client_slots() {
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
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100BadChest").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 197,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 3,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

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

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert_eq!(chest.slots[0].item_id, dirt_id);
    assert_eq!(chest.slots[0].count, 3);
}

