#[tokio::test]
async fn break_block_round_trips_update_ack_relight() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
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

    // The relight path needs the block-light table. Skip the test
    // if it isn't present — the same posture as the M4.f gate.
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!("skipping: {} ({err})", block_light_path.display(),);
            return;
        }
    };

    // Resolve the air state-id and the grass cell we expect to
    // break so the assertions don't hard-code 26.1.2 numerics.
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("air in registry");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M5.f block edit".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M5fTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    // Spawn burst.
    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let event: GameEvent = client.read_typed().await.expect("GameEvent");
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");

    // Wait only for the spawn chunk, then send the edit while the rest
    // of the view-distance window is still streaming. This is the M12
    // responsiveness gate: inbound edits must not sit behind all 441
    // chunks.
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let burst_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !chunks_seen.contains(&(0, 0)) {
        let remaining = burst_deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "spawn chunk stalled after {} chunks: {e}",
                    chunks_seen.len()
                )
            });
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode");
            chunks_seen.insert((pkt.chunk_x, pkt.chunk_z));
        } else if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
        }
        // Stray packets between chunks (keepalive, etc.) ignored.
    }

    // Send the break action against the top block under spawn. For the
    // old flat oracle this is Y=-61; for Solaris-generated worlds the
    // Play position is adaptive (`top + 2`). Sequence
    // = 1 — fresh per-connection counter from the client side.
    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    let sequence: i32 = 1;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence,
        })
        .await
        .expect("send break action");

    // Collect the resulting wire response. The handler emits:
    //   BlockUpdate(pos, air) → LightUpdate × 5 → BlockChangedAck.
    // We allow stray frames (e.g. keepalive) interleaved.
    let mut saw_block_update = false;
    let mut saw_ack = false;
    let mut saw_light_for_origin = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_update && saw_ack && saw_light_for_origin) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "edit response stalled: block_update={saw_block_update}, ack={saw_ack}, \
                     light_for_origin={saw_light_for_origin}: {e}"
                )
            });
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!(
                (px, py, pz),
                (0, target_y, 0),
                "BlockUpdate position must match the broken cell",
            );
            assert_eq!(
                pkt.state_id, air_state_id,
                "BlockUpdate state must be air after break",
            );
            saw_block_update = true;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            assert_eq!(
                pkt.sequence, sequence,
                "ack must echo the action's sequence"
            );
            saw_ack = true;
        } else if frame.id == LightUpdate::ID {
            let mut body = frame.body;
            let pkt = LightUpdate::decode(&mut body).expect("decode LightUpdate");
            // M4.f-style mask invariants — at least the origin chunk
            // must arrive lit-and-shaped.
            const ALL_26: u64 = (1 << 26) - 1;
            let sky_mask = mask_to_u64(&pkt.light.sky_y_mask);
            let empty_sky_mask = mask_to_u64(&pkt.light.empty_sky_y_mask);
            assert_eq!(
                sky_mask | empty_sky_mask,
                ALL_26,
                "LightUpdate sky present+empty must cover all 26 slots for chunk ({}, {})",
                pkt.chunk_x,
                pkt.chunk_z,
            );
            assert_eq!(
                pkt.light.sky_updates.len(),
                sky_mask.count_ones() as usize,
                "LightUpdate sky_updates count must match popcount",
            );
            if (pkt.chunk_x, pkt.chunk_z) == (0, 0) {
                saw_light_for_origin = true;
            }
        }
        // Ignore stray frames (keepalive, etc.) that might land in
        // between the edit response packets.
    }
}

#[tokio::test]
async fn break_block_broadcasts_update_to_second_subscriber() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
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
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!("skipping: {} ({err})", block_light_path.display(),);
            return;
        }
    };
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("air in registry");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M15 two-client block edit".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut actor, sync) = connect_to_play(addr, "M15Actor").await;
    let (mut observer, _) = connect_to_play(addr, "M15Observer").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch actor to creative");

    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    let sequence: i32 = 15;
    actor
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence,
        })
        .await
        .expect("send break action");

    let mut actor_saw_ack = false;
    let mut actor_saw_update = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(actor_saw_ack && actor_saw_update) {
        let frame = actor
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("actor edit response");
        if handle_keepalive(&mut actor, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode actor BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!((px, py, pz), (0, target_y, 0));
            assert_eq!(pkt.state_id, air_state_id);
            actor_saw_update = true;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode actor ack");
            assert_eq!(pkt.sequence, sequence);
            actor_saw_ack = true;
        }
    }

    let mut observer_saw_update = false;
    let mut observer_saw_animation = false;
    let mut observer_saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(observer_saw_update && observer_saw_animation) {
        let frame = observer
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("observer edit response");
        if handle_keepalive(&mut observer, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode observer BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!((px, py, pz), (0, target_y, 0));
            assert_eq!(pkt.state_id, air_state_id);
            observer_saw_update = true;
        } else if frame.id == BlockChangedAck::ID {
            observer_saw_ack = true;
        } else if frame.id == EntityAnimation::ID {
            let mut body = frame.body;
            let pkt = EntityAnimation::decode(&mut body).expect("decode observer animation");
            if pkt.action == EntityAnimationAction::SwingMainHand {
                observer_saw_animation = true;
            }
        }
    }
    assert!(!observer_saw_ack, "observer must not receive actor ack");
}

#[tokio::test]
async fn survival_break_requires_timed_stop_before_mutation() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
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
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("air in registry");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M22 survival mining".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M22SurvivalMiner").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 22,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(&mut client, 22, (0, target_y, 0)).await;

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 23,
        })
        .await
        .expect("send early survival stop break");
    read_ack_without_target_update(&mut client, 23, (0, target_y, 0)).await;

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 24,
        })
        .await
        .expect("send timed survival start break");
    read_ack_without_target_update(&mut client, 24, (0, target_y, 0)).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 25,
        })
        .await
        .expect("send completed survival stop break");

    let mut saw_ack = false;
    let mut saw_update = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_ack && saw_update) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("completed survival break response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode survival BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            if (px, py, pz) == (0, target_y, 0) {
                assert_eq!(pkt.state_id, air_state_id);
                saw_update = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode survival ack");
            if pkt.sequence == 25 {
                saw_ack = true;
            }
        }
    }
}

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
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
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
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M22PickupMiner").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
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
    tokio::time::sleep(Duration::from_millis(250)).await;
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
    let mut slot_stacks = Vec::new();
    let mut saw_slot = false;
    let mut saw_take = false;
    let mut saw_remove = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(dropped_stack.is_some() && saw_slot && saw_take && saw_remove) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drop pickup response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode item AddEntity");
            if pkt.entity_type_id == item_entity_type {
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
                    assert_eq!(stack.count, 1);
                    saw_slot = slot_stacks.iter().any(
                        |slot_stack: &mc_protocol::packets::play::ItemStack| {
                            slot_stack.item_id == stack.item_id && slot_stack.count >= 1
                        },
                    );
                    dropped_stack = Some(stack);
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
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
                assert_eq!(pkt.amount, 1);
                saw_take = true;
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode remove item");
            if let Some(id) = item_entity_id
                && pkt.entity_ids.contains(&id)
            {
                saw_remove = true;
            }
        }
    }
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
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("dirt in registry");
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
    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

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
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M23SurvivalPlace").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
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
    tokio::time::sleep(Duration::from_millis(250)).await;
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
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
    tokio::time::sleep(Duration::from_millis(1_700)).await;
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
