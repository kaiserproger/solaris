#[tokio::test]
async fn place_recipe_crafts_torch_from_authoritative_inventory() {
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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let coal_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
        .expect("coal item");
    let stick_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stick").unwrap())
        .expect("stick item");
    let torch_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:torch").unwrap())
        .expect("torch item");
    let loaded_recipes = mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
        .expect("recipes load");
    let Some(torch_recipe) = (if loaded_recipes.is_empty() {
        Some(0)
    } else {
        loaded_recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == "minecraft:torch")
            .and_then(|idx| i32::try_from(idx).ok())
    }) else {
        eprintln!("skipping: missing torch recipe");
        return;
    };

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 crafting".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(loaded_recipes),
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

    let (mut client, _) = connect_to_play(addr, "M23Crafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:coal 1 0".into(),
        })
        .await
        .expect("give coal");
    wait_for_slot_stack(&mut client, coal_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stick 1 1".into(),
        })
        .await
        .expect("give stick");
    wait_for_slot_stack(&mut client, stick_id, 1).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: torch_recipe,
            use_max_items: false,
        })
        .await
        .expect("place torch recipe");

    let mut saw_coal_consumed = false;
    let mut saw_stick_consumed = false;
    let mut saw_torches = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_coal_consumed && saw_stick_consumed && saw_torches) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("craft response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36 && pkt.item_stack.is_empty() {
                saw_coal_consumed = true;
            } else if pkt.slot == 37 && pkt.item_stack.is_empty() {
                saw_stick_consumed = true;
            } else if pkt.item_stack.item_id == torch_id && pkt.item_stack.count == 4 {
                saw_torches = true;
            }
        }
    }
}

#[tokio::test]
async fn place_recipe_crafts_tag_based_planks_sticks_and_table() {
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

    let loaded_recipes = mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
        .expect("recipes load");
    let fallback_recipe_display_id = |id: &str| match id {
        "minecraft:oak_planks" => Some(1),
        "minecraft:stick" => Some(2),
        "minecraft:crafting_table" => Some(3),
        _ => None,
    };
    let recipe_display_id = |id: &str| {
        if loaded_recipes.is_empty() {
            fallback_recipe_display_id(id)
        } else {
            loaded_recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .and_then(|idx| i32::try_from(idx).ok())
        }
    };
    let Some(oak_planks_recipe) = recipe_display_id("minecraft:oak_planks") else {
        eprintln!("skipping: missing oak_planks recipe");
        return;
    };
    let Some(stick_recipe) = recipe_display_id("minecraft:stick") else {
        eprintln!("skipping: missing stick recipe");
        return;
    };
    let Some(crafting_table_recipe) = recipe_display_id("minecraft:crafting_table") else {
        eprintln!("skipping: missing crafting_table recipe");
        return;
    };

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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let item_registry = mc_data::Identifier::parse("minecraft:item").unwrap();
    let oak_logs_tag = mc_data::Identifier::parse("minecraft:oak_logs").unwrap();
    let planks_tag = mc_data::Identifier::parse("minecraft:planks").unwrap();
    if !tags
        .registries
        .get(&item_registry)
        .is_some_and(|item_tags| {
            item_tags.contains_key(&oak_logs_tag) && item_tags.contains_key(&planks_tag)
        })
    {
        eprintln!("skipping: missing item oak_logs/planks tags");
        return;
    }
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let oak_log_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak_log item");
    let oak_planks_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak_planks item");
    let stick_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stick").unwrap())
        .expect("stick item");
    let crafting_table_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .expect("crafting_table item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 tag crafting".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(loaded_recipes.clone()),
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

    let (mut client, _) = connect_to_play(addr, "M23TagCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 0".into(),
        })
        .await
        .expect("give oak_log");
    wait_for_slot_stack(&mut client, oak_log_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("place oak_planks recipe");
    wait_for_slot_stack(&mut client, oak_planks_id, 8).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: crafting_table_recipe,
            use_max_items: false,
        })
        .await
        .expect("place crafting_table recipe");
    wait_for_slot_stack(&mut client, crafting_table_id, 1).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_planks 2 1".into(),
        })
        .await
        .expect("give oak_planks");
    wait_for_slot_stack(&mut client, oak_planks_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: stick_recipe,
            use_max_items: false,
        })
        .await
        .expect("place stick recipe");
    wait_for_slot_stack(&mut client, stick_id, 4).await;
}

#[tokio::test]
async fn crafting_table_container_crafts_shapeless_and_shaped_results() {
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
    let crafting_table_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("crafting_table in registry");
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
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
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
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M24TableCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:crafting_table 1 0".into(),
        })
        .await
        .expect("give crafting table");
    wait_for_slot_stack(&mut client, crafting_table_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 1".into(),
        })
        .await
        .expect("give oak log");
    wait_for_slot_stack(&mut client, oak_log_id, 2).await;

    let support_y = sync.y.floor() as i32 - 2;
    let table_y = support_y + 1;
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
            sequence: 101,
        })
        .await
        .expect("place crafting table");
    wait_for_block_update(&mut client, (0, table_y, 0), crafting_table_state_id).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, table_y, 0),
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
        pkt.carried_item.is_empty() && pkt.items[38].item_id == oak_log_id && pkt.items[38].count == 1
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
    for grid_slot in [1, 2, 4, 5] {
        let carried_after_click = content.carried_item.count - 1;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: grid_slot,
                button_num: 1,
                container_input: ContainerInput::Pickup,
                changed_slots: Vec::new(),
                carried_item: if carried_after_click > 0 {
                    HashedStack::Actual {
                        item_id: oak_planks_id,
                        count: carried_after_click,
                        components: HashedStackComponentHashes::empty(),
                    }
                } else {
                    HashedStack::empty()
                },
            })
            .await
            .expect("place plank in shaped grid");
        content = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    }
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("furnace in registry");
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
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
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

    let (mut actor, sync) = connect_to_play(addr, "M100FurnActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut actor, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
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
            sequence: 191,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut actor, (0, furnace_y, 0), furnace_state_id).await;

    let (mut observer, _) = connect_to_play(addr, "M100FurnObserve").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
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
    let observer_initial = wait_for_furnace_content(
        &mut observer,
        observer_opened.container_id,
        |pkt| pkt.items[0].is_empty() && pkt.carried_item.is_empty(),
    )
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
            position: pack_block_pos(0, furnace_y, 0),
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
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |_| true)
        .await;
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
    let observer_slot = wait_for_container_slot(
        &mut observer,
        observer_opened.container_id,
        0,
        |stack| stack.item_id == raw_iron_id && stack.count == 1,
    )
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
        };
        chest.slots[1] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 63,
            damage: None,
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|block| block.default.0 as i32)
        .expect("furnace in registry");
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
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
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

    let (mut client, sync) = connect_to_play(addr, "M100BadFurn").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut client, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
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
            sequence: 199,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut client, (0, furnace_y, 0), furnace_state_id).await;

    let furnace_pos = mc_world::BlockPos {
        x: 0,
        y: furnace_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut furnace = mc_world::FurnaceBlockEntity::default();
        furnace.slots[0] = mc_world::FurnaceSlot {
            item_id: raw_iron_id,
            count: 3,
            damage: None,
        };
        world
            .set_furnace_block_entity(furnace_pos, furnace)
            .expect("seed furnace entity");
    }

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
        pkt.items[0].item_id == raw_iron_id && pkt.items[0].count == 3 && pkt.carried_item.is_empty()
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

    let mut world = world_handle.lock().await;
    let furnace = world
        .furnace_block_entity(furnace_pos)
        .expect("read furnace entity")
        .expect("furnace entity present");
    assert_eq!(furnace.slots[0].item_id, raw_iron_id);
    assert_eq!(furnace.slots[0].count, 3);
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("furnace in registry");
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
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let coal_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
        .expect("coal item");
    let iron_ingot_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_ingot").unwrap())
        .expect("iron_ingot item");
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

    let (mut client, sync) = connect_to_play(addr, "M23Smelter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut client, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
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
            sequence: 91,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut client, (0, furnace_y, 0), furnace_state_id).await;

    let (mut observer, _) = connect_to_play(addr, "M24FurnaceViewer").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
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
            command: "debug give minecraft:coal 1 1".into(),
        })
        .await
        .expect("give coal");
    wait_for_slot_stack(&mut client, coal_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
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
            position: pack_block_pos(0, furnace_y, 0),
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
                item_id: coal_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up coal");
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == coal_id && pkt.carried_item.count == 1
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
        .expect("place coal fuel");
    wait_for_container_slot(&mut observer, observer_opened.container_id, 1, |stack| {
        stack.item_id == coal_id && stack.count == 1
    })
    .await;

    wait_for_furnace_data(&mut client, opened.container_id, 2, |value| value > 0).await;
    wait_for_furnace_data(&mut observer, observer_opened.container_id, 2, |value| {
        value > 0
    })
    .await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[2].item_id == iron_ingot_id && pkt.items[2].count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("shift-click result");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[2].is_empty()
            && pkt
                .items
                .iter()
                .skip(3)
                .any(|stack| stack.item_id == iron_ingot_id && stack.count == 1)
    })
    .await;
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

    let (mut client, sync) = connect_to_play(addr, "M71Furnaces").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let furnace_y = sync.y.floor() as i32 - 1;
    let cases = [
        (
            smoker_state_id,
            22,
            mc_world::BlockPos {
                x: 1,
                y: furnace_y,
                z: 0,
            },
            171,
        ),
        (
            blast_furnace_state_id,
            10,
            mc_world::BlockPos {
                x: 2,
                y: furnace_y,
                z: 0,
            },
            172,
        ),
    ];
    {
        let mut storage = world_handle.lock().await;
        for &(state_id, _, pos, _) in &cases {
            storage
                .set_block_at(pos, mc_world::BlockStateId(state_id as u32))
                .expect("seed specialized furnace block")
                .expect("generated spawn chunk exists");
        }
    }

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

#[tokio::test]
async fn survival_container_click_moves_stack_through_server_cursor() {
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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 container click".into(),
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

    let (mut client, _) = connect_to_play(addr, "M23Clicker").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 10 0".into(),
        })
        .await
        .expect("give dirt");
    let slot = wait_for_slot_stack_update(&mut client, dirt_id, 10).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: slot.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 10,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let content = wait_for_inventory_content(&mut client, |pkt| {
        pkt.items[36].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 10
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: content.state_id,
            slot_num: 37,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place dirt stack");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty()
            && pkt.items[36].is_empty()
            && pkt.items[37].item_id == dirt_id
            && pkt.items[37].count == 10
    })
    .await;
}

#[tokio::test]
async fn malformed_inventory_click_resyncs_without_advancing_state() {
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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed inventory click".into(),
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

    let (mut client, _) = connect_to_play(addr, "M100BadInv").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 10 0".into(),
        })
        .await
        .expect("give dirt");
    let slot = wait_for_slot_stack_update(&mut client, dirt_id, 10).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: slot.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(36, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send pickup with impossible carried item");
    let resync = wait_for_inventory_content(&mut client, |pkt| {
        pkt.items[36].item_id == dirt_id && pkt.items[36].count == 10 && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(
        resync.state_id, slot.state_id,
        "malformed inventory click should resync without advancing inventory state"
    );
    assert!(
        resync
            .items
            .iter()
            .enumerate()
            .filter(|(slot, _)| *slot != 36)
            .all(|(_, stack)| stack.item_id != dirt_id),
        "malformed inventory click must not move dirt into another inventory slot"
    );
}

#[tokio::test]
async fn unsupported_station_use_is_safe_noop_instead_of_block_placement() {
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
    let stonecutter_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:stonecutter").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("stonecutter in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("dirt in registry");
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
    let stonecutter_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stonecutter").unwrap())
        .expect("stonecutter item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M87 station no-op".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M87StationNoop").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stonecutter 1 0".into(),
        })
        .await
        .expect("give stonecutter");
    wait_for_slot_stack(&mut client, stonecutter_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let station_y = support_y + 1;
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
            sequence: 701,
        })
        .await
        .expect("place stonecutter");
    wait_for_block_update(&mut client, (0, station_y, 0), stonecutter_state_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, station_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 702,
        })
        .await
        .expect("use unsupported station");
    read_station_noop_ack(&mut client, 702, (0, station_y + 1, 0), dirt_state_id).await;
}

async fn read_station_noop_ack(
    client: &mut Client,
    sequence: i32,
    fallthrough_pos: (i32, i32, i32),
    fallthrough_state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("station no-op ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            panic!("unsupported station must not open an unimplemented menu");
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            assert!(
                unpack_block_pos(pkt.position) != fallthrough_pos
                    || pkt.state_id != fallthrough_state_id,
                "unsupported station use fell through into adjacent block placement"
            );
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}
