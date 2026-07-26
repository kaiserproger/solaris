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

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_planks 4 2".into(),
        })
        .await
        .expect("give planks for manual quick craft");
    let planks_update = wait_for_slot_stack_update(&mut client, oak_planks_id, 4).await;
    let hashed_planks = |count| HashedStack::Actual {
        item_id: oak_planks_id,
        count,
        components: HashedStackComponentHashes::empty(),
    };
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: planks_update.state_id,
            slot_num: planks_update.slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(planks_update.slot, HashedStack::Empty)],
            carried_item: hashed_planks(4),
        })
        .await
        .expect("pick up planks for manual quick craft");
    let picked_up = wait_for_inventory_content(&mut client, |packet| {
        packet.carried_item.item_id == oak_planks_id && packet.carried_item.count == 4
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: picked_up.state_id,
            slot_num: -999,
            button_num: 4,
            container_input: ContainerInput::QuickCraft,
            changed_slots: Vec::new(),
            carried_item: hashed_planks(4),
        })
        .await
        .expect("start inventory quick craft");
    for slot in 1..=4 {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: picked_up.state_id,
                slot_num: slot,
                button_num: 5,
                container_input: ContainerInput::QuickCraft,
                changed_slots: Vec::new(),
                carried_item: hashed_planks(4),
            })
            .await
            .expect("add inventory quick craft slot");
    }
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: picked_up.state_id,
            slot_num: -999,
            button_num: 6,
            container_input: ContainerInput::QuickCraft,
            changed_slots: (1..=4).map(|slot| (slot, hashed_planks(1))).collect(),
            carried_item: HashedStack::Empty,
        })
        .await
        .expect("finish inventory quick craft");
    wait_for_inventory_content(&mut client, |packet| {
        packet.items[0].item_id == crafting_table_id
            && packet.items[0].count == 1
            && (1..=4).all(|slot| {
                packet.items[slot].item_id == oak_planks_id && packet.items[slot].count == 1
            })
            && packet.carried_item.is_empty()
    })
    .await;
}

#[tokio::test]
async fn inventory_recipe_rejects_three_by_three_tool_without_crafting_table() {
    let data = embedded_play_data();
    let wooden_pickaxe_recipe = embedded_recipe_display_id(&data, "minecraft:wooden_pickaxe");
    let oak_planks_id = embedded_item_id(&data, "minecraft:oak_planks");
    let stick_id = embedded_item_id(&data, "minecraft:stick");
    let wooden_pickaxe_id = embedded_item_id(&data, "minecraft:wooden_pickaxe");

    let cfg = embedded_playable_config(
        &data,
        embedded_world(&data),
        "P2 embedded inventory recipe guard",
    );
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "P2RecipeGuard").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_planks 3 0".into(),
        })
        .await
        .expect("give planks");
    wait_for_slot_stack(&mut client, oak_planks_id, 3).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stick 2 1".into(),
        })
        .await
        .expect("give sticks");
    wait_for_slot_stack(&mut client, stick_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: wooden_pickaxe_recipe,
            use_max_items: false,
        })
        .await
        .expect("place wooden_pickaxe inventory recipe");
    assert_no_slot_stack_for(&mut client, wooden_pickaxe_id).await;
}
