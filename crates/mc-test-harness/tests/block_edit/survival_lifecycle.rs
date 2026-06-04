#[tokio::test]
async fn survival_double_chest_opens_combined_storage_and_mutates_second_half() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");

    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_item_id = items.id_of(&chest_id).expect("chest item");

    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M28 double chest".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M28DoubleChest").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 2 0".into(),
        })
        .await
        .expect("give chests");
    wait_for_slot_stack(&mut client, chest_item_id, 2).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    let left_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 1,
    };
    let right_pos = mc_world::BlockPos {
        x: 1,
        y: chest_y,
        z: 1,
    };
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(left_pos.x, support_y, left_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 89,
        })
        .await
        .expect("place left chest");
    wait_for_block_update(
        &mut client,
        (left_pos.x, left_pos.y, left_pos.z),
        chest_state_id,
    )
    .await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(right_pos.x, support_y, right_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 90,
        })
        .await
        .expect("place right chest");
    wait_for_block_update(
        &mut client,
        (right_pos.x, right_pos.y, right_pos.z),
        chest_state_id,
    )
    .await;

    {
        let mut world = world_handle.lock().await;
        let mut left_chest = mc_world::ChestBlockEntity::default();
        left_chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 5,
            damage: None,
        };
        let mut right_chest = mc_world::ChestBlockEntity::default();
        right_chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 7,
            damage: None,
        };
        world
            .set_chest_block_entity(left_pos, left_chest)
            .expect("seed left chest entity");
        world
            .set_chest_block_entity(right_pos, right_chest)
            .expect("seed right chest entity");
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(left_pos.x, left_pos.y, left_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 91,
        })
        .await
        .expect("open double chest");

    let opened = wait_for_open_screen(&mut client, 5).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items.len() == 90
            && pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 5
            && pkt.items[27].item_id == dirt_id
            && pkt.items[27].count == 7
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 27,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("take from second chest half");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[27].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 7
    })
    .await;

    let mut world = world_handle.lock().await;
    let left = world
        .chest_block_entity(left_pos)
        .expect("left chest read")
        .expect("left chest present");
    let right = world
        .chest_block_entity(right_pos)
        .expect("right chest read")
        .expect("right chest present");
    assert_eq!(left.slots[0].count, 5);
    assert!(right.slots[0].is_empty());
}

#[tokio::test]
async fn survival_armor_slot_reduces_debug_damage() {
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
    let chestplate_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap())
        .expect("iron chestplate item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 armor".into(),
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

    let (mut client, _) = connect_to_play(addr, "M23Armored").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:iron_chestplate 1 0".into(),
        })
        .await
        .expect("give chestplate");
    let slot = wait_for_slot_stack_update(&mut client, chestplate_id, 1).await;
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
        .expect("pick up chestplate");
    let content = wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.item_id == chestplate_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: content.state_id,
            slot_num: 6,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: chestplate_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("equip chestplate");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty() && pkt.items[6].item_id == chestplate_id
    })
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 10".into(),
    })
    .await
    .expect("damage armored player");
    wait_for_health_near(&mut client, 10.48, 0.02).await;
    wait_for_slot_damage(&mut client, 6, chestplate_id, 2).await;
}

#[tokio::test]
async fn survival_use_item_eats_apple_and_updates_food() {
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
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("apple item");
    let apple = mc_data::Identifier::parse("minecraft:apple").unwrap();
    let item_facts = Arc::new(
        mc_data::item_components::load_item_facts(
            vanilla_dir.join("reports/minecraft/components/item"),
        )
        .expect("item facts load"),
    );
    assert!(
        item_facts
            .get(&apple)
            .and_then(|facts| facts.food)
            .is_some(),
        "apple food must come from item component reports"
    );

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M22 food use".into(),
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
        item_facts,
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

    let (mut client, _) = connect_to_play(addr, "M22AppleEater").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival exhaust 28".into(),
        })
        .await
        .expect("exhaust hunger");
    wait_for_food_level(&mut client, 18).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:apple 2 0".into(),
        })
        .await
        .expect("give apple");
    wait_for_slot_stack(&mut client, apple_id, 2).await;

    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 41,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("eat apple");
    read_ack_without_food_or_slot_change(&mut client, 41, apple_id).await;

    let mut saw_decrement = false;
    let mut saw_food = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_decrement && saw_food) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("eat response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.item_stack.item_id == apple_id && pkt.item_stack.count == 1 {
                saw_decrement = true;
            }
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode set health");
            if pkt.food == 20 && pkt.saturation > 0.0 {
                saw_food = true;
            }
        }
    }
}

#[tokio::test]
async fn survival_use_item_release_cancels_food_use() {
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
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("apple item");
    let item_facts = Arc::new(
        mc_data::item_components::load_item_facts(
            vanilla_dir.join("reports/minecraft/components/item"),
        )
        .expect("item facts load"),
    );

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M24 food cancel".into(),
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
        item_facts,
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

    let (mut client, _) = connect_to_play(addr, "M24FoodCancel").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival exhaust 28".into(),
        })
        .await
        .expect("exhaust hunger");
    wait_for_food_level(&mut client, 18).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:apple 2 0".into(),
        })
        .await
        .expect("give apple");
    wait_for_slot_stack(&mut client, apple_id, 2).await;

    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 81,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("start eating apple");
    read_ack_without_food_or_slot_change(&mut client, 81, apple_id).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::ReleaseUseItem,
            position: 0,
            direction: Direction::Down,
            sequence: 82,
        })
        .await
        .expect("release use item");
    read_ack_without_food_or_slot_change(&mut client, 82, apple_id).await;
    assert_no_food_or_slot_change(&mut client, apple_id, Duration::from_millis(1_800)).await;
}

#[tokio::test]
async fn survival_bow_release_spawns_and_moves_arrow() {
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
    let bow_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:bow").unwrap())
        .expect("bow item");
    let arrow_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .expect("arrow item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let arrow_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("arrow entity type");
    let target_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:armor_stand").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("armor stand entity type");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M54 bow arrow lifecycle".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M54BowArrow").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bow 1 0".into(),
        })
        .await
        .expect("give bow");
    wait_for_slot_stack(&mut client, bow_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:arrow 3 1".into(),
        })
        .await
        .expect("give arrows");
    wait_for_slot_stack(&mut client, arrow_id, 3).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: format!(
                "summon minecraft:armor_stand {} {} {}",
                sync.x,
                sync.y,
                sync.z + 0.8
            ),
        })
        .await
        .expect("summon projectile target");
    let _target_entity_id = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let frame = client
                .read_frame_with_timeout(
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                )
                .await
                .expect("cow summon response");
            if handle_keepalive(&mut client, frame.id, &frame.body).await {
                continue;
            }
            if frame.id == AddEntity::ID {
                let mut body = frame.body;
                let pkt = AddEntity::decode(&mut body).expect("decode target AddEntity");
                if pkt.entity_type_id == target_entity_type {
                    break pkt.entity_id;
                }
            }
        }
    };

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: sync.x,
            y: sync.y,
            z: sync.z,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("aim bow");
    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 91,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("start drawing bow");
    wait_for_block_ack(&mut client, 91).await;
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::ReleaseUseItem,
            position: 0,
            direction: Direction::Down,
            sequence: 92,
        })
        .await
        .expect("release bow");

    let mut arrow_entity_id = None;
    let mut saw_arrow_decrement = false;
    let mut saw_release_ack = false;
    let mut saw_initial_motion = false;
    let mut saw_relative_move = false;
    let mut saw_arrow_despawn = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_arrow_decrement
        && saw_release_ack
        && saw_initial_motion
        && saw_relative_move
        && saw_arrow_despawn)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "bow arrow lifecycle response: {err}; decrement={saw_arrow_decrement} ack={saw_release_ack} motion={saw_initial_motion} move={saw_relative_move} despawn={saw_arrow_despawn} arrow={arrow_entity_id:?}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode arrow AddEntity");
            if pkt.entity_type_id == arrow_entity_type {
                saw_initial_motion |= pkt.movement.x != 0.0
                    || pkt.movement.y != 0.0
                    || pkt.movement.z != 0.0;
                arrow_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let pkt = SetEntityMotion::decode(&mut body).expect("decode arrow motion");
            if Some(pkt.entity_id) == arrow_entity_id {
                saw_initial_motion |= pkt.movement.x != 0.0
                    || pkt.movement.y != 0.0
                    || pkt.movement.z != 0.0;
            }
        } else if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let pkt = MoveEntityPosRot::decode(&mut body).expect("decode arrow relative move");
            if Some(pkt.entity_id) == arrow_entity_id {
                saw_relative_move |= pkt.delta_x != 0 || pkt.delta_y != 0 || pkt.delta_z != 0;
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode arrow despawn");
            saw_arrow_despawn |= arrow_entity_id.is_some_and(|id| pkt.entity_ids.contains(&id));
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode arrow slot");
            saw_arrow_decrement |= pkt.item_stack.item_id == arrow_id && pkt.item_stack.count == 2;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode release ack");
            saw_release_ack |= pkt.sequence == 92;
        }
    }
}

#[tokio::test]
async fn dead_survival_player_cannot_mine_or_eat() {
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
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("apple item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("item entity type");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M22 dead survival guard".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M22DeadGuard").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival exhaust 28".into(),
        })
        .await
        .expect("exhaust hunger");
    wait_for_food_level(&mut client, 18).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:apple 2 0".into(),
        })
        .await
        .expect("give apple");
    wait_for_slot_stack(&mut client, apple_id, 2).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 100".into(),
        })
        .await
        .expect("kill player");
    wait_for_health_level(&mut client, 0.0).await;
    wait_for_death_inventory_drop(&mut client, item_entity_type, apple_id, 2).await;

    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 71,
        })
        .await
        .expect("dead start break");
    read_ack_without_target_update(&mut client, 71, (0, target_y, 0)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 72,
        })
        .await
        .expect("dead stop break");
    read_ack_without_target_update(&mut client, 72, (0, target_y, 0)).await;

    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 73,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("dead eat apple");
    read_ack_without_food_or_slot_change(&mut client, 73, apple_id).await;
}

#[tokio::test]
async fn dead_survival_player_can_respawn_and_act_again() {
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
    let apple_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .expect("apple item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 respawn".into(),
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

    let (mut client, _) = connect_to_play(addr, "M23Respawn").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:apple 1 0".into(),
        })
        .await
        .expect("give apple");
    wait_for_slot_stack(&mut client, apple_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 100".into(),
        })
        .await
        .expect("kill player");
    wait_for_health_level(&mut client, 0.0).await;

    client
        .write_packet(&ServerboundClientCommand {
            action: ClientCommandAction::PerformRespawn,
        })
        .await
        .expect("request respawn");
    let mut saw_respawn = false;
    let mut saw_full_health = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_respawn && saw_full_health) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("respawn response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundRespawn::ID {
            let mut body = frame.body;
            let pkt = ClientboundRespawn::decode(&mut body).expect("decode Respawn");
            assert_eq!(pkt.game_mode, 0);
            saw_respawn = true;
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if (pkt.health - 20.0).abs() < f32::EPSILON && pkt.food == 20 {
                saw_full_health = true;
            }
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:apple 1 0".into(),
        })
        .await
        .expect("give post-respawn apple");
    wait_for_slot_stack(&mut client, apple_id, 1).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival exhaust 28".into(),
        })
        .await
        .expect("exhaust after respawn");
    wait_for_food_level(&mut client, 18).await;
    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 81,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("eat after respawn");

    let mut saw_consume = false;
    let mut saw_food = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_consume && saw_food) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("post-respawn eat response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.slot == 36 && pkt.item_stack.is_empty() {
                saw_consume = true;
            }
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if pkt.food == 20 {
                saw_food = true;
            }
        }
    }
}
