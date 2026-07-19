#[tokio::test]
async fn survival_break_mature_wheat_drops_wheat_and_seeds() {
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
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

    let farmland = crop_test_state(&blocks, "minecraft:farmland", &[]);
    let wheat_age7 = crop_test_state(&blocks, "minecraft:wheat", &[("age", "7")]);
    let air_state_id = crop_test_state(&blocks, "minecraft:air", &[]).0 as i32;
    let wheat_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:wheat").unwrap())
        .expect("wheat item");
    let seeds_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:wheat_seeds").unwrap())
        .expect("wheat seeds item");
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .expect("item entity type") as i32;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M58 wheat harvest drops".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
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

    let (mut client, sync) = connect_to_play(addr, "M58WheatDrops").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let wheat_pos = (0, support_y + 1, 2);
    {
        let mut storage = world.lock().await;
        crop_test_set(&mut storage, (wheat_pos.0, support_y, wheat_pos.2), farmland);
        crop_test_set(&mut storage, wheat_pos, wheat_age7);
    }

    let target_pos = pack_block_pos(wheat_pos.0, wheat_pos.1, wheat_pos.2);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 581,
        })
        .await
        .expect("send wheat start break");

    wait_for_wheat_harvest_drops(
        &mut client,
        wheat_pos,
        air_state_id,
        item_entity_type,
        wheat_item_id,
        seeds_item_id,
        581,
    )
    .await;
}

async fn wait_for_wheat_harvest_drops(
    client: &mut Client,
    wheat_pos: (i32, i32, i32),
    air_state_id: i32,
    item_entity_type: i32,
    wheat_item_id: u32,
    seeds_item_id: u32,
    sequence: i32,
) {
    let mut item_entities = HashSet::new();
    let mut saw_block_break = false;
    let mut saw_ack = false;
    let mut saw_wheat = false;
    let mut saw_seeds = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_break && saw_ack && saw_wheat && saw_seeds) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("wheat harvest drop response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode wheat BlockUpdate");
            if unpack_block_pos(pkt.position) == wheat_pos {
                assert_eq!(pkt.state_id, air_state_id);
                saw_block_break = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode wheat break ack");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode wheat item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entities.insert(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode wheat item data");
            if item_entities.contains(&pkt.entity_id) {
                for value in pkt.values {
                    if let EntityDataValue::ItemStack { index, stack } = value
                        && index == ITEM_ENTITY_DATA_ITEM_INDEX
                    {
                        saw_wheat |= stack.item_id == wheat_item_id && stack.count == 1;
                        saw_seeds |= stack.item_id == seeds_item_id && stack.count == 1;
                    }
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode wheat pickup SetSlot");
            saw_wheat |= pkt.item_stack.item_id == wheat_item_id && pkt.item_stack.count >= 1;
            saw_seeds |= pkt.item_stack.item_id == seeds_item_id && pkt.item_stack.count >= 1;
        }
    }
}
