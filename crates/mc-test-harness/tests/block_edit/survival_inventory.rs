#[tokio::test]
async fn survival_break_drops_item_entity_and_picks_it_up() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let stone_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .expect("stone in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let seeded_y =
        top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column has terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 0,
                y: seeded_y,
                z: 0,
            },
            stone_state_id,
        )
        .expect("seed stone target")
        .expect("replace generated top block");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let cobblestone_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:cobblestone").unwrap())
        .expect("cobblestone item");
    let pickaxe_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .expect("iron pickaxe item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .expect("item entity type") as i32;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M22 drops pickup".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::from_maps(
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::from([(
                mc_data::Identifier::parse("minecraft:stone").unwrap(),
                mc_data::Identifier::parse("minecraft:cobblestone").unwrap(),
            )]),
        )),
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
    let runtime_telemetry = bound.runtime_telemetry_handle();
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M22PickupMiner").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:iron_pickaxe 1 0".into(),
        })
        .await
        .expect("give pickaxe");
    wait_for_slot_stack(&mut client, pickaxe_id, 1).await;

    let target_y = sync.y.floor() as i32 - 2;
    assert_eq!(
        target_y, seeded_y,
        "client spawn should expose seeded target"
    );
    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 31,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(&mut client, 31, (0, target_y, 0)).await;
    wait_for_world_ticks(&mut client, 34).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 32,
        })
        .await
        .expect("send survival stop break");

    let mut item_entity_id = None;
    let mut dropped_stack = None;
    let mut drop_visible_at = None;
    let mut slot_stacks = Vec::new();
    let mut saw_break_update = false;
    let mut saw_break_ack = false;
    let mut tool_damage_updates = 0;
    let mut saw_slot = false;
    let mut saw_take = false;
    let mut saw_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_break_update
        && saw_break_ack
        && tool_damage_updates == 1
        && dropped_stack.is_some()
        && saw_slot
        && saw_take
        && saw_remove)
    {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drop pickup response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode survival BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            if (px, py, pz) == (0, target_y, 0) {
                saw_break_update = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode survival ack");
            if pkt.sequence == 32 {
                saw_break_ack = true;
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                assert!(saw_break_update, "item spawned before break block update");
                assert!(saw_break_ack, "item spawned before break ack");
                assert_eq!(
                    tool_damage_updates, 1,
                    "item spawned before the atomic tool durability update"
                );
                item_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode entity data");
            if Some(pkt.entity_id) == item_entity_id {
                let stack = pkt.values.iter().find_map(|value| match value {
                    EntityDataValue::ItemStack { index, stack }
                        if *index == ITEM_ENTITY_DATA_ITEM_INDEX =>
                    {
                        Some(stack.clone())
                    }
                    _ => None,
                });
                if let Some(stack) = stack {
                    assert_eq!(stack.item_id, cobblestone_item_id);
                    assert_eq!(stack.count, 1);
                    saw_slot = slot_stacks.iter().any(
                        |slot_stack: &mc_protocol::packets::play::ItemStack| {
                            slot_stack.item_id == stack.item_id && slot_stack.count >= 1
                        },
                    );
                    dropped_stack = Some(stack);
                    drop_visible_at.get_or_insert_with(tokio::time::Instant::now);
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36
                && pkt.item_stack.item_id == pickaxe_id
                && pkt.item_stack.count == 1
                && pkt.item_stack.damage == Some(1)
            {
                tool_damage_updates += 1;
                assert_eq!(
                    tool_damage_updates, 1,
                    "timed break damaged the held tool more than once"
                );
            }
            slot_stacks.push(pkt.item_stack.clone());
            if let Some(stack) = &dropped_stack
                && pkt.item_stack.item_id == stack.item_id
                && pkt.item_stack.count >= 1
            {
                saw_slot = true;
            }
        } else if frame.id == ClientboundTakeItemEntity::ID {
            let mut body = frame.body;
            let pkt = ClientboundTakeItemEntity::decode(&mut body).expect("decode take item");
            if Some(pkt.item_entity_id) == item_entity_id {
                let visible_for = drop_visible_at
                    .expect("pickup animation arrived before item metadata")
                    .elapsed();
                assert!(
                    visible_for >= Duration::from_millis(100),
                    "item pickup arrived before visible window: {visible_for:?}"
                );
                assert_eq!(pkt.amount, 1);
                saw_take = true;
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode remove item");
            if let Some(id) = item_entity_id
                && pkt.entity_ids.contains(&id)
            {
                let visible_for = drop_visible_at
                    .expect("item removal arrived before item metadata")
                    .elapsed();
                assert!(
                    visible_for >= Duration::from_millis(100),
                    "item removal arrived before visible window: {visible_for:?}"
                );
                saw_remove = true;
            }
        }
    }
    let simulation = runtime_telemetry.snapshot();
    eprintln!(
        "Prompt 03 pickup queue capacity={} enqueued={} processed={} block_edits={} item_pickups={} depth={} max_depth={} max_batch={}",
        simulation.simulation_queue_capacity,
        simulation.simulation_commands_enqueued,
        simulation.simulation_commands_processed,
        simulation.simulation_block_edits_processed,
        simulation.simulation_item_pickups_processed,
        simulation.simulation_queue_depth,
        simulation.simulation_queue_max_depth,
        simulation.simulation_max_batch,
    );
    assert_eq!(simulation.simulation_queue_capacity, 1024);
    assert_eq!(tool_damage_updates, 1);
    assert_eq!(simulation.simulation_block_edits_processed, 1);
    assert_eq!(simulation.simulation_item_pickups_processed, 1);
    assert!(simulation.simulation_commands_enqueued >= 1);
    assert!(simulation.simulation_commands_processed >= 1);
    assert_eq!(simulation.simulation_queue_depth, 0);
    assert!(
        (1..=simulation.simulation_queue_capacity).contains(&simulation.simulation_queue_max_depth)
    );
    assert!((1..=simulation.simulation_queue_max_depth).contains(&simulation.simulation_max_batch));
    assert_eq!(simulation.simulation_commands_rejected_full, 0);
    assert_eq!(simulation.simulation_commands_rejected_closed, 0);
    assert_eq!(simulation.simulation_commands_rejected_shutdown, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_busy, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_unavailable, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_mutation, 0);
    assert_eq!(simulation.simulation_commands_rejected_stale_session, 0);
    assert_eq!(simulation.simulation_commands_cancelled, 0);
}

#[tokio::test]
async fn survival_can_place_naturally_picked_up_block() {
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
        .map(|block| block.default)
        .expect("air in registry");
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|block| block.default)
        .expect("dirt in registry");
    let dirt_state_id = dirt_state.0 as i32;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let target_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn column terrain");
    storage
        .set_block_at(mc_world::BlockPos { x: 0, y: target_y, z: 0 }, dirt_state)
        .expect("seed dirt target")
        .expect("replace generated surface block");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 survival place pickup".into(),
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

    let (mut client, _sync) = connect_to_play(addr, "M23SurvivalPlace").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 81,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(&mut client, 81, (0, target_y, 0)).await;
    wait_for_world_ticks(&mut client, vanilla_stop_destroy_ticks(0.5, 1.0, true)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 82,
        })
        .await
        .expect("send survival stop break");
    wait_for_slot_stack(&mut client, dirt_item_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, target_y - 1, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 83,
        })
        .await
        .expect("send survival placement");

    let mut saw_block_update = false;
    let mut saw_ack = false;
    let mut saw_decrement = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_update && saw_ack && saw_decrement) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("survival placement response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode placement BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            if (px, py, pz) == (0, target_y, 0) {
                assert_eq!(pkt.state_id, dirt_state_id);
                saw_block_update = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode placement ack");
            if pkt.sequence == 83 {
                saw_ack = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36 && pkt.item_stack.is_empty() {
                saw_decrement = true;
            }
        }
    }
}

#[tokio::test]
async fn invalid_carried_item_slot_does_not_change_survival_placement_slot() {
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
        .map(|block| block.default)
        .expect("air in registry");
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|block| block.default)
        .expect("dirt in registry");
    let dirt_state_id = dirt_state.0 as i32;
    let stone_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("stone in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let target_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn column terrain");
    storage
        .set_block_at(mc_world::BlockPos { x: 0, y: target_y, z: 0 }, dirt_state)
        .expect("seed dirt target")
        .expect("replace generated surface block");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let stone_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .expect("stone item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(
        mc_data::entity_types::EntityTypeRegistry::try_from_report_26_1_2(&entity_report)
            .expect("exact 26.1.2 entity registry"),
    );

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 invalid carried slot".into(),
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

    let (mut client, _sync) = connect_to_play(addr, "M100BadHotbar").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 91,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(&mut client, 91, (0, target_y, 0)).await;
    wait_for_world_ticks(&mut client, vanilla_stop_destroy_ticks(0.5, 1.0, true)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 92,
        })
        .await
        .expect("send survival stop break");
    wait_for_slot_stack(&mut client, dirt_item_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stone 1 8".into(),
        })
        .await
        .expect("give stone in hotbar slot 8");
    wait_for_slot_stack(&mut client, stone_item_id, 1).await;

    client
        .write_packet(&ServerboundSetCarriedItem { slot: 99 })
        .await
        .expect("send invalid carried slot");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, target_y - 1, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 93,
        })
        .await
        .expect("send survival placement after invalid slot");

    let mut saw_block_update = false;
    let mut saw_ack = false;
    let mut saw_dirt_decrement = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_update && saw_ack && saw_dirt_decrement) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("invalid slot placement response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode placement BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            if (px, py, pz) == (0, target_y, 0) {
                assert_eq!(
                    pkt.state_id, dirt_state_id,
                    "invalid carried slot must preserve the prior selected dirt slot, not select stone ({stone_state_id})",
                );
                saw_block_update = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode placement ack");
            if pkt.sequence == 93 {
                saw_ack = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            assert!(
                pkt.slot != 44 || !pkt.item_stack.is_empty(),
                "invalid carried slot must not consume hotbar slot 8"
            );
            if pkt.slot == 36 && pkt.item_stack.is_empty() {
                saw_dirt_decrement = true;
            }
        }
    }
}

#[tokio::test]
async fn survival_break_damages_held_tool() {
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
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let pickaxe_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .expect("iron pickaxe item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 durability".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M23ToolMiner").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:iron_pickaxe 1 0".into(),
        })
        .await
        .expect("give pickaxe");
    wait_for_slot_stack(&mut client, pickaxe_id, 1).await;

    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 51,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(&mut client, 51, (0, target_y, 0)).await;
    wait_for_world_ticks(&mut client, 34).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 52,
        })
        .await
        .expect("send survival stop break");

    let mut saw_ack = false;
    let mut saw_damage = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_ack && saw_damage) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("durability break response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36
                && pkt.item_stack.item_id == pickaxe_id
                && pkt.item_stack.count == 1
                && pkt.item_stack.damage == Some(1)
            {
                saw_damage = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            if pkt.sequence == 52 {
                saw_ack = true;
            }
        }
    }
}

#[tokio::test]
async fn survival_hoe_use_tills_dirt_and_damages_tool() {
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let farmland_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:farmland").unwrap())
        .map(|b| b.default)
        .expect("farmland in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let surface_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 0,
                y: surface_y,
                z: 0,
            },
            dirt_state_id,
        )
        .expect("seed dirt")
        .expect("replace generated surface");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let wooden_hoe_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:wooden_hoe").unwrap())
        .expect("wooden hoe item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "P2 hoe tilling".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
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

    let (mut client, sync) = connect_to_play(addr, "P2HoeTiller").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:wooden_hoe 1 0".into(),
        })
        .await
        .expect("give wooden hoe");
    wait_for_slot_stack(&mut client, wooden_hoe_id, 1).await;

    let target_y = sync.y.floor() as i32 - 2;
    assert_eq!(target_y, surface_y, "spawn target should be seeded dirt");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, target_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 121,
        })
        .await
        .expect("use hoe on dirt");

    let mut saw_farmland = false;
    let mut saw_ack = false;
    let mut saw_damage = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_farmland && saw_ack && saw_damage) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("hoe tilling response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode till BlockUpdate");
            if unpack_block_pos(pkt.position) == (0, target_y, 0) {
                assert_eq!(pkt.state_id, farmland_state_id.0 as i32);
                saw_farmland = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode till ack");
            if pkt.sequence == 121 {
                saw_ack = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36
                && pkt.item_stack.item_id == wooden_hoe_id
                && pkt.item_stack.count == 1
                && pkt.item_stack.damage == Some(1)
            {
                saw_damage = true;
            }
        }
    }
}
