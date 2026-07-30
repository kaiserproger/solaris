#[tokio::test]
async fn water_bucket_spread_waits_for_scheduled_fluid_delay() {
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
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&report);
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
    let source = mc_world::BlockPos {
        x: 1,
        y: surface_y,
        z: 0,
    };
    let spread = mc_world::BlockPos {
        x: 2,
        y: surface_y,
        z: 0,
    };
    for pos in [clicked, source, spread] {
        storage
            .set_block_at(
                mc_world::BlockPos {
                    y: surface_y - 1,
                    ..pos
                },
                dirt_state_id,
            )
            .expect("seed floor");
    }
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(source, air_state_id)
        .expect("seed source air")
        .expect("replace source");
    storage
        .set_block_at(spread, air_state_id)
        .expect("seed spread air")
        .expect("replace spread");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 water bucket scheduled spread".into(),
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
        block_facts: Arc::new(block_facts.clone()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100WaterDelay").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(
        sync.y.floor() as i32 - 2,
        surface_y,
        "spawn should expose seeded water test cells"
    );

    wait_for_world_ticks(&mut client, 8).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give water bucket");
    wait_for_slot_stack(&mut client, water_bucket_item_id, 1).await;

    let sequence = 6301;
    let early_spread_tick = (*simulation_ticks.borrow()).saturating_add(4);
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
        .expect("place water bucket");

    let mut saw_source = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_source && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("water source placement response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let position = unpack_block_pos(pkt.position);
            if position == (source.x, source.y, source.z) {
                assert!(is_water_state(&block_facts, pkt.state_id));
                saw_source = true;
            } else {
                assert!(
                    !is_water_state(&block_facts, pkt.state_id),
                    "water spread update arrived before placement acknowledgement at {position:?}"
                );
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode water ack");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
        }
    }

    assert_no_early_water_spread(
        &mut client,
        &block_facts,
        (source.x, source.y, source.z),
        &mut simulation_ticks,
        early_spread_tick,
    )
    .await;
    wait_for_delayed_water_spread(&mut client, &block_facts, (source.x, source.y, source.z)).await;
}

#[tokio::test]
async fn lava_bucket_next_to_water_solidifies_through_scheduled_fluid_tick() {
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
    let block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report(&report);
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let water_source_state_id =
        fluid_state_with_level(&blocks, "minecraft:water", 0).expect("water source in registry");
    let obsidian_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:obsidian").unwrap())
        .map(|b| b.default)
        .expect("obsidian in registry");
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
    let lava_target = mc_world::BlockPos {
        x: 1,
        y: surface_y,
        z: 0,
    };
    let water = mc_world::BlockPos {
        x: 2,
        y: surface_y,
        z: 0,
    };
    for pos in [clicked, lava_target, water] {
        storage
            .set_block_at(
                mc_world::BlockPos {
                    y: surface_y - 1,
                    ..pos
                },
                dirt_state_id,
            )
            .expect("seed floor");
    }
    storage
        .set_block_at(clicked, dirt_state_id)
        .expect("seed clicked")
        .expect("replace clicked");
    storage
        .set_block_at(lava_target, air_state_id)
        .expect("seed lava target air")
        .expect("replace lava target");
    storage
        .set_block_at(water, water_source_state_id)
        .expect("seed water source")
        .expect("replace water source");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let lava_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:lava_bucket").unwrap())
        .expect("lava bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 lava water scheduled solidification".into(),
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
        block_facts: Arc::new(block_facts.clone()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100LavaWater").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(
        sync.y.floor() as i32 - 2,
        surface_y,
        "spawn should expose seeded lava/water test cells"
    );

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:lava_bucket 1 0".into(),
        })
        .await
        .expect("give lava bucket");
    wait_for_slot_stack(&mut client, lava_bucket_item_id, 1).await;

    let sequence = 6401;
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
        .expect("place lava bucket");

    wait_for_fluid_block_ack_and_source(
        &mut client,
        &block_facts,
        sequence,
        (lava_target.x, lava_target.y, lava_target.z),
        mc_data::block_facts::FluidKind::Lava,
    )
    .await;
    wait_for_block_update(
        &mut client,
        (lava_target.x, lava_target.y, lava_target.z),
        obsidian_state_id.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn water_bucket_scheduled_spread_survives_save_restart_without_duplicate_tick() {
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
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &report,
    ));
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|block| block.default)
        .expect("air in registry");
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|block| block.default)
        .expect("dirt in registry");
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report =
        mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");

    let world_dir = tempfile::tempdir().expect("fluid restart temp world");
    std::fs::create_dir_all(world_dir.path().join("region"))
        .expect("create fluid restart region dir");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("open fluid restart disk world")
    .with_item_registry(Arc::clone(&items))
    .with_generator(generator);
    let surface_y =
        top_non_air_y(&mut storage, 0, 0, air_state).expect("fluid restart spawn terrain");
    let clicked = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };
    let source = mc_world::BlockPos { x: 1, ..clicked };
    let spread = mc_world::BlockPos { x: 2, ..clicked };
    for pos in [clicked, source, spread] {
        storage
            .set_block_at(
                mc_world::BlockPos {
                    y: surface_y - 1,
                    ..pos
                },
                dirt_state,
            )
            .expect("seed fluid restart floor");
    }
    storage
        .set_block_at(clicked, dirt_state)
        .expect("seed fluid restart clicked block")
        .expect("replace fluid restart clicked block");
    storage
        .set_block_at(source, air_state)
        .expect("seed fluid restart source air")
        .expect("replace fluid restart source");
    storage
        .set_block_at(spread, air_state)
        .expect("seed fluid restart spread air")
        .expect("replace fluid restart spread");

    let first_shutdown = mc_net::ShutdownHandle::default();
    let first_cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "T02 scheduled fluid restart placement".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(&data),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(storage))),
        tags: Arc::clone(&tags),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::clone(&block_facts),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: first_shutdown.clone(),
    };
    let first_bound = mc_net::bind(first_cfg)
        .await
        .expect("bind first fluid restart server");
    let first_addr = first_bound.local_addr().expect("first fluid restart addr");
    let first_serve = tokio::spawn(async move { first_bound.serve_and_save().await });

    let (mut client, sync) = connect_to_play(first_addr, "T02FluidRestart").await;
    assert_eq!(surface_y, sync.y.floor() as i32 - 2);
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give restart water bucket");
    wait_for_slot_stack(&mut client, water_bucket_item_id, 1).await;

    let sequence = 6501;
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
        .expect("place restart water source");
    wait_for_fluid_block_ack_and_source(
        &mut client,
        &block_facts,
        sequence,
        (source.x, source.y, source.z),
        mc_data::block_facts::FluidKind::Water,
    )
    .await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("save pending fluid world");
    wait_for_save_all_feedback(&mut client).await;
    drop(client);
    first_shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), first_serve)
        .await
        .expect("first fluid server shutdown")
        .expect("first fluid server join")
        .expect("first fluid server serve");

    let cpos = mc_world::ChunkPos {
        x: source.x.div_euclid(16),
        z: source.z.div_euclid(16),
    };
    let mut first_reopen =
        mc_world::WorldStorage::open(world_dir.path(), Arc::clone(&blocks))
            .expect("reopen pending fluid world")
            .with_item_registry(Arc::clone(&items));
    let persisted_source = first_reopen
        .get_block(source)
        .expect("read persisted water source")
        .expect("persisted source chunk exists");
    assert!(is_water_state(&block_facts, persisted_source.0 as i32));
    let persisted_spread = first_reopen
        .get_block(spread)
        .expect("read persisted spread cell")
        .expect("persisted spread chunk exists");
    let persisted_ticks = assert_unique_scheduled_fluid_ticks(&mut first_reopen, cpos);
    if persisted_spread == air_state {
        assert!(
            persisted_ticks.iter().any(|tick| {
                tick.pos == source && tick.fluid.as_str() == "minecraft:water"
            }),
            "pending spread air must retain its source water tick across restart"
        );
    } else {
        assert!(
            is_water_state(&block_facts, persisted_spread.0 as i32),
            "restart boundary must persist either pending air or settled water"
        );
    }
    drop(first_reopen);

    let second_generator =
        Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let second_storage = mc_world::WorldStorage::open_with_capacity(
        world_dir.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("open restarted fluid world")
    .with_item_registry(Arc::clone(&items))
    .with_generator(second_generator);
    let second_world = Arc::new(tokio::sync::Mutex::new(second_storage));
    let second_shutdown = mc_net::ShutdownHandle::default();
    let second_cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "T02 scheduled fluid restart continuation".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(&data),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&second_world)),
        tags: Arc::clone(&tags),
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::clone(&block_facts),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: second_shutdown.clone(),
    };
    let second_bound = mc_net::bind(second_cfg)
        .await
        .expect("bind restarted fluid server");
    let second_addr = second_bound.local_addr().expect("restarted fluid addr");
    let second_serve = tokio::spawn(async move { second_bound.serve_and_save().await });

    let (mut client, _) = connect_to_play(second_addr, "T02FluidRestart").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    wait_for_world_ticks(&mut client, 12).await;
    {
        let mut storage = second_world.lock().await;
        let source_state = storage
            .get_block(source)
            .expect("read restarted water source")
            .expect("restarted source chunk exists");
        let spread_state = storage
            .get_block(spread)
            .expect("read restarted spread cell")
            .expect("restarted spread chunk exists");
        assert!(is_water_state(&block_facts, source_state.0 as i32));
        assert!(
            is_water_state(&block_facts, spread_state.0 as i32),
            "persisted fluid continuation must reach the adjacent spread cell"
        );
        assert_unique_scheduled_fluid_ticks(&mut storage, cpos);
    }
    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("save continued fluid world");
    wait_for_save_all_feedback(&mut client).await;
    drop(client);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), second_serve)
        .await
        .expect("second fluid server shutdown")
        .expect("second fluid server join")
        .expect("second fluid server serve");

    let mut final_reopen =
        mc_world::WorldStorage::open(world_dir.path(), Arc::clone(&blocks))
            .expect("reopen continued fluid world")
            .with_item_registry(Arc::clone(&items));
    for (pos, label) in [(source, "source"), (spread, "spread")] {
        let state = final_reopen
            .get_block(pos)
            .unwrap_or_else(|error| panic!("read final fluid {label}: {error}"))
            .unwrap_or_else(|| panic!("final fluid {label} chunk exists"));
        assert!(
            is_water_state(&block_facts, state.0 as i32),
            "final {label} must remain water"
        );
    }
    assert_unique_scheduled_fluid_ticks(&mut final_reopen, cpos);
}

fn assert_unique_scheduled_fluid_ticks(
    storage: &mut mc_world::WorldStorage,
    cpos: mc_world::ChunkPos,
) -> Vec<mc_world::ScheduledFluidTick> {
    let ticks = storage
        .scheduled_fluid_ticks(cpos)
        .expect("read scheduled fluid ticks")
        .unwrap_or(&[])
        .to_vec();
    let mut requests = HashSet::new();
    let mut sequences = HashSet::new();
    for tick in &ticks {
        assert!(
            requests.insert((
                tick.pos.x,
                tick.pos.y,
                tick.pos.z,
                tick.fluid.as_str().to_owned(),
                tick.trigger_tick,
                tick.priority,
            )),
            "save/restart must not duplicate a scheduled fluid request"
        );
        assert!(
            sequences.insert(tick.sequence()),
            "save/restart must not duplicate a scheduled fluid sequence"
        );
    }
    ticks
}

async fn assert_no_early_water_spread(
    client: &mut Client,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    source: (i32, i32, i32),
    simulation_ticks: &mut tokio::sync::watch::Receiver<u64>,
    target_tick: u64,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while *simulation_ticks.borrow_and_update() < target_tick {
            tokio::select! {
                changed = simulation_ticks.changed() => {
                    changed.expect("simulation tick publisher remains active");
                }
                frame = client.read_frame() => {
                    let frame = frame.expect("read pre-spread fluid frame");
                    if handle_keepalive(client, frame.id, &frame.body).await {
                        continue;
                    }
                    if frame.id == BlockUpdate::ID {
                        let mut body = frame.body;
                        let pkt = BlockUpdate::decode(&mut body).expect("decode early BlockUpdate");
                        let pos = unpack_block_pos(pkt.position);
                        assert!(
                            !(pos != source && is_water_state(block_facts, pkt.state_id)),
                            "water spread update arrived before the scheduled fluid delay at {pos:?}"
                        );
                    }
                }
            }
        }
    })
    .await
    .expect("simulation reached the pre-spread fluid tick fence");
}

async fn wait_for_delayed_water_spread(
    client: &mut Client,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    source: (i32, i32, i32),
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("delayed water spread update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode delayed BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos != source && is_water_state(block_facts, pkt.state_id) {
                return;
            }
        }
    }
}

fn is_water_state(block_facts: &mc_data::block_facts::BlockFactsTable, state_id: i32) -> bool {
    state_id >= 0
        && block_facts
            .fluid(state_id as u32)
            .is_some_and(|fluid| fluid.kind == mc_data::block_facts::FluidKind::Water)
}

async fn wait_for_fluid_block_ack_and_source(
    client: &mut Client,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    sequence: i32,
    pos: (i32, i32, i32),
    kind: mc_data::block_facts::FluidKind,
) {
    let mut saw_source = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_source && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("fluid source placement response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            if unpack_block_pos(pkt.position) == pos
                && pkt.state_id >= 0
                && block_facts
                    .fluid(pkt.state_id as u32)
                    .is_some_and(|fluid| fluid.kind == kind && fluid.source)
            {
                saw_source = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
        }
    }
}

fn fluid_state_with_level(
    blocks: &mc_world::BlockRegistry,
    id: &str,
    level: u8,
) -> Option<mc_world::BlockStateId> {
    blocks.by_name_and_props(
        &mc_data::Identifier::parse(id).expect("static fluid identifier"),
        &[("level".to_string(), level.to_string())],
    )
}
