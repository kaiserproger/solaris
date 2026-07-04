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
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
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

    tokio::time::sleep(Duration::from_millis(400)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:water_bucket 1 0".into(),
        })
        .await
        .expect("give water bucket");
    wait_for_slot_stack(&mut client, water_bucket_item_id, 1).await;

    let sequence = 6301;
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
            if unpack_block_pos(pkt.position) == (source.x, source.y, source.z) {
                assert!(is_water_state(&block_facts, pkt.state_id));
                saw_source = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode water ack");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
        }
    }

    assert_no_early_water_spread(&mut client, &block_facts, (source.x, source.y, source.z)).await;
    wait_for_delayed_water_spread(&mut client, &block_facts, (source.x, source.y, source.z)).await;
}

async fn assert_no_early_water_spread(
    client: &mut Client,
    block_facts: &mc_data::block_facts::BlockFactsTable,
    source: (i32, i32, i32),
) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return;
        };
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
