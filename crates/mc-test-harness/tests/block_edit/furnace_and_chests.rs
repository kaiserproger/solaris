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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
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
            command: "debug give minecraft:oak_log 1 1".into(),
        })
        .await
        .expect("give oak log");
    wait_for_slot_stack(&mut client, oak_log_id, 1).await;

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
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("pick up oak log");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == oak_log_id && pkt.carried_item.count == 1
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
            carried_item: HashedStack::Actual {
                item_id: oak_log_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("place oak log in crafting grid");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == oak_planks_id && pkt.items[0].count == 4
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
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("pick up planks");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == oak_planks_id && pkt.carried_item.count == 4
    })
    .await;
    for grid_slot in [1, 2, 4, 5] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: grid_slot,
                button_num: 1,
                container_input: ContainerInput::Pickup,
                changed_slots: Vec::new(),
                carried_item: HashedStack::Actual {
                    item_id: oak_planks_id,
                    count: content.carried_item.count,
                    components: HashedStackComponentHashes::empty(),
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
            carried_item: HashedStack::empty(),
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
            carried_item: HashedStack::empty(),
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
            carried_item: HashedStack::Actual {
                item_id: raw_iron_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
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
            carried_item: HashedStack::empty(),
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
            carried_item: HashedStack::Actual {
                item_id: coal_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
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
            carried_item: HashedStack::empty(),
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
