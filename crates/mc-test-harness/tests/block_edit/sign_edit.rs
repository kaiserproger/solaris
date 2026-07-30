#[tokio::test]
#[ignore = "requires local data/vanilla sidecars"]
async fn survival_places_sign_and_updates_plain_text() {
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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let oak_sign_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_sign").unwrap())
        .expect("oak sign item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M74 sign edit".into(),
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
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M74SignEdit").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_sign 1 0".into(),
        })
        .await
        .expect("give sign");
    wait_for_slot_stack(&mut client, oak_sign_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let sign_pos = (0, support_y + 1, 0);
    let packed_sign_pos = pack_block_pos(sign_pos.0, sign_pos.1, sign_pos.2);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 74,
        })
        .await
        .expect("place sign");
    wait_for_open_sign_editor(&mut client, packed_sign_pos).await;

    let mismatched_pos = pack_block_pos(sign_pos.0, sign_pos.1, sign_pos.2 + 1);
    client
        .write_packet(&ServerboundSignUpdate {
            position: mismatched_pos,
            lines: ["bad", "position", "must", "skip"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            is_front_text: true,
        })
        .await
        .expect("send mismatched sign update");
    assert_no_sign_block_entity_data(&mut client, mismatched_pos).await;

    client
        .write_packet(&ServerboundSignUpdate {
            position: packed_sign_pos,
            lines: ["bad", "side", "must", "skip"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            is_front_text: false,
        })
        .await
        .expect("send wrong-side sign update");
    assert_no_sign_block_entity_data(&mut client, packed_sign_pos).await;

    let lines = ["Solaris", "M74", "plain", "text"];
    client
        .write_packet(&ServerboundSignUpdate {
            position: packed_sign_pos,
            lines: lines.iter().map(ToString::to_string).collect(),
            is_front_text: true,
        })
        .await
        .expect("update sign text");

    let update = wait_for_sign_block_entity_data(&mut client, packed_sign_pos).await;
    assert_plain_sign_text(&update.nbt, "front_text", &lines);
    assert_plain_sign_text(&update.nbt, "back_text", &["", "", "", ""]);
}

#[tokio::test]
async fn stale_sign_editor_cannot_edit_replaced_sign() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let oak_sign_id = embedded_item_id(&data, "minecraft:oak_sign");
    let mut storage = embedded_world(&data);
    let support_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn column terrain");
    let mut cfg = embedded_playable_config(&data, storage, "stale sign editor fencing");
    let world = Arc::clone(cfg.world.as_ref().expect("embedded world"));
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true);
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "StaleSignEditor").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_sign 1 0".into(),
        })
        .await
        .expect("give sign");
    wait_for_slot_stack(&mut client, oak_sign_id, 1).await;

    let sign_pos = mc_world::BlockPos {
        x: 0,
        y: support_y + 1,
        z: 0,
    };
    let packed_sign_pos = pack_block_pos(sign_pos.x, sign_pos.y, sign_pos.z);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 175,
        })
        .await
        .expect("place original sign");
    wait_for_open_sign_editor(&mut client, packed_sign_pos).await;

    {
        let mut storage = world.lock().await;
        let sign_state = storage
            .get_block(sign_pos)
            .expect("read original sign")
            .expect("original sign exists");
        storage
            .set_block_at(sign_pos, air_state)
            .expect("break original sign")
            .expect("original sign was present");
        storage
            .set_block_at(sign_pos, sign_state)
            .expect("place replacement sign")
            .expect("air was present");
    }

    client
        .write_packet(&ServerboundSignUpdate {
            position: packed_sign_pos,
            lines: ["stale", "editor", "must", "fail"]
                .iter()
                .map(ToString::to_string)
                .collect(),
            is_front_text: true,
        })
        .await
        .expect("send stale sign update");
    assert_no_sign_block_entity_data(&mut client, packed_sign_pos).await;
}

#[tokio::test]
#[ignore = "requires local data/vanilla sidecars"]
async fn survival_sign_text_survives_flush_and_reopen() {
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
    let tmp_world = tempfile::tempdir().expect("temp world");
    std::fs::create_dir_all(tmp_world.path().join("region")).expect("region dir");
    let storage = mc_world::WorldStorage::open_with_capacity(
        tmp_world.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("disk-backed world")
    .with_generator(generator);
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let oak_sign_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_sign").unwrap())
        .expect("oak sign item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 sign restart persistence".into(),
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
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

    let (mut client, sync) = connect_to_play(addr, "M100SignRestart").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_sign 1 0".into(),
        })
        .await
        .expect("give sign");
    wait_for_slot_stack(&mut client, oak_sign_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let sign_pos = (0, support_y + 1, 0);
    let packed_sign_pos = pack_block_pos(sign_pos.0, sign_pos.1, sign_pos.2);
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 174,
        })
        .await
        .expect("place sign");
    wait_for_open_sign_editor(&mut client, packed_sign_pos).await;

    let lines = ["Solaris", "M100", "restart", "sign"];
    client
        .write_packet(&ServerboundSignUpdate {
            position: packed_sign_pos,
            lines: lines.iter().map(ToString::to_string).collect(),
            is_front_text: true,
        })
        .await
        .expect("update sign text");
    let update = wait_for_sign_block_entity_data(&mut client, packed_sign_pos).await;
    assert_plain_sign_text(&update.nbt, "front_text", &lines);

    {
        let mut storage = world.lock().await;
        let flushed = storage.flush_dirty().expect("flush dirty chunks");
        assert!(
            flushed >= 1,
            "expected sign edit to flush at least one dirty chunk, flushed {flushed}"
        );
    }

    let cpos = mc_world::ChunkPos {
        x: sign_pos.0.div_euclid(16),
        z: sign_pos.2.div_euclid(16),
    };
    let block_pos = mc_world::BlockPos {
        x: sign_pos.0,
        y: sign_pos.1,
        z: sign_pos.2,
    };
    let mut reopened =
        mc_world::WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&blocks), 4)
            .expect("reopen");
    let chunk = reopened
        .get_chunk(cpos)
        .expect("load chunk")
        .expect("chunk persisted");
    let bytes = chunk
        .block_entities
        .get(&block_pos)
        .expect("persisted sign block entity");
    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let persisted = mc_nbt::read_network(&mut cursor).expect("decode persisted sign entity");
    assert_eq!(
        compound_field(&persisted, "id"),
        Some(&mc_nbt::Tag::String("minecraft:sign".into()))
    );
    assert_eq!(
        compound_field(&persisted, "x"),
        Some(&mc_nbt::Tag::Int(sign_pos.0))
    );
    assert_eq!(
        compound_field(&persisted, "y"),
        Some(&mc_nbt::Tag::Int(sign_pos.1))
    );
    assert_eq!(
        compound_field(&persisted, "z"),
        Some(&mc_nbt::Tag::Int(sign_pos.2))
    );
    assert_plain_sign_text(&persisted, "front_text", &lines);
    assert_plain_sign_text(&persisted, "back_text", &["", "", "", ""]);

    let wire_records = mc_world::wire::client_block_entities(
        chunk,
        &blocks,
        &mc_data::items::ItemRegistry::default(),
    );
    let sign_record = wire_records
        .iter()
        .find(|record| record.pos == block_pos)
        .expect("persisted sign emits client block entity data");
    assert_eq!(
        sign_record.type_name,
        mc_data::Identifier::parse("minecraft:sign").unwrap()
    );
    assert_eq!(compound_field(&sign_record.nbt, "id"), None);
    assert_eq!(compound_field(&sign_record.nbt, "x"), None);
    assert_plain_sign_text(&sign_record.nbt, "front_text", &lines);
}

async fn wait_for_open_sign_editor(
    client: &mut Client,
    position: i64,
) -> ClientboundOpenSignEditor {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("open sign editor");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenSignEditor::ID {
            let mut body = frame.body;
            let pkt = ClientboundOpenSignEditor::decode(&mut body).expect("decode OpenSignEditor");
            if pkt.position == position {
                assert!(pkt.is_front_text, "new sign should edit front text");
                return pkt;
            }
        }
    }
}

async fn wait_for_sign_block_entity_data(
    client: &mut Client,
    position: i64,
) -> ClientboundBlockEntityData {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("sign block entity update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundBlockEntityData::decode(&mut body).expect("decode BlockEntityData");
            if pkt.position == position {
                return pkt;
            }
        }
    }
}

async fn assert_no_sign_block_entity_data(client: &mut Client, position: i64) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "time set 1000".into(),
        })
        .await
        .expect("send rejected sign packet fence");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected sign packet fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let _time =
                ClientboundSetTime::decode(&mut body).expect("decode rejected sign packet fence");
            return;
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode rejected BlockEntityData");
            assert_ne!(
                pkt.position, position,
                "mismatched sign update must not emit block entity data"
            );
        }
    }
}

fn assert_plain_sign_text(nbt: &mc_nbt::Tag, side: &str, expected: &[&str; 4]) {
    let side = compound_field(nbt, side).expect("sign side text compound");
    let messages = compound_field(side, "messages").expect("sign messages list");
    let mc_nbt::Tag::List(messages) = messages else {
        panic!("sign messages must be a list");
    };
    assert_eq!(messages.element_type, mc_nbt::tag_type::STRING);
    assert_eq!(messages.elements.len(), expected.len());
    for (actual, expected) in messages.elements.iter().zip(expected.iter()) {
        assert_eq!(actual, &mc_nbt::Tag::String((*expected).to_string()));
    }
    assert_eq!(
        compound_field(side, "color"),
        Some(&mc_nbt::Tag::String("black".into()))
    );
    assert_eq!(
        compound_field(side, "has_glowing_text"),
        Some(&mc_nbt::Tag::Byte(0))
    );
}

fn compound_field<'a>(tag: &'a mc_nbt::Tag, name: &str) -> Option<&'a mc_nbt::Tag> {
    let mc_nbt::Tag::Compound(fields) = tag else {
        return None;
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
}
