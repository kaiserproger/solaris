#[tokio::test]
#[ignore = "requires local data/vanilla sidecars"]
async fn bonemeal_growth_debits_only_successful_survival_use() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        panic!(
            "prerequisite failed: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
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
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let farmland = crop_test_state(&blocks, "minecraft:farmland", &[]);
    let wheat_age0 = crop_test_state(&blocks, "minecraft:wheat", &[("age", "0")]);
    let wheat_age1 = crop_test_state(&blocks, "minecraft:wheat", &[("age", "1")]);
    let wheat_age7 = crop_test_state(&blocks, "minecraft:wheat", &[("age", "7")]);
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M57 crop bonemeal".into(),
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
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M57CropMeal").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let young_pos = (0, support_y + 1, 2);
    let mature_pos = (1, support_y + 1, 2);
    let creative_pos = (2, support_y + 1, 2);
    {
        let mut storage = world.lock().await;
        crop_test_set(&mut storage, (0, support_y, 2), farmland);
        crop_test_set(&mut storage, young_pos, wheat_age0);
        crop_test_set(&mut storage, (1, support_y, 2), farmland);
        crop_test_set(&mut storage, mature_pos, wheat_age7);
        crop_test_set(&mut storage, (2, support_y, 2), farmland);
        crop_test_set(&mut storage, creative_pos, wheat_age0);
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bone_meal 2 0".into(),
        })
        .await
        .expect("give bone meal");
    wait_for_slot_stack(&mut client, bone_meal_item_id, 2).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(young_pos.0, young_pos.1, young_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 201,
        })
        .await
        .expect("bonemeal young wheat");
    wait_for_block_update(&mut client, young_pos, wheat_age1.0 as i32).await;
    wait_for_slot_stack(&mut client, bone_meal_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(mature_pos.0, mature_pos.1, mature_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 202,
        })
        .await
        .expect("bonemeal mature wheat");
    wait_for_mature_bonemeal_noop(&mut client, mature_pos, 202).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(creative_pos.0, creative_pos.1, creative_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 203,
        })
        .await
        .expect("bonemeal wheat in creative");
    wait_for_block_update(&mut client, creative_pos, wheat_age1.0 as i32).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:bone_meal 1".into(),
        })
        .await
        .expect("probe creative-preserved bone meal stack");
    wait_for_slot_stack(&mut client, bone_meal_item_id, 2).await;
}

fn crop_test_state(
    blocks: &mc_world::BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> mc_world::BlockStateId {
    let id = mc_data::Identifier::parse(name).expect("static identifier");
    if props.is_empty() {
        return blocks
            .block(&id)
            .unwrap_or_else(|| panic!("missing block {name}"))
            .default;
    }
    let props = props
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .unwrap_or_else(|| panic!("missing block state {name} {props:?}"))
}

fn crop_test_set(
    storage: &mut mc_world::WorldStorage,
    pos: (i32, i32, i32),
    state: mc_world::BlockStateId,
) {
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            state,
        )
        .expect("crop fixture block edit succeeds");
}

async fn wait_for_mature_bonemeal_noop(client: &mut Client, pos: (i32, i32, i32), sequence: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("mature bonemeal ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode mature bonemeal BlockUpdate");
            assert_ne!(
                unpack_block_pos(pkt.position),
                pos,
                "mature crop must not grow when bonemealed"
            );
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode mature bonemeal SetSlot");
            assert_ne!(
                pkt.slot, 36,
                "mature crop bonemeal must not consume the held stack"
            );
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode mature bonemeal ack");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}
