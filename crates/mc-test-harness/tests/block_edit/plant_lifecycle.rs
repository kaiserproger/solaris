struct ExpectedPlantDelta {
    pos: (i32, i32, i32),
    state_id: i32,
    seen: bool,
}

#[tokio::test]
async fn survival_bonemeal_stem_places_fruit_and_cocoa_beans_place_cocoa() {
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
    let melon_stem_age7 = crop_test_state(&blocks, "minecraft:melon_stem", &[("age", "7")]);
    let attached_melon_stem_north = crop_test_state(
        &blocks,
        "minecraft:attached_melon_stem",
        &[("facing", "north")],
    );
    let melon = crop_test_state(&blocks, "minecraft:melon", &[]);
    let jungle_log = crop_test_state(&blocks, "minecraft:jungle_log", &[]);
    let cocoa_age0_east = crop_test_state(
        &blocks,
        "minecraft:cocoa",
        &[("age", "0"), ("facing", "east")],
    );
    let bone_meal_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:bone_meal").unwrap())
        .expect("bone meal item");
    let cocoa_beans_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:cocoa_beans").unwrap())
        .expect("cocoa beans item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M72 plant lifecycle".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
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

    let (mut client, sync) = connect_to_play(addr, "M72Plants").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let stem_pos = (0, support_y + 1, 2);
    let fruit_pos = (0, support_y + 1, 1);
    let log_pos = (0, support_y + 1, 3);
    let cocoa_pos = (1, support_y + 1, 3);
    {
        let mut storage = world.lock().await;
        crop_test_set(&mut storage, (0, support_y, 2), farmland);
        crop_test_set(&mut storage, stem_pos, melon_stem_age7);
        crop_test_set(&mut storage, log_pos, jungle_log);
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bone_meal 1 0".into(),
        })
        .await
        .expect("give bone meal");
    wait_for_slot_stack(&mut client, bone_meal_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(stem_pos.0, stem_pos.1, stem_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 721,
        })
        .await
        .expect("bonemeal mature melon stem");
    wait_for_plant_deltas(
        &mut client,
        &[
            (stem_pos, attached_melon_stem_north.0 as i32),
            (fruit_pos, melon.0 as i32),
        ],
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:cocoa_beans 1 0".into(),
        })
        .await
        .expect("give cocoa beans");
    wait_for_slot_stack(&mut client, cocoa_beans_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(log_pos.0, log_pos.1, log_pos.2),
            direction: Direction::East,
            cursor_x: 1.0,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 722,
        })
        .await
        .expect("place cocoa beans on jungle log");
    wait_for_plant_deltas(&mut client, &[(cocoa_pos, cocoa_age0_east.0 as i32)]).await;
}

async fn wait_for_plant_deltas(client: &mut Client, expected: &[((i32, i32, i32), i32)]) {
    let mut expected = expected
        .iter()
        .map(|(pos, state_id)| ExpectedPlantDelta {
            pos: *pos,
            state_id: *state_id,
            seen: false,
        })
        .collect::<Vec<_>>();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while expected.iter().any(|delta| !delta.seen) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("plant lifecycle block delta");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode plant BlockUpdate");
            mark_plant_delta(&mut expected, unpack_block_pos(pkt.position), pkt.state_id);
            continue;
        }
        if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body).expect("decode plant SectionBlocksUpdate");
            mark_plant_section_deltas(&mut expected, pkt);
        }
    }
}

fn mark_plant_section_deltas(expected: &mut [ExpectedPlantDelta], pkt: SectionBlocksUpdate) {
    for delta in expected.iter_mut().filter(|delta| !delta.seen) {
        let section_pos = pack_section_pos(
            delta.pos.0.div_euclid(16),
            delta.pos.1.div_euclid(16),
            delta.pos.2.div_euclid(16),
        );
        if pkt.section_pos != section_pos {
            continue;
        }
        let relative_pos = pack_section_relative_pos(delta.pos.0, delta.pos.1, delta.pos.2);
        delta.seen = pkt
            .changes
            .iter()
            .any(|change| change.relative_pos == relative_pos && change.state_id == delta.state_id);
    }
}

fn mark_plant_delta(expected: &mut [ExpectedPlantDelta], pos: (i32, i32, i32), state_id: i32) {
    if let Some(delta) = expected
        .iter_mut()
        .find(|delta| !delta.seen && delta.pos == pos && delta.state_id == state_id)
    {
        delta.seen = true;
    }
}
