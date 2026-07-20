#[tokio::test]
async fn survival_water_bucket_fills_and_drains_cauldron_with_persistence() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping cauldron bucket test; missing {} or {}",
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
        .map(|block| block.default)
        .expect("air in registry");
    let cauldron_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:cauldron").unwrap())
        .map(|block| block.default)
        .expect("cauldron in registry");
    let water_cauldron_state_id =
        cauldron_state(&blocks, "minecraft:water_cauldron", &[("level", "3")]);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let temp = tempfile::tempdir().expect("create cauldron test world");
    std::fs::create_dir_all(temp.path().join("region")).expect("create region dir");
    let mut storage =
        mc_world::WorldStorage::open_with_capacity(temp.path(), Arc::clone(&blocks), 128)
            .expect("open cauldron test world")
            .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let cauldron_pos = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };
    storage
        .set_block_at(cauldron_pos, cauldron_state_id)
        .expect("seed cauldron")
        .expect("replace terrain with cauldron");
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let water_bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .expect("water bucket item");
    let bucket_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:bucket").unwrap())
        .expect("bucket item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 cauldron bucket persistence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
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

    let (mut client, sync) = connect_to_play(addr, "M100Cauldron").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(
        sync.y.floor() as i32 - 2,
        surface_y,
        "spawn should expose seeded cauldron"
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
            position: pack_block_pos(cauldron_pos.x, cauldron_pos.y, cauldron_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 7101,
        })
        .await
        .expect("fill cauldron");
    wait_for_cauldron_bucket_result(
        &mut client,
        7101,
        cauldron_pos,
        water_cauldron_state_id,
        &[(36, bucket_item_id, 1)],
    )
    .await;
    flush_and_expect_cached_block(&world_handle, cauldron_pos, water_cauldron_state_id).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(cauldron_pos.x, cauldron_pos.y, cauldron_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 7102,
        })
        .await
        .expect("drain cauldron");
    wait_for_cauldron_bucket_result(
        &mut client,
        7102,
        cauldron_pos,
        cauldron_state_id,
        &[(36, water_bucket_item_id, 1)],
    )
    .await;
    flush_and_expect_cached_block(&world_handle, cauldron_pos, cauldron_state_id).await;

    client
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput {
                shift: true,
                ..PlayerInput::default()
            },
        })
        .await
        .expect("start shifting before cauldron bucket use");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(cauldron_pos.x, cauldron_pos.y, cauldron_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 7103,
        })
        .await
        .expect("fill cauldron while shifting");
    wait_for_cauldron_bucket_result(
        &mut client,
        7103,
        cauldron_pos,
        water_cauldron_state_id,
        &[(36, bucket_item_id, 1)],
    )
    .await;
    flush_and_expect_cached_block(&world_handle, cauldron_pos, water_cauldron_state_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bucket 2 0".into(),
        })
        .await
        .expect("give stacked empty buckets");
    wait_for_slot_stack(&mut client, bucket_item_id, 2).await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(cauldron_pos.x, cauldron_pos.y, cauldron_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 7104,
        })
        .await
        .expect("drain cauldron with stacked empty buckets");
    wait_for_cauldron_bucket_result(
        &mut client,
        7104,
        cauldron_pos,
        cauldron_state_id,
        &[(36, bucket_item_id, 1), (9, water_bucket_item_id, 1)],
    )
    .await;
    flush_and_expect_cached_block(&world_handle, cauldron_pos, cauldron_state_id).await;
}

#[test]
fn cauldron_states_survive_disk_flush_and_reopen() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!(
            "skipping cauldron state persistence test; missing {}",
            blocks_json.display()
        );
        return;
    }

    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|block| block.default)
        .expect("air in registry");
    let cauldron_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:cauldron").unwrap())
        .map(|block| block.default)
        .expect("cauldron in registry");
    let water_cauldron_state_id =
        cauldron_state(&blocks, "minecraft:water_cauldron", &[("level", "3")]);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let temp = tempfile::tempdir().expect("create cauldron persistence world");
    std::fs::create_dir_all(temp.path().join("region")).expect("create region dir");
    let mut storage =
        mc_world::WorldStorage::open_with_capacity(temp.path(), Arc::clone(&blocks), 128)
            .expect("open cauldron persistence world")
            .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let cauldron_pos = mc_world::BlockPos {
        x: 0,
        y: surface_y,
        z: 0,
    };

    storage
        .set_block_at(cauldron_pos, water_cauldron_state_id)
        .expect("write water cauldron")
        .expect("replace terrain with water cauldron");
    assert!(storage.flush_dirty().expect("flush water cauldron") >= 1);
    drop(storage);

    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(temp.path(), Arc::clone(&blocks), 128)
            .expect("reopen water cauldron world");
    assert_eq!(
        reopened
            .get_block(cauldron_pos)
            .expect("read reopened water cauldron"),
        Some(water_cauldron_state_id)
    );
    reopened
        .set_block_at(cauldron_pos, cauldron_state_id)
        .expect("write empty cauldron")
        .expect("replace water cauldron with empty cauldron");
    assert!(reopened.flush_dirty().expect("flush empty cauldron") >= 1);
    drop(reopened);

    let mut reopened_empty =
        mc_world::WorldStorage::open_with_capacity(temp.path(), Arc::clone(&blocks), 128)
            .expect("reopen empty cauldron world");
    assert_eq!(
        reopened_empty
            .get_block(cauldron_pos)
            .expect("read reopened empty cauldron"),
        Some(cauldron_state_id)
    );
}

fn cauldron_state(
    blocks: &mc_world::BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> mc_world::BlockStateId {
    let id = mc_data::Identifier::parse(name).unwrap();
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .unwrap_or_else(|| panic!("missing block state {name} {props:?}"))
}

async fn wait_for_cauldron_bucket_result(
    client: &mut Client,
    sequence: i32,
    pos: mc_world::BlockPos,
    expected_state: mc_world::BlockStateId,
    expected_slots: &[(i16, u32, i32)],
) {
    let mut saw_block = false;
    let mut saw_slots = vec![false; expected_slots.len()];
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !(saw_block && saw_slots.iter().all(|seen| *seen) && saw_ack) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for cauldron bucket result: block={saw_block} slots={saw_slots:?} ack={saw_ack}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "cauldron bucket response ended before expected result: block={saw_block} slots={saw_slots:?} ack={saw_ack}: {err}"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode cauldron BlockUpdate");
            if unpack_block_pos(pkt.position) == (pos.x, pos.y, pos.z) {
                assert_eq!(pkt.state_id, expected_state.0 as i32);
                saw_block = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.container_id == 0 {
                for (index, (slot, item_id, count)) in expected_slots.iter().enumerate() {
                    if pkt.slot == *slot
                        && pkt.item_stack.item_id == *item_id
                        && pkt.item_stack.count == *count
                    {
                        saw_slots[index] = true;
                    }
                }
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode cauldron ack");
            if pkt.sequence == sequence {
                assert!(saw_block, "cauldron ack arrived before block update");
                saw_ack = true;
            }
        }
    }
}

async fn flush_and_expect_cached_block(
    world: &tokio::sync::Mutex<mc_world::WorldStorage>,
    pos: mc_world::BlockPos,
    expected_state: mc_world::BlockStateId,
) {
    let mut storage = world.lock().await;
    let flushed = storage.flush_dirty().expect("flush cauldron world");
    assert!(
        flushed >= 1,
        "cauldron edit should dirty at least one chunk"
    );
    assert_eq!(
        storage.get_block(pos).expect("read flushed cauldron"),
        Some(expected_state)
    );
}
