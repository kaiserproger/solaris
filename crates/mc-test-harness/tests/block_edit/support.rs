use super::*;

pub(super) async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    client
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("report grounded spawn pose");
    (client, sync)
}

pub(super) struct WallTorchWireFixture {
    pub(super) client: Client,
    pub(super) clicked: mc_world::BlockPos,
    pub(super) target: mc_world::BlockPos,
    pub(super) air_state: mc_world::BlockStateId,
    pub(super) support_state: mc_world::BlockStateId,
    pub(super) wall_torch_east: mc_world::BlockStateId,
    pub(super) torch_item: u32,
}

pub(super) async fn start_wall_torch_wire_fixture(
    client_name: &str,
    support_block: &str,
) -> Option<WallTorchWireFixture> {
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
        return None;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let support_state = blocks
        .block(&mc_data::Identifier::parse(support_block).expect("valid support block identifier"))
        .unwrap_or_else(|| panic!("missing support block {support_block}"))
        .default;
    let wall_torch_east = blocks
        .by_name_and_props(
            &mc_data::Identifier::parse("minecraft:wall_torch").unwrap(),
            &[("facing".to_string(), "east".to_string())],
        )
        .expect("wall_torch east state");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn terrain");
    let clicked = mc_world::BlockPos {
        x: 2,
        y: surface_y,
        z: 2,
    };
    let target = mc_world::BlockPos { x: 3, ..clicked };
    storage
        .set_block_at(clicked, support_state)
        .expect("seed wall torch support");
    storage
        .set_block_at(target, air_state)
        .expect("clear wall torch target");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let torch_item = items
        .id_of(&mc_data::Identifier::parse("minecraft:torch").unwrap())
        .expect("torch item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "wall torch wire fixture".into(),
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, client_name).await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(surface_y, sync.y.floor() as i32 - 2, "seeded spawn support");
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:torch 1 0".into(),
        })
        .await
        .expect("give torch");
    wait_for_slot_stack(&mut client, torch_item, 1).await;

    Some(WallTorchWireFixture {
        client,
        clicked,
        target,
        air_state,
        support_state,
        wall_torch_east,
        torch_item,
    })
}

pub(super) async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
                return;
            }
        }
    }
}

pub(super) async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}

pub(super) async fn wait_for_world_ticks(client: &mut Client, ticks: i64) {
    let mut baseline = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for simulation ticks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != ClientboundSetTime::ID {
            continue;
        }
        let mut body = frame.body;
        let packet = ClientboundSetTime::decode(&mut body).expect("decode SetTime");
        let start = *baseline.get_or_insert(packet.game_time);
        if packet.game_time.saturating_sub(start) >= ticks {
            return;
        }
    }
}

pub(super) fn vanilla_stop_destroy_ticks(
    destroy_speed: f64,
    item_speed: f64,
    correct_tool_for_drops: bool,
) -> i64 {
    let divisor = if correct_tool_for_drops { 30.0 } else { 100.0 };
    (0.7 * destroy_speed * divisor / item_speed).ceil() as i64
}

pub(super) async fn read_ack_without_target_update(
    client: &mut Client,
    sequence: i32,
    target: (i32, i32, i32),
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("ack before target update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate before ack");
            let pos = unpack_block_pos(pkt.position);
            assert_ne!(
                pos, target,
                "survival break mutated before timed completion"
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

pub(super) async fn wait_for_block_ack(client: &mut Client, sequence: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("block ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_food_level(client: &mut Client, food: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("food level update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if pkt.food == food {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_experience(
    client: &mut Client,
    predicate: impl Fn(&ClientboundSetExperience) -> bool,
) -> ClientboundSetExperience {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("experience update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body).expect("decode SetExperience");
            if predicate(&packet) {
                return packet;
            }
        }
    }
}

pub(super) async fn wait_for_health_level(client: &mut Client, health: f32) {
    wait_for_health_near(client, health, f32::EPSILON).await;
}

pub(super) async fn wait_for_health_near(client: &mut Client, health: f32, tolerance: f32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("health level update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            if (pkt.health - health).abs() <= tolerance {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_death_inventory_and_xp_drop(
    client: &mut Client,
    item_entity_type: i32,
    xp_orb_entity_type: i32,
    item_id: u32,
    count: i32,
    xp_value: i32,
) {
    let mut item_entities = HashSet::new();
    let mut saw_drop_stack = false;
    let mut saw_xp_orb = false;
    let mut saw_inventory_clear = false;
    let mut saw_xp_reset = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_drop_stack && saw_xp_orb && saw_inventory_clear && saw_xp_reset) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("death inventory drop");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode death drop AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entities.insert(pkt.entity_id);
            } else if pkt.entity_type_id == xp_orb_entity_type {
                assert_eq!(
                    pkt.data, xp_value,
                    "death XP orb must carry the recoverable value"
                );
                saw_xp_orb = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode death drop data");
            if item_entities.contains(&pkt.entity_id) {
                saw_drop_stack |= pkt.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == item_id
                                && stack.count == count
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode death inventory clear");
            saw_inventory_clear |= pkt.container_id == 0
                && pkt.items.get(36).is_some_and(|stack| stack.is_empty())
                && pkt.carried_item.is_empty();
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundSetExperience::decode(&mut body).expect("decode death experience reset");
            saw_xp_reset |= pkt.total_experience == 0 && pkt.experience_level == 0;
        }
    }
}

pub(super) async fn wait_for_keep_inventory_death_fence(
    client: &mut Client,
    item_entity_type: i32,
    xp_orb_entity_type: i32,
    item_id: u32,
    count: i32,
    total_xp: i32,
) {
    let mut saw_death_health = false;
    let mut saw_preserved_inventory = false;
    let mut saw_gamerule_feedback = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_death_health && saw_preserved_inventory && saw_gamerule_feedback) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("keepInventory death fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode keepInventory AddEntity");
            assert!(
                packet.entity_type_id != item_entity_type
                    && packet.entity_type_id != xp_orb_entity_type,
                "keepInventory death must not spawn item or XP entities: {packet:?}"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetHealth::decode(&mut body).expect("decode keepInventory health");
            saw_death_health |= packet.health == 0.0;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode keepInventory inventory snapshot");
            if packet.container_id == 0 {
                let stack = packet.items.get(36).expect("hotbar slot 36");
                assert_eq!((stack.item_id, stack.count), (item_id, count));
                assert!(packet.carried_item.is_empty());
                saw_preserved_inventory = true;
            }
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode keepInventory experience");
            assert_eq!(packet.total_experience, total_xp);
        } else if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet = ClientboundSystemChat::decode(&mut body)
                .expect("decode keepInventory gamerule feedback");
            saw_gamerule_feedback |= system_chat_text(&packet) == "keep_inventory = true";
        }
    }
}

pub(super) async fn wait_for_rejoined_keep_inventory_state(
    client: &mut Client,
    item_entity_type: i32,
    xp_orb_entity_type: i32,
    item_id: u32,
    count: i32,
    total_xp: i32,
) {
    let mut saw_inventory = false;
    let mut saw_experience = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_inventory && saw_experience) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejoined keepInventory state");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode rejoined inventory snapshot");
            if packet.container_id == 0 {
                let stack = packet.items.get(36).expect("rejoined hotbar slot 36");
                assert_eq!((stack.item_id, stack.count), (item_id, count));
                assert!(packet.carried_item.is_empty());
                saw_inventory = true;
            }
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode rejoined experience snapshot");
            assert_eq!(packet.total_experience, total_xp);
            saw_experience = true;
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode rejoined AddEntity");
            assert!(
                packet.entity_type_id != item_entity_type
                    && packet.entity_type_id != xp_orb_entity_type,
                "restart must not restore keepInventory death drops: {packet:?}"
            );
        }
    }
}

pub(super) async fn read_ack_without_food_or_slot_change(
    client: &mut Client,
    sequence: i32,
    item_id: u32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("dead use item ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.item_stack.item_id != item_id || pkt.item_stack.count != 1,
                "dead use item must not consume the held food stack"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            assert_ne!(pkt.food, 20, "dead use item must not restore food");
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == sequence {
                return;
            }
        }
    }
}

pub(super) async fn assert_no_food_or_slot_change_until_world_ticks(
    client: &mut Client,
    item_id: u32,
    ticks: i64,
) {
    let mut baseline = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for world ticks");
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for canceled-use world ticks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.item_stack.item_id != item_id || pkt.item_stack.count != 1,
                "canceled use item must not consume the held food stack"
            );
        } else if frame.id == ClientboundSetHealth::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHealth::decode(&mut body).expect("decode SetHealth");
            assert_ne!(pkt.food, 20, "canceled use item must not restore food");
        } else if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let packet = ClientboundSetTime::decode(&mut body).expect("decode SetTime");
            let start = *baseline.get_or_insert(packet.game_time);
            if packet.game_time.saturating_sub(start) >= ticks {
                return;
            }
        }
    }
}

pub(super) async fn wait_for_slot_stack(client: &mut Client, item_id: u32, count: i32) {
    let _ = wait_for_slot_stack_update(client, item_id, count).await;
}

pub(super) async fn wait_for_slot_stack_update(
    client: &mut Client,
    item_id: u32,
    count: i32,
) -> ClientboundContainerSetSlot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("slot stack update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == item_id && pkt.item_stack.count == count {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_container_slot(
    client: &mut Client,
    container_id: i32,
    slot: i16,
    predicate: impl Fn(&mc_protocol::packets::play::ItemStack) -> bool,
) -> ClientboundContainerSetSlot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("container slot update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.container_id == container_id && pkt.slot == slot && predicate(&pkt.item_stack) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_inventory_content(
    client: &mut Client,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("inventory content update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body).expect("decode SetContent");
            if predicate(&pkt) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_open_screen(
    client: &mut Client,
    menu_type: i32,
) -> ClientboundOpenScreen {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("open screen");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            let mut body = frame.body;
            let pkt = ClientboundOpenScreen::decode(&mut body).expect("decode OpenScreen");
            if pkt.menu_type == menu_type {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_furnace_content(
    client: &mut Client,
    container_id: i32,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("furnace content update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode furnace SetContent");
            if pkt.container_id == container_id && predicate(&pkt) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_furnace_data(
    client: &mut Client,
    container_id: i32,
    data_id: i16,
    predicate: impl Fn(i16) -> bool,
) -> ClientboundContainerSetData {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("furnace data update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetData::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetData::decode(&mut body).expect("decode SetData");
            if pkt.container_id == container_id && pkt.id == data_id && predicate(pkt.value) {
                return pkt;
            }
        }
    }
}

pub(super) async fn wait_for_block_update(
    client: &mut Client,
    pos: (i32, i32, i32),
    state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut observed_updates = Vec::new();
    let mut observed_acks = Vec::new();
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "block update: {error}; expected pos={pos:?} state={state_id}, observed updates={observed_updates:?}, acks={observed_acks:?}"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let update_pos = unpack_block_pos(pkt.position);
            if update_pos == pos && pkt.state_id == state_id {
                return;
            }
            if observed_updates.len() == 16 {
                observed_updates.remove(0);
            }
            observed_updates.push((update_pos, pkt.state_id));
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if observed_acks.len() == 16 {
                observed_acks.remove(0);
            }
            observed_acks.push(pkt.sequence);
        }
    }
}

pub(super) fn top_non_air_y(
    world: &mut mc_world::WorldStorage,
    x: i32,
    z: i32,
    air: mc_world::BlockStateId,
) -> Option<i32> {
    // Generated-world fixtures need the same solid, no-leaves surface used by spawn selection.
    (mc_world::MIN_Y..mc_world::MAX_Y).rev().find(|&y| {
        let Some(state_id) = world
            .get_block(mc_world::BlockPos { x, y, z })
            .ok()
            .flatten()
        else {
            return false;
        };
        if state_id == air {
            return false;
        }
        let Some(state) = world.registry().by_id(state_id) else {
            return false;
        };
        !state.block.id.path().ends_with("_leaves")
            && mc_data::collision_shapes::vanilla_collision_shapes()
                .get(state_id.0)
                .is_some_and(|shape| !shape.is_empty())
    })
}

pub(super) fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
