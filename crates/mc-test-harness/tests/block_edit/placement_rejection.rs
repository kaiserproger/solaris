#[tokio::test]
async fn rejected_occupied_use_item_on_resyncs_clicked_and_target_before_ack() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let clicked = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };
    let target = mc_world::BlockPos {
        x: 1,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(target, dirt_state_id)
        .expect("seed occupied target")
        .expect("replace target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 rejected UseItemOn resync".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100Reject").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(surface_y, support_y, "spawn should expose seeded cells");

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1001,
        })
        .await
        .expect("send occupied placement");

    read_rejected_place_resync_before_ack(
        &mut client,
        1001,
        (clicked.x, clicked.y, clicked.z),
        (target.x, target.y, target.z),
        dirt_state_id.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn rejected_occupied_bucket_use_item_on_resyncs_blocks_and_held_slot_before_ack() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let clicked = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };
    let target = mc_world::BlockPos {
        x: 1,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(target, dirt_state_id)
        .expect("seed occupied target")
        .expect("replace target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 rejected bucket UseItemOn resync".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100BucketReject").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(surface_y, support_y, "spawn should expose seeded cells");

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give water bucket");
    let give_slot = wait_for_slot_stack_update(&mut client, water_bucket_item_id, 1).await;
    let expected_corrective_state_id = give_slot.state_id.wrapping_add(1);

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1002,
        })
        .await
        .expect("send occupied bucket use");

    read_rejected_bucket_resync_before_ack(
        &mut client,
        1002,
        (clicked.x, clicked.y, clicked.z),
        (target.x, target.y, target.z),
        dirt_state_id.0 as i32,
        water_bucket_item_id,
        expected_corrective_state_id,
    )
    .await;
}

#[tokio::test]
async fn rejected_world_border_use_item_on_resyncs_without_placing() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let clicked = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };
    let target = mc_world::BlockPos {
        x: 1,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(target, air_state_id)
        .expect("seed target")
        .expect("replace target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 world-border UseItemOn resync".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100Border").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(surface_y, support_y, "spawn should expose seeded cells");

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: true,
            sequence: 1005,
        })
        .await
        .expect("send world-border placement");

    read_rejected_world_border_resync_before_ack(
        &mut client,
        1005,
        (clicked.x, clicked.y, clicked.z),
        (target.x, target.y, target.z),
        dirt_state_id.0 as i32,
        air_state_id.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn far_world_border_use_item_on_does_not_load_blocks_or_resync_bucket() {
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
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 far world-border UseItemOn no load".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100FarBorder").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let clicked = (96, sync.y.floor() as i32 - 2, 0);
    let target = (97, clicked.1, 0);

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give water bucket");
    wait_for_slot_stack(&mut client, water_bucket_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.0, clicked.1, clicked.2),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: true,
            sequence: 1006,
        })
        .await
        .expect("send far world-border bucket use");

    read_far_world_border_ack_without_resync(
        &mut client,
        1006,
        clicked,
        target,
        water_bucket_item_id,
    )
    .await;
}

#[tokio::test]
async fn rejected_out_of_reach_use_item_on_resyncs_clicked_and_target_before_ack() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y =
        top_non_air_y(&mut storage, 6, 0, air_state_id).expect("loaded test column terrain");
    let clicked = mc_world::BlockPos {
        x: 6,
        y: surface_y,
        z: 0,
    };
    let target = mc_world::BlockPos {
        x: 7,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(target, dirt_state_id)
        .expect("seed target")
        .expect("replace target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 out-of-reach UseItemOn resync".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100FarReject").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let dx = sync.x - (f64::from(clicked.x) + 0.5);
    let dy = sync.y + 1.62 - (f64::from(clicked.y) + 0.5);
    let dz = sync.z - (f64::from(clicked.z) + 0.5);
    assert!(
        dx * dx + dy * dy + dz * dz > 25.0,
        "seeded clicked block must be outside survival reach"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1003,
        })
        .await
        .expect("send out-of-reach placement");

    read_rejected_place_resync_before_ack(
        &mut client,
        1003,
        (clicked.x, clicked.y, clicked.z),
        (target.x, target.y, target.z),
        dirt_state_id.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn rejected_out_of_reach_bucket_use_item_on_resyncs_blocks_without_held_slot_before_ack() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y =
        top_non_air_y(&mut storage, 6, 0, air_state_id).expect("loaded test column terrain");
    let clicked = mc_world::BlockPos {
        x: 6,
        y: surface_y,
        z: 0,
    };
    let target = mc_world::BlockPos {
        x: 7,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(target, dirt_state_id)
        .expect("seed target")
        .expect("replace target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 out-of-reach bucket UseItemOn resync".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100FarBucket").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let dx = sync.x - (f64::from(clicked.x) + 0.5);
    let dy = sync.y + 1.62 - (f64::from(clicked.y) + 0.5);
    let dz = sync.z - (f64::from(clicked.z) + 0.5);
    assert!(
        dx * dx + dy * dy + dz * dz > 25.0,
        "seeded clicked block must be outside survival reach"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give water bucket");
    wait_for_slot_stack(&mut client, water_bucket_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 1004,
        })
        .await
        .expect("send out-of-reach bucket use");

    read_rejected_bucketless_resync_before_ack(
        &mut client,
        1004,
        (clicked.x, clicked.y, clicked.z),
        (target.x, target.y, target.z),
        dirt_state_id.0 as i32,
        water_bucket_item_id,
    )
    .await;
}

async fn read_rejected_place_resync_before_ack(
    client: &mut Client,
    sequence: i32,
    clicked: (i32, i32, i32),
    target: (i32, i32, i32),
    dirt_state_id: i32,
) {
    let mut saw_clicked_resync = false;
    let mut saw_target_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected placement resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == clicked {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_clicked_resync = true;
            } else if pos == target {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_target_resync = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.container_id != 0 || pkt.slot != 36,
                "rejected placement must not debit or resync the held dirt slot before ack"
            );
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                assert!(
                    saw_clicked_resync,
                    "rejected placement ack arrived before clicked-cell BlockUpdate"
                );
                assert!(
                    saw_target_resync,
                    "rejected placement ack arrived before target-cell BlockUpdate"
                );
                return;
            }
        }
    }
}

async fn read_far_world_border_ack_without_resync(
    client: &mut Client,
    sequence: i32,
    clicked: (i32, i32, i32),
    target: (i32, i32, i32),
    water_bucket_item_id: u32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("far world-border placement ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            assert!(
                pos != clicked && pos != target,
                "far world-border UseItemOn must not load and resync clicked/target cells"
            );
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                !(pkt.container_id == 0
                    && pkt.slot == 36
                    && pkt.item_stack.item_id == water_bucket_item_id
                    && pkt.item_stack.count == 1),
                "far world-border UseItemOn must not resync the held bucket slot before ack"
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

async fn read_rejected_world_border_resync_before_ack(
    client: &mut Client,
    sequence: i32,
    clicked: (i32, i32, i32),
    target: (i32, i32, i32),
    clicked_state_id: i32,
    target_state_id: i32,
) {
    let mut saw_clicked_resync = false;
    let mut saw_target_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("world-border placement resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == clicked {
                assert_eq!(pkt.state_id, clicked_state_id);
                saw_clicked_resync = true;
            } else if pos == target {
                assert_eq!(
                    pkt.state_id, target_state_id,
                    "world-border UseItemOn must not place into the target cell"
                );
                saw_target_resync = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.slot != 36 || !pkt.item_stack.is_empty(),
                "world-border UseItemOn must not consume the held stack"
            );
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                assert!(
                    saw_clicked_resync,
                    "world-border ack arrived before clicked-cell BlockUpdate"
                );
                assert!(
                    saw_target_resync,
                    "world-border ack arrived before target-cell BlockUpdate"
                );
                return;
            }
        }
    }
}

async fn read_rejected_bucketless_resync_before_ack(
    client: &mut Client,
    sequence: i32,
    clicked: (i32, i32, i32),
    target: (i32, i32, i32),
    dirt_state_id: i32,
    water_bucket_item_id: u32,
) {
    let mut saw_clicked_resync = false;
    let mut saw_target_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected bucketless placement resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == clicked {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_clicked_resync = true;
            } else if pos == target {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_target_resync = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.container_id == 0
                && pkt.slot == 36
                && pkt.item_stack.item_id == water_bucket_item_id
                && pkt.item_stack.count == 1
            {
                panic!("out-of-reach UseItemOn must not resync held water-bucket slot before ack");
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                assert!(
                    saw_clicked_resync,
                    "rejected bucketless ack arrived before clicked-cell BlockUpdate"
                );
                assert!(
                    saw_target_resync,
                    "rejected bucketless ack arrived before target-cell BlockUpdate"
                );
                return;
            }
        }
    }
}

async fn read_rejected_bucket_resync_before_ack(
    client: &mut Client,
    sequence: i32,
    clicked: (i32, i32, i32),
    target: (i32, i32, i32),
    dirt_state_id: i32,
    water_bucket_item_id: u32,
    expected_corrective_state_id: i32,
) {
    let mut saw_clicked_resync = false;
    let mut saw_target_resync = false;
    let mut saw_held_slot_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected bucket placement resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == clicked {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_clicked_resync = true;
            } else if pos == target {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_target_resync = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.container_id == 0
                && pkt.slot == 36
                && pkt.item_stack.item_id == water_bucket_item_id
                && pkt.item_stack.count == 1
            {
                assert_eq!(
                    pkt.state_id, expected_corrective_state_id,
                    "rejected bucket corrective SetSlot must advance inventory state id"
                );
                saw_held_slot_resync = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                assert!(
                    saw_clicked_resync,
                    "rejected bucket ack arrived before clicked-cell BlockUpdate"
                );
                assert!(
                    saw_target_resync,
                    "rejected bucket ack arrived before target-cell BlockUpdate"
                );
                assert!(
                    saw_held_slot_resync,
                    "rejected bucket ack arrived before held-slot ContainerSetSlot"
                );
                return;
            }
        }
    }
}

#[tokio::test]
async fn rejected_wall_torch_on_fence_resyncs_before_ack_without_debit() {
    let Some(WallTorchWireFixture {
        mut client,
        clicked,
        target,
        air_state,
        support_state: fence_state,
        torch_item,
        ..
    }) = start_wall_torch_wire_fixture("WallTorchReject", "minecraft:oak_fence").await
    else {
        return;
    };

    let sequence = 2502;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(clicked.x, clicked.y, clicked.z),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("attempt wall torch placement");

    let mut saw_clicked_resync = false;
    let mut saw_target_resync = false;
    let mut saw_held_slot_resync = false;
    loop {
        let frame = client
            .read_frame_with_timeout(Duration::from_secs(30))
            .await
            .expect("rejected wall torch response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode torch resync");
            let pos = unpack_block_pos(packet.position);
            if pos == (clicked.x, clicked.y, clicked.z) {
                assert_eq!(packet.state_id, fence_state.0 as i32);
                saw_clicked_resync = true;
            } else if pos == (target.x, target.y, target.z) {
                assert_eq!(packet.state_id, air_state.0 as i32);
                saw_target_resync = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body).expect("decode torch slot");
            if packet.container_id == 0 && packet.slot == 36 {
                assert_eq!(packet.item_stack.item_id, torch_item);
                assert_eq!(packet.item_stack.count, 1);
                saw_held_slot_resync = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode torch rejection ack");
            if packet.sequence == sequence {
                assert!(
                    saw_clicked_resync,
                    "clicked resync precedes rejected torch ack"
                );
                assert!(
                    saw_target_resync,
                    "target resync precedes rejected torch ack"
                );
                assert!(
                    saw_held_slot_resync,
                    "held torch stack resync precedes rejected torch ack"
                );
                return;
            }
        }
    }
}
