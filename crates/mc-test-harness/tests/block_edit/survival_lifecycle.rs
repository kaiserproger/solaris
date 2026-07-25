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
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state = blocks.block(&chest_id).expect("chest block").default;

    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .expect("stone block")
        .default;
    let support_y = top_non_air_y(&mut storage, 0, 0, air).expect("spawn surface");
    let chest_y = support_y + 1;
    let left_pos = mc_world::BlockPos {
        x: 2,
        y: chest_y,
        z: 1,
    };
    let right_pos = mc_world::BlockPos {
        x: 3,
        y: chest_y,
        z: 1,
    };
    for x in [left_pos.x, right_pos.x] {
        storage
            .set_block_at(mc_world::BlockPos { x, y: support_y, z: 1 }, stone)
            .expect("seed interior chest support")
            .expect("interior chest support chunk is loaded");
    }
    let mut left_chest = mc_world::ChestBlockEntity::default();
    left_chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 5,
        damage: None,
        enchantments: Vec::new(),
    };
    let mut right_chest = mc_world::ChestBlockEntity::default();
    right_chest.slots[0] = mc_world::FurnaceSlot {
        item_id: dirt_id,
        count: 7,
        damage: None,
        enchantments: Vec::new(),
    };
    for (position, chest) in [(left_pos, left_chest), (right_pos, right_chest)] {
        storage
            .set_block_at(position, chest_state)
            .expect("seed double chest block")
            .expect("double chest chunk is loaded");
        storage
            .set_chest_block_entity(position, chest)
            .expect("seed double chest entity");
    }

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

    let (mut client, _sync) = connect_to_play(addr, "M28DoubleChest").await;
    drain_until_chunk(&mut client, (0, 0)).await;

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
async fn survival_generic_damage_bypasses_armor_and_durability() {
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
        item_facts: Arc::new(
            mc_data::item_components::load_item_facts(
                vanilla_dir.join("reports/minecraft/components/item"),
            )
            .expect("item facts load"),
        ),
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
            carried_item: HashedStack::Actual {
                item_id: chestplate_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
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
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("equip chestplate");
    let equipped = wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty() && pkt.items[6].item_id == chestplate_id
    })
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 10".into(),
        })
        .await
        .expect("damage armored player");
    wait_for_health_near(&mut client, 10.0, 0.02).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: equipped.state_id,
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
        .expect("inspect chestplate after generic damage");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.item_id == chestplate_id
            && pkt.carried_item.count == 1
            && pkt.carried_item.damage.unwrap_or_default() == 0
    })
    .await;
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

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::SwapItemWithOffhand,
            position: 0,
            direction: Direction::Down,
            sequence: 42,
        })
        .await
        .expect("move remaining apple to offhand");
    assert_offhand_swap_before_ack(&mut client, 42, apple_id, true).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival exhaust 20".into(),
        })
        .await
        .expect("exhaust hunger before offhand use");
    wait_for_food_level(&mut client, 18).await;
    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 43,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("eat apple from offhand");
    read_ack_without_food_or_slot_change(&mut client, 43, apple_id).await;

    let mut saw_offhand_empty = false;
    let mut saw_offhand_food = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_offhand_empty && saw_offhand_food) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("offhand eat response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode offhand food SetSlot");
            saw_offhand_empty |=
                packet.container_id == 0 && packet.slot == 45 && packet.item_stack.is_empty();
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode offhand food health");
            saw_offhand_food |= packet.food == 20 && packet.saturation > 0.0;
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
    assert_no_food_or_slot_change_until_world_ticks(&mut client, apple_id, 40).await;
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
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );
    let arrow_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("arrow entity type");
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
    wait_for_world_ticks(&mut client, 20).await;
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
    let mut saw_bow_damage = false;
    let mut saw_release_ack = false;
    let mut saw_initial_motion = false;
    let mut saw_relative_move = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_arrow_decrement
        && saw_bow_damage
        && saw_release_ack
        && saw_initial_motion
        && saw_relative_move)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "bow arrow lifecycle response: {err}; decrement={saw_arrow_decrement} bow_damage={saw_bow_damage} ack={saw_release_ack} motion={saw_initial_motion} move={saw_relative_move} arrow={arrow_entity_id:?}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode arrow AddEntity");
            if pkt.entity_type_id == arrow_entity_type {
                saw_initial_motion |=
                    pkt.movement.x != 0.0 || pkt.movement.y != 0.0 || pkt.movement.z != 0.0;
                arrow_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let pkt = SetEntityMotion::decode(&mut body).expect("decode arrow motion");
            if Some(pkt.entity_id) == arrow_entity_id {
                saw_initial_motion |=
                    pkt.movement.x != 0.0 || pkt.movement.y != 0.0 || pkt.movement.z != 0.0;
            }
        } else if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let pkt = MoveEntityPosRot::decode(&mut body).expect("decode arrow relative move");
            if Some(pkt.entity_id) == arrow_entity_id {
                saw_relative_move |= pkt.delta_x != 0 || pkt.delta_y != 0 || pkt.delta_z != 0;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundContainerSetSlot::decode(&mut body).expect("decode inventory slot");
            saw_arrow_decrement |= pkt.item_stack.item_id == arrow_id && pkt.item_stack.count == 2;
            saw_bow_damage |= pkt.item_stack.item_id == bow_id
                && pkt.item_stack.count == 1
                && pkt.item_stack.damage == Some(1);
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode release ack");
            saw_release_ack |= pkt.sequence == 92;
        }
    }

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::SwapItemWithOffhand,
            position: 0,
            direction: Direction::Down,
            sequence: 93,
        })
        .await
        .expect("move bow to offhand");
    assert_offhand_swap_before_ack(&mut client, 93, bow_id, true).await;

    client
        .write_packet(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 94,
            y_rot: 0.0,
            x_rot: 0.0,
        })
        .await
        .expect("start drawing bow from offhand");
    wait_for_block_ack(&mut client, 94).await;
    wait_for_world_ticks(&mut client, 20).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::ReleaseUseItem,
            position: 0,
            direction: Direction::Down,
            sequence: 95,
        })
        .await
        .expect("release offhand bow");

    let mut saw_offhand_arrow = false;
    let mut saw_second_arrow_decrement = false;
    let mut saw_offhand_bow_damage = false;
    let mut saw_offhand_release_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_offhand_arrow
        && saw_second_arrow_decrement
        && saw_offhand_bow_damage
        && saw_offhand_release_ack)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "offhand bow release response: {error}; arrow={saw_offhand_arrow} decrement={saw_second_arrow_decrement} bow_damage={saw_offhand_bow_damage} ack={saw_offhand_release_ack}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode offhand arrow AddEntity");
            saw_offhand_arrow |= packet.entity_type_id == arrow_entity_type;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode offhand bow inventory slot");
            saw_second_arrow_decrement |= packet.slot == 37
                && packet.item_stack.item_id == arrow_id
                && packet.item_stack.count == 1;
            saw_offhand_bow_damage |= packet.slot == 45
                && packet.item_stack.item_id == bow_id
                && packet.item_stack.count == 1
                && packet.item_stack.damage == Some(2);
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode offhand release ack");
            saw_offhand_release_ack |= packet.sequence == 95;
        }
    }
}

#[tokio::test]
async fn selected_item_drop_debits_slot_and_spawns_exact_stack() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let item_id = data
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log item");
    let item_entity_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("item entity type");
    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "Prompt 03B selected item drop");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "AtomicDropWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:birch_log 2 0".into(),
        })
        .await
        .expect("give selected drop fixture");
    wait_for_slot_stack(&mut client, item_id, 2).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::DropItem,
            position: 0,
            direction: Direction::Down,
            sequence: 93,
        })
        .await
        .expect("drop selected item");

    let mut item_entities = HashSet::new();
    let mut saw_drop_stack = false;
    let mut saw_slot_debit = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_drop_stack && saw_slot_debit && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "selected item drop response: {error}; stack={saw_drop_stack} debit={saw_slot_debit} ack={saw_ack}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode selected drop AddEntity");
            if packet.entity_type_id == item_entity_type {
                item_entities.insert(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode selected drop entity data");
            if item_entities.contains(&packet.entity_id) {
                saw_drop_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode selected drop SetSlot");
            saw_slot_debit |= packet.slot == 36
                && packet.item_stack.item_id == item_id
                && packet.item_stack.count == 1;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode selected drop ack");
            saw_ack |= packet.sequence == 93;
        }
    }

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("selected item drop server shutdown")
        .expect("selected item drop server join")
        .expect("selected item drop server serve");
}

async fn drop_one_selected_item_wire(
    client: &mut Client,
    item_entity_type: i32,
    item_id: u32,
    remaining_count: i32,
    sequence: i32,
) -> i32 {
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::DropItem,
            position: 0,
            direction: Direction::Down,
            sequence,
        })
        .await
        .expect("drop selected merge fixture");
    let mut entity_ids = HashSet::new();
    let mut dropped_id = None;
    let mut saw_stack = false;
    let mut saw_slot = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_stack && saw_slot && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("selected merge drop response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode merge drop AddEntity");
            if packet.entity_type_id == item_entity_type {
                entity_ids.insert(packet.entity_id);
                dropped_id = Some(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode merge drop entity data");
            if entity_ids.contains(&packet.entity_id) {
                saw_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode merge drop SetSlot");
            saw_slot |= packet.slot == 36
                && if remaining_count == 0 {
                    packet.item_stack.is_empty()
                } else {
                    packet.item_stack.item_id == item_id
                        && packet.item_stack.count == remaining_count
                };
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode merge drop ack");
            saw_ack |= packet.sequence == sequence;
        }
    }
    dropped_id.expect("selected merge drop entity id")
}

#[tokio::test]
async fn nearby_selected_item_drops_merge_and_publish_survivor() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let item_id = data
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log item");
    let item_entity_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("item entity type");
    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "T04 dropped item merge");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind dropped item merge");
    let addr = bound.local_addr().expect("dropped item merge local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "ItemMergeWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:birch_log 2 0".into(),
        })
        .await
        .expect("give dropped item merge fixture");
    wait_for_slot_stack(&mut client, item_id, 2).await;

    let first_id =
        drop_one_selected_item_wire(&mut client, item_entity_type, item_id, 1, 101).await;
    let second_id =
        drop_one_selected_item_wire(&mut client, item_entity_type, item_id, 0, 102).await;
    assert!(first_id < second_id, "first dropped identity must be older/lower");

    let mut saw_survivor = false;
    let mut saw_consumed_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_survivor && saw_consumed_remove) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "dropped item merge wire response: {error}; survivor={saw_survivor} remove={saw_consumed_remove}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode merged item entity data");
            if packet.entity_id == first_id {
                saw_survivor |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == 2
                    )
                });
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode merged item removal");
            saw_consumed_remove |= packet.entity_ids.contains(&second_id);
        }
    }

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("dropped item merge server shutdown")
        .expect("dropped item merge server join")
        .expect("dropped item merge server serve");
}

async fn assert_offhand_swap_before_ack(
    client: &mut Client,
    sequence: i32,
    item_id: u32,
    item_moves_to_offhand: bool,
) {
    let item_slot = if item_moves_to_offhand { 45 } else { 36 };
    let empty_slot = if item_moves_to_offhand { 36 } else { 45 };
    let mut saw_item = false;
    let mut saw_empty = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("offhand swap response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet =
                ClientboundContainerSetSlot::decode(&mut body).expect("decode swap SetSlot");
            saw_item |= packet.container_id == 0
                && packet.slot == item_slot
                && packet.item_stack.item_id == item_id
                && packet.item_stack.count == 1;
            saw_empty |= packet.container_id == 0
                && packet.slot == empty_slot
                && packet.item_stack.is_empty();
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode swap ack");
            if packet.sequence == sequence {
                assert!(
                    saw_item,
                    "offhand swap must send the moved stack before ack"
                );
                assert!(saw_empty, "offhand swap must clear the old slot before ack");
                return;
            }
        }
    }
}

#[tokio::test]
async fn offhand_swap_updates_both_slots_and_owner_inventory() {
    let data = embedded_play_data();
    let world = embedded_world(&data);
    let item_id = data
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log item");
    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "offhand swap");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "OffhandSwapWire").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:birch_log 1 0".into(),
        })
        .await
        .expect("give offhand swap fixture");
    wait_for_slot_stack(&mut client, item_id, 1).await;

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::SwapItemWithOffhand,
            position: 0,
            direction: Direction::Down,
            sequence: 94,
        })
        .await
        .expect("swap item into offhand");
    assert_offhand_swap_before_ack(&mut client, 94, item_id, true).await;

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::SwapItemWithOffhand,
            position: 0,
            direction: Direction::Down,
            sequence: 95,
        })
        .await
        .expect("swap item back to main hand");
    assert_offhand_swap_before_ack(&mut client, 95, item_id, false).await;

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("offhand swap server shutdown")
        .expect("offhand swap server join")
        .expect("offhand swap server serve");
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
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );
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
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

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

    let (mut client, sync) = connect_to_play(addr, "M23Respawn").await;
    drain_complete_spawn_view(&mut client).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 100".into(),
        })
        .await
        .expect("kill player");
    wait_for_health_level(&mut client, 0.0).await;
    let death_inventory = wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0 && pkt.items.iter().all(mc_protocol::packets::play::ItemStack::is_empty)
    })
    .await;
    assert!(death_inventory.items.iter().all(mc_protocol::packets::play::ItemStack::is_empty));

    client
        .write_packet(&ServerboundClientCommand {
            action: ClientCommandAction::PerformRespawn,
        })
        .await
        .expect("request respawn");
    let mut saw_respawn = false;
    let mut saw_center_chunk = false;
    let mut saw_respawn_chunk = false;
    let mut saw_load_start = false;
    let mut saw_full_health = false;
    let mut saw_position_sync = false;
    let mut saw_abilities = false;
    let mut saw_default_spawn = false;
    let mut saw_inventory_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_respawn
        && saw_center_chunk
        && saw_respawn_chunk
        && saw_load_start
        && saw_full_health
        && saw_position_sync
        && saw_abilities
        && saw_default_spawn
        && saw_inventory_resync)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "respawn response timed out: respawn={saw_respawn} center={saw_center_chunk} chunk={saw_respawn_chunk} load_start={saw_load_start} health={saw_full_health} position_sync={saw_position_sync} abilities={saw_abilities} default_spawn={saw_default_spawn} inventory={saw_inventory_resync}: {error}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundRespawn::ID {
            let mut body = frame.body;
            let pkt = ClientboundRespawn::decode(&mut body).expect("decode Respawn");
            assert_eq!(pkt.game_mode, 0);
            saw_respawn = true;
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode respawn SynchronizePlayerPosition");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: pkt.teleport_id,
                })
                .await
                .expect("confirm respawn teleport");
            saw_position_sync = true;
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if (pkt.health - 20.0).abs() < f32::EPSILON && pkt.food == 20 {
                saw_full_health = true;
            }
        } else if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let pkt = SetCenterChunk::decode(&mut body).expect("decode SetCenterChunk");
            if pkt.chunk_x == 0 && pkt.chunk_z == 0 {
                saw_center_chunk = true;
            }
        } else if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let _ = LevelChunkWithLight::decode(&mut body).expect("decode respawn chunk");
            saw_respawn_chunk = true;
        } else if frame.id == GameEvent::ID {
            let mut body = frame.body;
            let pkt = GameEvent::decode(&mut body).expect("decode respawn GameEvent");
            if pkt.event == GameEvent::EVENT_START_WAITING_FOR_CHUNKS {
                saw_load_start = true;
            }
        } else if frame.id == mc_protocol::packets::play::ClientboundPlayerAbilities::ID {
            let mut body = frame.body;
            let pkt = mc_protocol::packets::play::ClientboundPlayerAbilities::decode(&mut body)
                .expect("decode respawn PlayerAbilities");
            assert!(!pkt.invulnerable);
            assert!(!pkt.flying);
            assert!(!pkt.can_fly);
            assert!(!pkt.instabuild);
            saw_abilities = true;
        } else if frame.id == mc_protocol::packets::play::SetDefaultSpawnPosition::ID {
            let mut body = frame.body;
            let pkt = mc_protocol::packets::play::SetDefaultSpawnPosition::decode(&mut body)
                .expect("decode respawn SetDefaultSpawnPosition");
            assert_eq!(pkt.dimension.as_str(), "minecraft:overworld");
            assert_eq!(
                unpack_block_pos(pkt.position),
                (
                    sync.x.floor() as i32,
                    sync.y.floor() as i32,
                    sync.z.floor() as i32,
                )
            );
            saw_default_spawn = true;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode respawn inventory resync");
            if pkt.container_id == 0 && pkt.items.iter().all(mc_protocol::packets::play::ItemStack::is_empty) {
                saw_inventory_resync = true;
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
            .unwrap_or_else(|error| {
                panic!(
                    "post-respawn eat response timed out: consume={saw_consume} food={saw_food}: {error}"
                )
            });
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

#[tokio::test]
async fn respawned_survival_player_rejoins_alive_after_saved_restart() {
    let data = embedded_play_data();
    let dirt_id = embedded_item_id(&data, "minecraft:dirt");
    let world_dir = tempfile::tempdir().expect("create respawn persistence world");

    let first_shutdown = mc_net::ShutdownHandle::default();
    let mut first_cfg = embedded_playable_config(
        &data,
        embedded_disk_world(&data, world_dir.path()),
        "T02 respawn persistence",
    );
    first_cfg.shutdown = first_shutdown.clone();
    let first_bound = mc_net::bind(first_cfg).await.expect("bind first respawn server");
    let first_addr = first_bound.local_addr().expect("first local_addr");
    let first_serve = tokio::spawn(async move { first_bound.serve_and_save().await });

    let (mut client, _) = connect_to_play(first_addr, "T02Respawn").await;
    wait_for_inventory_content(&mut client, |pkt| pkt.container_id == 0).await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give pre-death dirt");
    wait_for_slot_stack(&mut client, dirt_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 100".into(),
        })
        .await
        .expect("kill persisted player");
    wait_for_health_level(&mut client, 0.0).await;
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt
                .items
                .iter()
                .all(mc_protocol::packets::play::ItemStack::is_empty)
    })
    .await;

    client
        .write_packet(&ServerboundClientCommand {
            action: ClientCommandAction::PerformRespawn,
        })
        .await
        .expect("request persisted respawn");
    let mut saw_full_health = false;
    let mut saw_position_sync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_full_health && saw_position_sync) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("complete respawn before persistence save");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode persisted respawn position");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: pkt.teleport_id,
                })
                .await
                .expect("confirm persisted respawn teleport");
            saw_position_sync = true;
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body)
                .expect("decode persisted respawn health");
            if (pkt.health - 20.0).abs() < f32::EPSILON && pkt.food == 20 {
                saw_full_health = true;
            }
        }
    }
    client
        .write_packet(&mc_protocol::packets::play::ServerboundPlayerLoaded)
        .await
        .expect("acknowledge respawn load");

    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("save respawned player");
    wait_for_save_all_feedback(&mut client).await;

    drop(client);
    first_shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), first_serve)
        .await
        .expect("first respawn server shutdown")
        .expect("first respawn server join")
        .expect("first respawn server serve");

    let second_shutdown = mc_net::ShutdownHandle::default();
    let mut second_cfg = embedded_playable_config(
        &data,
        embedded_disk_world(&data, world_dir.path()),
        "T02 respawn persistence rejoin",
    );
    second_cfg.shutdown = second_shutdown.clone();
    let second_bound = mc_net::bind(second_cfg).await.expect("bind restarted respawn server");
    let second_addr = second_bound.local_addr().expect("second local_addr");
    let second_serve = tokio::spawn(async move { second_bound.serve_and_save().await });

    let (mut rejoined, _) = connect_to_play(second_addr, "T02Respawn").await;
    rejoined
        .write_packet(&ServerboundChatCommand {
            command: "debug survival damage 1".into(),
        })
        .await
        .expect("damage rejoined alive player");
    wait_for_health_level(&mut rejoined, 19.0).await;

    drop(rejoined);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), second_serve)
        .await
        .expect("second respawn server shutdown")
        .expect("second respawn server join")
        .expect("second respawn server serve");
}

async fn drain_complete_spawn_view(client: &mut Client) {
    let expected_count = (2 * VIEW_DISTANCE + 1).pow(2) as usize;
    let mut seen = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while seen.len() < expected_count {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "initial view did not complete before respawn probe: saw {} of {expected_count}: {error}",
                    seen.len()
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode initial chunk");
            if (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_x)
                && (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_z)
            {
                seen.insert((pkt.chunk_x, pkt.chunk_z));
            }
        }
    }
}
