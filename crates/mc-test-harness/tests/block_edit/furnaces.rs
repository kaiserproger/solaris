#[tokio::test]
async fn two_clients_stale_furnace_click_after_peer_update_resyncs() {
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
    let furnace_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default)
        .expect("furnace in registry");
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
    let furnace_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("furnace column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(furnace_pos, furnace_state)
        .expect("seed furnace block")
        .expect("furnace chunk exists");
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .expect("seed furnace entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let tags = Arc::new(
        mc_data::tags::load(&vanilla_dir, &data)
            .expect("tags load")
            .with_vanilla_fuel_values(&items),
    );
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 stale furnace resync".into(),
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
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut actor, _) = connect_to_play(addr, "M100FurnActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;

    let (mut observer, _) = connect_to_play(addr, "M100FurnObserve").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 192,
        })
        .await
        .expect("observer opens furnace");
    let observer_opened = wait_for_open_screen(&mut observer, furnace_menu_id).await;
    let observer_initial =
        wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
            pkt.items[0].is_empty() && pkt.carried_item.is_empty()
        })
        .await;

    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:raw_iron 1 0".into(),
        })
        .await
        .expect("give raw iron");
    wait_for_slot_stack(&mut actor, raw_iron_id, 1).await;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 193,
        })
        .await
        .expect("actor opens furnace");
    let actor_opened = wait_for_open_screen(&mut actor, furnace_menu_id).await;
    let actor_content =
        wait_for_furnace_content(&mut actor, actor_opened.container_id, |_| true).await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 30,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: raw_iron_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up raw iron");
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.carried_item.item_id == raw_iron_id && pkt.carried_item.count == 1
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
        .expect("place raw iron input");
    wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    let observer_slot =
        wait_for_container_slot(&mut observer, observer_opened.container_id, 0, |stack| {
            stack.item_id == raw_iron_id && stack.count == 1
        })
        .await;
    assert!(
        observer_slot.state_id > observer_initial.state_id,
        "peer furnace update should advance the shared container state"
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
        .expect("send stale observer furnace click");
    let resync = wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.state_id == observer_slot.state_id
            && pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(resync.items[0].item_id, raw_iron_id);
    assert!(resync.carried_item.is_empty());
}

#[tokio::test]
async fn malformed_furnace_clicks_resync_without_trusting_client_slots() {
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
    let furnace_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|block| block.default)
        .expect("furnace in registry");
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|block| block.default)
        .expect("air in registry");
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let tags = Arc::new(
        mc_data::tags::load(&vanilla_dir, &data)
            .expect("tags load")
            .with_vanilla_fuel_values(&items),
    );
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let furnace_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("furnace column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(furnace_pos, furnace_state)
        .expect("seed furnace block")
        .expect("furnace chunk exists");
    let mut furnace = mc_world::FurnaceBlockEntity::default();
    furnace.slots[0] = mc_world::FurnaceSlot {
        item_id: raw_iron_id,
        count: 3,
        damage: None,
        enchantments: Vec::new(),
    };
    storage
        .set_furnace_block_entity(furnace_pos, furnace)
        .expect("seed furnace entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed furnace click".into(),
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
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M100BadFurn").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 200,
        })
        .await
        .expect("open furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 3
            && pkt.carried_item.is_empty()
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
                    item_id: raw_iron_id,
                    count: 3,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send unsupported furnace click");
        wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
            pkt.state_id == content.state_id
                && pkt.items[0].item_id == raw_iron_id
                && pkt.items[0].count == 3
                && pkt.carried_item.is_empty()
        })
        .await;
    }

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send pickup with impossible carried item");
    let resync = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    assert_eq!(
        resync.state_id, content.state_id,
        "malformed pickup should resync without advancing furnace state"
    );
    assert_eq!(resync.items[0].item_id, raw_iron_id);
    assert_eq!(resync.items[0].count, 3);
    assert!(
        resync.carried_item.is_empty(),
        "server cursor should stay authoritative instead of trusting client carried item"
    );
    assert!(
        resync
            .items
            .iter()
            .enumerate()
            .skip(3)
            .all(|(_, stack)| stack.item_id != raw_iron_id),
        "malformed pickup must not move furnace input into player inventory slots"
    );
}

#[tokio::test]
async fn survival_furnace_container_smelts_input_with_fuel() {
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
    let furnace_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default)
        .expect("furnace in registry");
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
    let furnace_pos = mc_world::BlockPos {
        x: 2,
        y: top_non_air_y(&mut storage, 2, 2, air_state).expect("furnace column terrain") + 1,
        z: 2,
    };
    storage
        .set_block_at(furnace_pos, furnace_state)
        .expect("seed furnace block")
        .expect("furnace chunk exists");
    storage
        .set_furnace_block_entity(furnace_pos, mc_world::FurnaceBlockEntity::default())
        .expect("seed furnace entity");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let tags = Arc::new(
        mc_data::tags::load(&vanilla_dir, &data)
            .expect("tags load")
            .with_vanilla_fuel_values(&items),
    );
    let raw_iron = mc_data::Identifier::parse("minecraft:raw_iron").unwrap();
    let iron_ingot = mc_data::Identifier::parse("minecraft:iron_ingot").unwrap();
    let raw_iron_id = items.id_of(&raw_iron).expect("raw_iron item");
    let oak_stairs_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_stairs").unwrap())
        .expect("oak stairs item");
    let iron_ingot_id = items.id_of(&iron_ingot).expect("iron_ingot item");
    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: mc_data::Identifier::parse("minecraft:test_raw_iron_smelting").unwrap(),
        kind: mc_data::recipes::RecipeKind::Smelting(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(raw_iron)],
            },
            cooking_time: 4,
            experience_milli: 1_000,
        }),
        result: mc_data::recipes::RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    }]);
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let experience_orb_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:experience_orb").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("experience orb entity type");
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 smelting".into(),
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
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M23Smelter").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let (mut observer, observer_sync) = connect_to_play(addr, "M24FurnaceViewer").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 90,
        })
        .await
        .expect("observer opens furnace");
    let observer_opened = wait_for_open_screen(&mut observer, furnace_menu_id).await;
    wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.items[0].is_empty() && pkt.items[1].is_empty() && pkt.items[2].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:raw_iron 1 0".into(),
        })
        .await
        .expect("give raw_iron");
    wait_for_slot_stack(&mut client, raw_iron_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_stairs 1 1".into(),
        })
        .await
        .expect("give oak stairs");
    wait_for_slot_stack(&mut client, oak_stairs_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 92,
        })
        .await
        .expect("open furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    assert_eq!(content.items.len(), 39);

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 30,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: raw_iron_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up raw iron");
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == raw_iron_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place raw iron input");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    wait_for_container_slot(&mut observer, observer_opened.container_id, 0, |stack| {
        stack.item_id == raw_iron_id && stack.count == 1
    })
    .await;

    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close furnace after input insert");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 93,
        })
        .await
        .expect("reopen furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 31,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: oak_stairs_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up oak stairs");
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == oak_stairs_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place oak stairs fuel");
    wait_for_container_slot(&mut observer, observer_opened.container_id, 1, |stack| {
        stack.item_id == oak_stairs_id && stack.count == 1
    })
    .await;

    wait_for_furnace_data(&mut client, opened.container_id, 2, |value| value > 0).await;
    wait_for_furnace_data(&mut observer, observer_opened.container_id, 2, |value| {
        value > 0
    })
    .await;
    let output = wait_for_container_slot(&mut client, opened.container_id, 2, |stack| {
        stack.item_id == iron_ingot_id && stack.count == 1
    })
    .await;
    let observer_x = observer_sync.x + 4.0;
    observer
        .write_packet(&ServerboundChatCommand {
            command: format!("tp {observer_x} {} {}", observer_sync.y, observer_sync.z),
        })
        .await
        .expect("move furnace observer outside XP pickup radius");
    let observer_position =
        wait_for_position_correction(&mut observer, Duration::from_secs(2)).await;
    assert_position_near(
        &observer_position,
        observer_x,
        observer_sync.y,
        observer_sync.z,
        0.001,
    );
    observer
        .write_packet(&ConfirmTeleportation {
            teleport_id: observer_position.teleport_id,
        })
        .await
        .expect("confirm furnace observer position");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: output.state_id,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("shift-click result");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_inventory_result = false;
    let mut saw_experience_orb = false;
    let mut saw_experience_credit = false;
    while !saw_inventory_result || !saw_experience_orb || !saw_experience_credit {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "furnace result/XP events failed: inventory={saw_inventory_result} \
                     orb={saw_experience_orb} credit={saw_experience_credit}: {error}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode furnace result content");
            saw_inventory_result |= packet.container_id == opened.container_id
                && packet.items[2].is_empty()
                && packet
                    .items
                    .iter()
                    .skip(3)
                    .any(|stack| stack.item_id == iron_ingot_id && stack.count == 1);
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode furnace experience orb");
            saw_experience_orb |=
                packet.entity_type_id == experience_orb_type_id && packet.data == 1;
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode furnace experience credit");
            saw_experience_credit |= packet.total_experience == 1;
        }
    }
}

#[tokio::test]
async fn survival_specialized_furnaces_open_vanilla_menu_types() {
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
    let block_state = |name: &str| {
        blocks
            .block(&mc_data::Identifier::parse(name).unwrap())
            .map(|block| block.default.0 as i32)
            .expect("block in registry")
    };
    let smoker_state_id = block_state("minecraft:smoker");
    let blast_furnace_state_id = block_state("minecraft:blast_furnace");
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
    let furnace_y = top_non_air_y(&mut storage, 2, 2, air_state)
        .expect("specialized furnace column terrain")
        + 1;
    let cases = [
        (
            smoker_state_id,
            22,
            mc_world::BlockPos {
                x: 2,
                y: furnace_y,
                z: 2,
            },
            171,
        ),
        (
            blast_furnace_state_id,
            10,
            mc_world::BlockPos {
                x: 3,
                y: furnace_y,
                z: 2,
            },
            172,
        ),
    ];
    for &(state_id, _, pos, _) in &cases {
        storage
            .set_block_at(pos, mc_world::BlockStateId(state_id as u32))
            .expect("seed specialized furnace block")
            .expect("specialized furnace chunk exists");
        storage
            .set_furnace_block_entity(pos, mc_world::FurnaceBlockEntity::default())
            .expect("seed specialized furnace entity");
    }
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let tags = Arc::new(
        mc_data::tags::load(&vanilla_dir, &data)
            .expect("tags load")
            .with_vanilla_fuel_values(&items),
    );
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M71 specialized furnaces".into(),
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
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M71Furnaces").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    for (_, menu_id, pos, sequence) in cases {
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(pos.x, pos.y, pos.z),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence: sequence + 10,
            })
            .await
            .expect("open specialized furnace");
        let opened = wait_for_open_screen(&mut client, menu_id).await;
        client
            .write_packet(&ServerboundContainerClose {
                container_id: opened.container_id,
            })
            .await
            .expect("close specialized furnace");
    }
}
