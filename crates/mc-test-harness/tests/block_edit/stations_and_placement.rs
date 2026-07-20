#[tokio::test]
async fn station_noop_and_creative_placement_preserve_inventory() {
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
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let station_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:smithing_table").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("smithing table in registry");
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let dirt_state_id = dirt_state.0 as i32;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn terrain");
    let placement_columns = [(2, 2), (3, 2), (2, 3)];
    for (x, z) in placement_columns {
        storage
            .set_block_at(mc_world::BlockPos { x, y: surface_y, z }, dirt_state)
            .expect("seed station placement support");
        storage
            .set_block_at(
                mc_world::BlockPos {
                    x,
                    y: surface_y + 1,
                    z,
                },
                air_state,
            )
            .expect("clear station placement target");
    }
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let station_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:smithing_table").unwrap())
        .expect("smithing table item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M87 station no-op".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M87StationNoop").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:smithing_table 1 0".into(),
        })
        .await
        .expect("give smithing table");
    wait_for_slot_stack(&mut client, station_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    assert_eq!(support_y, surface_y, "seeded spawn terrain");
    let station_y = support_y + 1;
    let (station_x, station_z) = placement_columns[0];
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(station_x, support_y, station_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 701,
        })
        .await
        .expect("place smithing table");
    wait_for_block_update(
        &mut client,
        (station_x, station_y, station_z),
        station_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(station_x, station_y, station_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 702,
        })
        .await
        .expect("use unsupported station");
    read_station_noop_ack(
        &mut client,
        702,
        (station_x, station_y + 1, station_z),
        dirt_state_id,
    )
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    for (sequence, (x, z)) in [(703, placement_columns[1]), (704, placement_columns[2])] {
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(x, support_y, z),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence,
            })
            .await
            .expect("place creative dirt");
        wait_for_block_update(&mut client, (x, station_y, z), dirt_state_id).await;
    }
}

async fn read_station_noop_ack(
    client: &mut Client,
    sequence: i32,
    fallthrough_pos: (i32, i32, i32),
    fallthrough_state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("station no-op ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            panic!("unsupported station must not open an unimplemented menu");
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            assert!(
                unpack_block_pos(pkt.position) != fallthrough_pos
                    || pkt.state_id != fallthrough_state_id,
                "unsupported station use fell through into adjacent block placement"
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

#[tokio::test]
async fn placing_torch_on_a_wall_publishes_exact_state_then_ack_then_one_debit() {
    let Some(WallTorchWireFixture {
        mut client,
        clicked,
        target,
        wall_torch_east,
        ..
    }) = start_wall_torch_wire_fixture("WallTorchPlace", "minecraft:dirt").await
    else {
        return;
    };

    let sequence = 2501;
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
        .expect("place wall torch");

    let mut target_updates = 0;
    let mut saw_ack = false;
    let mut debits = 0;
    while !(target_updates == 1 && saw_ack && debits == 1) {
        let frame = client
            .read_frame_with_timeout(Duration::from_secs(30))
            .await
            .expect("wall torch placement response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode torch BlockUpdate");
            if unpack_block_pos(packet.position) == (target.x, target.y, target.z) {
                assert_eq!(packet.state_id, wall_torch_east.0 as i32);
                target_updates += 1;
                assert_eq!(
                    target_updates, 1,
                    "torch placement publishes one target update"
                );
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode torch ack");
            if packet.sequence == sequence {
                assert_eq!(target_updates, 1, "target update precedes torch ack");
                assert_eq!(debits, 0, "torch debit follows its acknowledgement");
                saw_ack = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet =
                ClientboundContainerSetSlot::decode(&mut body).expect("decode torch debit");
            if packet.container_id == 0 && packet.slot == 36 && packet.item_stack.is_empty() {
                assert!(saw_ack, "torch debit follows acknowledgement");
                debits += 1;
                assert_eq!(debits, 1, "torch placement debits exactly once");
            }
        }
    }
}
