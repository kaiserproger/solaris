#[tokio::test]
async fn survival_campfire_cooks_held_input_into_item_entity() {
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
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let dirt = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt block")
        .default;
    let surface_y = top_non_air_y(&mut storage, 0, 0, air).expect("spawn terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 2,
                y: surface_y,
                z: 2,
            },
            dirt,
        )
        .expect("seed campfire support");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 2,
                y: surface_y + 1,
                z: 2,
            },
            air,
        )
        .expect("clear campfire target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items.id_of(&cooked_porkchop).expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 4,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M55 campfire cooking".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M55CampfireCook").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 101,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, campfire_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 102,
        })
        .await
        .expect("start campfire cooking");
    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    let mut item_entity_id = None;
    let mut saw_cooked_stack = false;
    let mut saw_empty_visual = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(saw_cooked_stack && saw_empty_visual) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("campfire cooked output");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode campfire ack");
            assert_eq!(pkt.sequence, 102);
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode campfire item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundSetEntityData::decode(&mut body).expect("decode campfire item metadata");
            if Some(pkt.entity_id) == item_entity_id {
                saw_cooked_stack = pkt.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == cooked_porkchop_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode campfire pickup slot");
            saw_cooked_stack =
                pkt.item_stack.item_id == cooked_porkchop_id && pkt.item_stack.count >= 1;
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode campfire clear BlockEntityData");
            if pkt.position == campfire_pos {
                saw_empty_visual = campfire_items(&pkt.nbt).is_some_and(|items| items.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn survival_unlit_campfire_does_not_finish_cooking() {
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
    let surface_y = seed_campfire_placement_site(&mut storage, &blocks);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items.id_of(&cooked_porkchop).expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 4,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 unlit campfire cooking".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
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
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100UnlitCamp").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 701,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: campfire_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 702,
        })
        .await
        .expect("start campfire cooking");
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    let campfire_block_pos = mc_world::BlockPos {
        x: campfire_x,
        y: campfire_y,
        z: campfire_z,
    };
    let unlit_state = {
        let mut storage = world.lock().await;
        let current = storage
            .get_block(campfire_block_pos)
            .expect("read campfire block")
            .expect("campfire block present");
        let current_state = blocks.by_id(current).expect("current campfire state");
        let mut props = current_state.properties.clone();
        for (key, value) in &mut props {
            if key == "lit" {
                *value = "false".into();
            }
        }
        blocks
            .by_name_and_props(&campfire_id, &props)
            .expect("unlit campfire state")
    };
    world
        .lock()
        .await
        .set_block_at(campfire_block_pos, unlit_state)
        .expect("set unlit campfire");

    assert_no_cooked_campfire_output(
        &mut client,
        campfire_pos,
        cooked_porkchop_id,
        item_entity_type,
        &mut simulation_ticks,
        18,
    )
    .await
    .expect("unlit campfire observation reaches its simulation-tick fence");

    let bytes = world
        .lock()
        .await
        .cached_chunk(mc_world::ChunkPos { x: 0, z: 0 })
        .expect("campfire chunk remains resident")
        .block_entities
        .get(&campfire_block_pos)
        .expect("unlit campfire block entity remains present")
        .clone();
    let mut cursor = std::io::Cursor::new(bytes);
    let tag = mc_nbt::read_network(&mut cursor).expect("decode cooled campfire state");
    assert_eq!(
        compound_int_array(&tag, "CookingTimes").and_then(|times| times.first().copied()),
        Some(0),
        "unlit campfire should cool vanilla progress back to zero"
    );
    assert!(
        campfire_items(&tag).is_some_and(|items| items
            .iter()
            .any(|item| campfire_item_matches(item, &porkchop_name))),
        "cooling down must retain the uncooked input"
    );
}

#[tokio::test]
async fn survival_campfire_in_flight_state_flushes_to_disk() {
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
    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("region dir");
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));

    let mut storage =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("disk world opens")
            .with_generator(generator)
            .with_item_registry(Arc::clone(&items));
    let surface_y = seed_campfire_placement_site(&mut storage, &blocks);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let porkchop_name = porkchop.as_str().to_string();

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 200,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire persistence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes,
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let save = bound.save_handle();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100CampPersist").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 201,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: campfire_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 202,
        })
        .await
        .expect("start campfire cooking");
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    let report = save.save_all().await;
    assert!(report.is_ok(), "save errors: {:?}", report.errors);
    assert!(
        report.chunks_flushed > 0,
        "in-flight campfire state should dirty a chunk"
    );

    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("reopen disk world")
            .with_item_registry(items);
    let campfire_block_pos = mc_world::BlockPos {
        x: campfire_x,
        y: campfire_y,
        z: campfire_z,
    };
    let chunk = reopened
        .get_chunk(mc_world::ChunkPos { x: 0, z: 0 })
        .expect("load reopened chunk")
        .expect("reopened chunk exists");
    let bytes = chunk
        .block_entities
        .get(&campfire_block_pos)
        .expect("in-flight campfire block entity persisted");
    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let tag = mc_nbt::read_network(&mut cursor).expect("read persisted campfire block entity");

    assert_eq!(
        compound_string(&tag, "id"),
        Some("minecraft:campfire"),
        "persistent campfire block entity should keep its type id"
    );
    assert_eq!(compound_int(&tag, "x"), Some(campfire_x));
    assert_eq!(compound_int(&tag, "y"), Some(campfire_y));
    assert_eq!(compound_int(&tag, "z"), Some(campfire_z));
    assert!(
        campfire_items(&tag).is_some_and(|items| items
            .iter()
            .any(|item| campfire_item_matches(item, &porkchop_name))),
        "persistent campfire block entity should retain the cooking input"
    );
    assert!(
        compound_int_array(&tag, "CookingTimes")
            .is_some_and(|times| times.first().is_some_and(|ticks| (0..200).contains(ticks))),
        "persistent campfire block entity should retain vanilla spent cook time"
    );
    assert_eq!(
        compound_int_array(&tag, "CookingTotalTimes").and_then(|total| total.first().copied()),
        Some(200)
    );
    assert_eq!(compound_int_array(&tag, "solaris_cooking_remaining"), None);
    assert_eq!(compound_int_array(&tag, "solaris_cooking_total"), None);
}

#[tokio::test]
async fn survival_campfire_in_flight_state_resumes_after_reopen() {
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
    let generator: Arc<dyn mc_world::ChunkGenerator> =
        Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("region dir");
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items.id_of(&cooked_porkchop).expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 4,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let mut first_storage =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("disk world opens")
            .with_generator(Arc::clone(&generator))
            .with_item_registry(Arc::clone(&items));
    let surface_y = seed_campfire_placement_site(&mut first_storage, &blocks);
    let first_world = Arc::new(tokio::sync::Mutex::new(first_storage));
    let first_shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire restart persistence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(&data),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&first_world)),
        tags: Arc::clone(&tags),
        recipes: Arc::clone(&recipes),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::clone(&entity_types),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: first_shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind first server");
    let first_addr = bound.local_addr().expect("first local_addr");
    let first_task = tokio::spawn(async move { bound.serve().await });

    let (mut client, sync) = connect_to_play(first_addr, "M100CampResumeA").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 301,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: campfire_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 302,
        })
        .await
        .expect("start campfire cooking");
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    {
        let mut storage = first_world.lock().await;
        storage
            .flush_dirty()
            .expect("flush in-flight campfire state");
    }

    drop(client);
    first_shutdown.request();
    tokio::time::timeout(Duration::from_secs(6), first_task)
        .await
        .expect("first server stops")
        .expect("first server task joins")
        .expect("first server serve result");
    drop(first_world);

    let second_world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("reopen disk world")
            .with_generator(generator)
            .with_item_registry(Arc::clone(&items)),
    ));
    let second_shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire restart persistence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world: Some(second_world),
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
        shutdown: second_shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind second server");
    let second_addr = bound.local_addr().expect("second local_addr");
    let second_task = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(second_addr, "M100CampResumeB").await;
    wait_for_campfire_cooked_output(
        &mut client,
        (0, 0),
        campfire_pos,
        cooked_porkchop_id,
        item_entity_type,
    )
    .await;

    drop(client);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(6), second_task)
        .await
        .expect("second server stops")
        .expect("second server task joins")
        .expect("second server serve result");
}

#[tokio::test]
async fn survival_campfire_finishes_while_no_clients_are_connected() {
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
    let surface_y = seed_campfire_placement_site(&mut storage, &blocks);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items.id_of(&cooked_porkchop).expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 80,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire zero-client ticking".into(),
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
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    let server_task = tokio::spawn(async move { bound.serve().await });

    let (mut client, sync) = connect_to_play(addr, "M100CampNoA").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 401,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: campfire_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 402,
        })
        .await
        .expect("start campfire cooking");
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    drop(client);
    wait_for_additional_simulation_ticks(&mut simulation_ticks, 80).await;

    let (mut client, _) = connect_to_play(addr, "M100CampNoB").await;
    wait_for_cooked_item_entity(
        &mut client,
        cooked_porkchop_id,
        item_entity_type,
        Duration::from_millis(900),
    )
    .await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(6), server_task)
        .await
        .expect("server stops")
        .expect("server task joins")
        .expect("server serve result");
}

#[tokio::test]
async fn survival_campfire_finishes_after_restart_before_any_client_reconnects() {
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
    let generator: Arc<dyn mc_world::ChunkGenerator> =
        Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::create_dir_all(world_dir.path().join("region")).expect("region dir");
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id = i32::try_from(
        blocks
            .block(&campfire_id)
            .expect("campfire block")
            .default
            .0,
    )
    .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items.id_of(&cooked_porkchop).expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 80,
            experience_milli: 0,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let mut first_storage =
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("disk world opens")
            .with_generator(Arc::clone(&generator))
            .with_item_registry(Arc::clone(&items));
    let surface_y = seed_campfire_placement_site(&mut first_storage, &blocks);
    let first_world = Arc::new(tokio::sync::Mutex::new(first_storage));
    let first_shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire restart no-client ticking".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data: Arc::clone(&data),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&first_world)),
        tags: Arc::clone(&tags),
        recipes: Arc::clone(&recipes),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::clone(&entity_types),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: first_shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind first server");
    let first_addr = bound.local_addr().expect("first local_addr");
    let first_task = tokio::spawn(async move { bound.serve().await });

    let (mut client, sync) = connect_to_play(first_addr, "M100NoCliA").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let campfire_x = 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(campfire_x, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 501,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(
        &mut client,
        (campfire_x, campfire_y, campfire_z),
        campfire_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    let campfire_pos = pack_block_pos(campfire_x, campfire_y, campfire_z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: campfire_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 502,
        })
        .await
        .expect("start campfire cooking");
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    {
        let mut storage = first_world.lock().await;
        let flushed = storage.flush_dirty().expect("flush dirty world");
        assert!(flushed > 0, "in-flight campfire state should dirty a chunk");
    }

    drop(client);
    first_shutdown.request();
    tokio::time::timeout(Duration::from_secs(6), first_task)
        .await
        .expect("first server stops")
        .expect("first server task joins")
        .expect("first server serve result");
    drop(first_world);

    let second_world = Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open_with_capacity(world_dir.path(), Arc::clone(&blocks), 128)
            .expect("reopen disk world")
            .with_generator(generator)
            .with_item_registry(Arc::clone(&items)),
    ));
    let second_shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 campfire restart no-client ticking".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world: Some(second_world),
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
        shutdown: second_shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind second server");
    let second_addr = bound.local_addr().expect("second local_addr");
    let mut simulation_ticks = bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    let second_task = tokio::spawn(async move { bound.serve().await });

    wait_for_additional_simulation_ticks(&mut simulation_ticks, 80).await;

    let (mut client, _) = connect_to_play(second_addr, "M100NoCliB").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    wait_for_cooked_item_entity(
        &mut client,
        cooked_porkchop_id,
        item_entity_type,
        Duration::from_millis(900),
    )
    .await;

    drop(client);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(6), second_task)
        .await
        .expect("second server stops")
        .expect("second server task joins")
        .expect("second server serve result");
}

fn seed_campfire_placement_site(
    storage: &mut mc_world::WorldStorage,
    blocks: &mc_world::BlockRegistry,
) -> i32 {
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let dirt = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt block")
        .default;
    let surface_y = top_non_air_y(storage, 0, 0, air).expect("spawn terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 2,
                y: surface_y,
                z: 2,
            },
            dirt,
        )
        .expect("seed campfire support");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 2,
                y: surface_y + 1,
                z: 2,
            },
            air,
        )
        .expect("clear campfire target");
    surface_y
}

async fn wait_for_additional_simulation_ticks(
    ticks: &mut tokio::sync::watch::Receiver<u64>,
    additional: u64,
) {
    let target = (*ticks.borrow()).saturating_add(additional);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let current = *ticks.borrow_and_update();
            if current >= target {
                return;
            }
            ticks
                .changed()
                .await
                .expect("simulation tick publisher remains active");
        }
    })
    .await
    .expect("simulation did not reach the required cooking tick");
}

async fn wait_for_campfire_input_visual_and_slot(
    client: &mut Client,
    campfire_pos: i64,
    input_item: &str,
) {
    let mut saw_slot_empty = false;
    let mut saw_input_visual = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_slot_empty && saw_input_visual) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("campfire input visual");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode campfire input SetSlot");
            saw_slot_empty |= pkt.container_id == 0 && pkt.slot == 36 && pkt.item_stack.is_empty();
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode campfire input BlockEntityData");
            if pkt.position == campfire_pos {
                saw_input_visual = campfire_items(&pkt.nbt).is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| campfire_item_matches(item, input_item))
                });
            }
        }
    }
}

async fn wait_for_campfire_cooked_output(
    client: &mut Client,
    target_chunk: (i32, i32),
    campfire_pos: i64,
    cooked_item_id: u32,
    item_entity_type: i32,
) {
    let (campfire_x, campfire_y, campfire_z) = unpack_block_pos(campfire_pos);
    let packed_xz = (((campfire_x & 15) << 4) | (campfire_z & 15)) as u8;
    let mut saw_target_chunk = false;
    let mut item_entity_id = None;
    let mut saw_cooked_stack = false;
    let mut saw_empty_visual = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(saw_target_chunk && saw_cooked_stack && saw_empty_visual) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("campfire cooked output after restart");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt =
                LevelChunkWithLight::decode(&mut body).expect("decode restarted campfire chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target_chunk {
                saw_target_chunk = true;
                saw_empty_visual |= pkt.block_entities.iter().any(|block_entity| {
                    block_entity.packed_xz == packed_xz
                        && i32::from(block_entity.y) == campfire_y
                        && campfire_items(&block_entity.nbt).is_some_and(|items| items.is_empty())
                });
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt =
                AddEntity::decode(&mut body).expect("decode restarted campfire item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body)
                .expect("decode restarted campfire item metadata");
            if Some(pkt.entity_id) == item_entity_id {
                saw_cooked_stack = pkt.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == cooked_item_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode restarted campfire pickup slot");
            saw_cooked_stack =
                pkt.item_stack.item_id == cooked_item_id && pkt.item_stack.count >= 1;
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode restarted campfire clear BlockEntityData");
            if pkt.position == campfire_pos {
                saw_empty_visual = campfire_items(&pkt.nbt).is_some_and(|items| items.is_empty());
            }
        }
    }
}

async fn wait_for_cooked_item_entity(
    client: &mut Client,
    cooked_item_id: u32,
    item_entity_type: i32,
    timeout: Duration,
) {
    let mut item_entity_ids = HashSet::new();
    let mut cooked_metadata_before_add = HashSet::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("cooked item entity after zero-client campfire ticking");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode campfire item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                if cooked_metadata_before_add.contains(&pkt.entity_id) {
                    return;
                }
                item_entity_ids.insert(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundSetEntityData::decode(&mut body).expect("decode campfire item metadata");
            let has_cooked_stack = pkt.values.iter().any(|value| {
                matches!(
                    value,
                    EntityDataValue::ItemStack { index, stack }
                        if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                            && stack.item_id == cooked_item_id
                            && stack.count == 1
                )
            });
            if has_cooked_stack {
                if item_entity_ids.contains(&pkt.entity_id) {
                    return;
                }
                cooked_metadata_before_add.insert(pkt.entity_id);
            }
        }
    }
}

async fn assert_no_cooked_campfire_output(
    client: &mut Client,
    campfire_pos: i64,
    cooked_item_id: u32,
    item_entity_type: i32,
    simulation_ticks: &mut tokio::sync::watch::Receiver<u64>,
    additional_ticks: u64,
) -> Result<(), tokio::time::error::Elapsed> {
    let target_tick = (*simulation_ticks.borrow()).saturating_add(additional_ticks);
    let mut cooked_entity_ids = HashSet::new();
    let mut cooked_metadata_before_add = HashSet::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while *simulation_ticks.borrow_and_update() < target_tick {
            tokio::select! {
                changed = simulation_ticks.changed() => {
                    changed.expect("simulation tick publisher remains active");
                }
                frame = client.read_frame() => {
                    assert_not_cooked_campfire_frame(
                        client,
                        frame.expect("read unlit campfire frame"),
                        campfire_pos,
                        cooked_item_id,
                        item_entity_type,
                        &mut cooked_entity_ids,
                        &mut cooked_metadata_before_add,
                    ).await;
                }
            }
        }

        client
            .write_packet(&ServerboundChatCommand {
                command: "time set 1000".into(),
            })
            .await
            .expect("send unlit campfire packet fence");
        loop {
            let frame = client
                .read_frame()
                .await
                .expect("read unlit campfire packet fence");
            if frame.id == ClientboundSetTime::ID {
                let mut body = frame.body;
                let _time = ClientboundSetTime::decode(&mut body)
                    .expect("decode unlit campfire packet fence");
                return;
            }
            assert_not_cooked_campfire_frame(
                client,
                frame,
                campfire_pos,
                cooked_item_id,
                item_entity_type,
                &mut cooked_entity_ids,
                &mut cooked_metadata_before_add,
            )
            .await;
        }
    })
    .await
}

async fn assert_not_cooked_campfire_frame(
    client: &mut Client,
    frame: mc_protocol::RawFrame,
    campfire_pos: i64,
    cooked_item_id: u32,
    item_entity_type: i32,
    cooked_entity_ids: &mut HashSet<i32>,
    cooked_metadata_before_add: &mut HashSet<i32>,
) {
    if handle_keepalive(client, frame.id, &frame.body).await {
        return;
    }
    if frame.id == AddEntity::ID {
        let mut body = frame.body;
        let pkt = AddEntity::decode(&mut body).expect("decode campfire item AddEntity");
        if pkt.entity_type_id == item_entity_type {
            assert!(
                !cooked_metadata_before_add.contains(&pkt.entity_id),
                "unlit campfire should not spawn cooked item entity"
            );
            cooked_entity_ids.insert(pkt.entity_id);
        }
    } else if frame.id == ClientboundSetEntityData::ID {
        let mut body = frame.body;
        let pkt =
            ClientboundSetEntityData::decode(&mut body).expect("decode campfire item metadata");
        let has_cooked_stack = pkt.values.iter().any(|value| {
            matches!(
                value,
                EntityDataValue::ItemStack { index, stack }
                    if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                        && stack.item_id == cooked_item_id
                        && stack.count >= 1
            )
        });
        if has_cooked_stack {
            assert!(
                !cooked_entity_ids.contains(&pkt.entity_id),
                "unlit campfire should not spawn cooked item entity"
            );
            cooked_metadata_before_add.insert(pkt.entity_id);
        }
    } else if frame.id == ClientboundBlockEntityData::ID {
        let mut body = frame.body;
        let pkt =
            ClientboundBlockEntityData::decode(&mut body).expect("decode campfire clear data");
        assert!(
            pkt.position != campfire_pos
                || !campfire_items(&pkt.nbt).is_some_and(|items| items.is_empty()),
            "unlit campfire should not clear cooking visual state"
        );
    }
}

fn campfire_items(nbt: &mc_nbt::Tag) -> Option<&[mc_nbt::Tag]> {
    let mc_nbt::Tag::Compound(fields) = nbt else {
        return None;
    };
    let items = fields
        .iter()
        .find_map(|(name, value)| (name == "Items").then_some(value))?;
    let mc_nbt::Tag::List(list) = items else {
        return None;
    };
    Some(&list.elements)
}

fn compound_string<'a>(nbt: &'a mc_nbt::Tag, name: &str) -> Option<&'a str> {
    let mc_nbt::Tag::Compound(fields) = nbt else {
        return None;
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
        .and_then(|value| match value {
            mc_nbt::Tag::String(value) => Some(value.as_str()),
            _ => None,
        })
}

fn compound_int(nbt: &mc_nbt::Tag, name: &str) -> Option<i32> {
    let mc_nbt::Tag::Compound(fields) = nbt else {
        return None;
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
        .and_then(|value| match value {
            mc_nbt::Tag::Int(value) => Some(*value),
            _ => None,
        })
}

fn compound_int_array<'a>(nbt: &'a mc_nbt::Tag, name: &str) -> Option<&'a [i32]> {
    let mc_nbt::Tag::Compound(fields) = nbt else {
        return None;
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
        .and_then(|value| match value {
            mc_nbt::Tag::IntArray(values) => Some(values.as_slice()),
            _ => None,
        })
}

fn campfire_item_matches(item: &mc_nbt::Tag, expected_id: &str) -> bool {
    let mc_nbt::Tag::Compound(fields) = item else {
        return false;
    };
    let field = |name: &str| {
        fields
            .iter()
            .find_map(|(field, value)| (field == name).then_some(value))
    };
    matches!(field("Slot"), Some(mc_nbt::Tag::Int(0)))
        && matches!(field("id"), Some(mc_nbt::Tag::String(id)) if id == expected_id)
        && matches!(field("count"), Some(mc_nbt::Tag::Int(1)))
}
