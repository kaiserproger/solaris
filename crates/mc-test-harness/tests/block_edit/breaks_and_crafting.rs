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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
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
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands = client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition = client
        .read_typed()
        .await
        .expect("SetDefaultSpawnPosition");
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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
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
    let mut observer_saw_break_event = false;
    let mut observer_saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(observer_saw_update && observer_saw_animation && observer_saw_break_event) {
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
        } else if frame.id == LevelEvent::ID {
            let mut body = frame.body;
            let pkt = LevelEvent::decode(&mut body).expect("decode observer level event");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!((px, py, pz), (0, target_y, 0));
            assert_eq!(pkt.event_id, 2001);
            assert_ne!(pkt.data, air_state_id);
            assert!(!pkt.global);
            observer_saw_break_event = true;
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
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let air_state_id = air_state.0 as i32;
    let stone_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .expect("stone in registry");
    let stone_state_id = stone_state.0 as i32;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let seeded_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn column has terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 0,
                y: seeded_y,
                z: 0,
            },
            stone_state,
        )
        .expect("seed stone target")
        .expect("replace generated top block");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M22SurvivalMiner").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
    assert_eq!(target_y, seeded_y, "spawn should expose seeded stone target");
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
    read_rejected_break_resync_before_ack(&mut client, 23, (0, target_y, 0), stone_state_id).await;

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
    wait_for_world_ticks(
        &mut client,
        vanilla_stop_destroy_ticks(1.5, 1.0, false),
    )
    .await;
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
async fn stale_survival_break_cannot_break_peer_replacement() {
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
    let stone_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|block| block.default)
        .expect("stone in registry");
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|block| block.default)
        .expect("dirt in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let target_y = top_non_air_y(&mut storage, 0, 0, air_state).expect("spawn terrain");
    let target = mc_world::BlockPos {
        x: 0,
        y: target_y,
        z: 0,
    };
    storage
        .set_block_at(target, stone_state)
        .expect("seed stone target")
        .expect("replace generated top block");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
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
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "Prompt 02 stale block break".into(),
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

    let (mut miner, miner_sync) = connect_to_play(addr, "Prompt02Miner").await;
    let (mut peer, peer_sync) = connect_to_play(addr, "Prompt02Peer").await;
    drain_until_chunk(&mut miner, (0, 0)).await;
    drain_until_chunk(&mut peer, (0, 0)).await;
    assert_eq!(miner_sync.y.floor() as i32 - 2, target_y);
    assert_eq!(peer_sync.y.floor() as i32 - 2, target_y);

    peer.write_packet(&ServerboundChatCommand {
        command: "gamemode creative".into(),
    })
    .await
    .expect("make peer creative");
    peer.write_packet(&ServerboundChatCommand {
        command: "debug give minecraft:dirt 1 0".into(),
    })
    .await
    .expect("give peer replacement block");
    wait_for_slot_stack(&mut peer, dirt_item_id, 1).await;

    let target_pos = pack_block_pos(0, target_y, 0);
    miner
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 300,
        })
        .await
        .expect("start survival break");
    read_ack_without_target_update(&mut miner, 300, (0, target_y, 0)).await;

    peer.write_packet(&ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: target_pos,
        direction: Direction::Up,
        sequence: 301,
    })
    .await
    .expect("peer breaks original target");
    wait_for_block_state_and_ack(&mut peer, 301, (0, target_y, 0), air_state.0 as i32).await;

    peer.write_packet(&ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(0, target_y - 1, 0),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 302,
    })
    .await
    .expect("peer places replacement target");
    wait_for_block_state_and_ack(&mut peer, 302, (0, target_y, 0), dirt_state.0 as i32).await;
    wait_for_block_state(&mut miner, (0, target_y, 0), dirt_state.0 as i32).await;

    wait_for_world_ticks(&mut miner, 32).await;
    miner
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 303,
        })
        .await
        .expect("stop stale survival break");
    wait_for_stale_break_resync(
        &mut miner,
        303,
        (0, target_y, 0),
        dirt_state.0 as i32,
    )
    .await;

    let final_state = world
        .lock()
        .await
        .get_block(target)
        .expect("read final target");
    assert_eq!(
        final_state,
        Some(dirt_state),
        "stale mining completion must preserve the peer replacement"
    );

    peer.write_packet(&ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: target_pos,
        direction: Direction::Up,
        sequence: 304,
    })
    .await
    .expect("peer clears first replacement");
    wait_for_block_state_and_ack(&mut peer, 304, (0, target_y, 0), air_state.0 as i32).await;
    peer.write_packet(&ServerboundChatCommand {
        command: "debug give minecraft:stone 1 0".into(),
    })
    .await
    .expect("give peer stone setup block");
    wait_for_slot_stack(&mut peer, stone_item_id, 1).await;
    peer.write_packet(&ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(0, target_y - 1, 0),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 305,
    })
    .await
    .expect("peer restores stone before ABA test");
    wait_for_block_state_and_ack(&mut peer, 305, (0, target_y, 0), stone_state.0 as i32).await;
    wait_for_block_state(&mut miner, (0, target_y, 0), stone_state.0 as i32).await;

    miner
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 306,
        })
        .await
        .expect("start ABA survival break");
    read_ack_without_target_update(&mut miner, 306, (0, target_y, 0)).await;

    peer.write_packet(&ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: target_pos,
        direction: Direction::Up,
        sequence: 307,
    })
    .await
    .expect("peer starts ABA transition");
    wait_for_block_state_and_ack(&mut peer, 307, (0, target_y, 0), air_state.0 as i32).await;
    peer.write_packet(&ServerboundChatCommand {
        command: "debug give minecraft:dirt 1 0".into(),
    })
    .await
    .expect("give peer ABA dirt");
    wait_for_slot_stack(&mut peer, dirt_item_id, 1).await;
    peer.write_packet(&ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(0, target_y - 1, 0),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 308,
    })
    .await
    .expect("peer places ABA dirt");
    wait_for_block_state_and_ack(&mut peer, 308, (0, target_y, 0), dirt_state.0 as i32).await;
    peer.write_packet(&ServerboundPlayerAction {
        action: PlayerActionKind::StartDestroyBlock,
        position: target_pos,
        direction: Direction::Up,
        sequence: 309,
    })
    .await
    .expect("peer clears ABA dirt");
    wait_for_block_state_and_ack(&mut peer, 309, (0, target_y, 0), air_state.0 as i32).await;
    peer.write_packet(&ServerboundChatCommand {
        command: "debug give minecraft:stone 1 0".into(),
    })
    .await
    .expect("give peer ABA stone");
    wait_for_slot_stack(&mut peer, stone_item_id, 1).await;
    peer.write_packet(&ServerboundUseItemOn {
        hand: InteractionHand::MainHand,
        position: pack_block_pos(0, target_y - 1, 0),
        direction: Direction::Up,
        cursor_x: 0.5,
        cursor_y: 1.0,
        cursor_z: 0.5,
        inside: false,
        world_border_hit: false,
        sequence: 310,
    })
    .await
    .expect("peer completes ABA stone restore");
    wait_for_block_state_and_ack(&mut peer, 310, (0, target_y, 0), stone_state.0 as i32).await;
    wait_for_block_state(&mut miner, (0, target_y, 0), stone_state.0 as i32).await;

    wait_for_world_ticks(&mut miner, 32).await;
    miner
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: 311,
        })
        .await
        .expect("stop ABA survival break");
    wait_for_stale_break_resync(
        &mut miner,
        311,
        (0, target_y, 0),
        stone_state.0 as i32,
    )
    .await;
    assert_eq!(
        world
            .lock()
            .await
            .get_block(target)
            .expect("read ABA final target"),
        Some(stone_state),
        "ABA mutation version must preserve the peer-restored stone"
    );
}

async fn wait_for_block_state_and_ack(
    client: &mut Client,
    sequence: i32,
    target: (i32, i32, i32),
    expected_state: i32,
) {
    let mut saw_state = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_state && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("block state and ack");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode block state");
            if unpack_block_pos(packet.position) == target && packet.state_id == expected_state {
                saw_state = true;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet =
                SectionBlocksUpdate::decode(&mut body).expect("decode section block state");
            saw_state |= section_update_contains_state(&packet, target, expected_state);
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode block ack");
            saw_ack |= packet.sequence == sequence;
        }
    }
}

async fn wait_for_block_state(
    client: &mut Client,
    target: (i32, i32, i32),
    expected_state: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("peer block state");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode peer block state");
            if unpack_block_pos(packet.position) == target && packet.state_id == expected_state {
                return;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let packet =
                SectionBlocksUpdate::decode(&mut body).expect("decode peer section block state");
            if section_update_contains_state(&packet, target, expected_state) {
                return;
            }
        }
    }
}

fn section_update_contains_state(
    packet: &SectionBlocksUpdate,
    target: (i32, i32, i32),
    expected_state: i32,
) -> bool {
    packet.section_pos
        == pack_section_pos(
            target.0.div_euclid(16),
            target.1.div_euclid(16),
            target.2.div_euclid(16),
        )
        && packet.changes.iter().any(|change| {
            change.relative_pos == pack_section_relative_pos(target.0, target.1, target.2)
                && change.state_id == expected_state
        })
}

async fn wait_for_stale_break_resync(
    client: &mut Client,
    sequence: i32,
    target: (i32, i32, i32),
    expected_state: i32,
) {
    let mut saw_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("stale break resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode stale break resync");
            if unpack_block_pos(packet.position) == target {
                assert_eq!(
                    packet.state_id, expected_state,
                    "stale break must resync the peer replacement instead of mutating it"
                );
                saw_resync = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode stale break ack");
            if packet.sequence == sequence {
                assert!(saw_resync, "stale break resync must precede its ack");
                return;
            }
        }
    }
}

#[tokio::test]
async fn out_of_reach_survival_and_creative_breaks_resync_target_before_ack() {
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
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let stone_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .expect("stone in registry");
    let stone_state_id = stone_state.0 as i32;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let seeded_y =
        top_non_air_y(&mut storage, 6, 0, air_state).expect("loaded column has terrain");
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: 6,
                y: seeded_y,
                z: 0,
            },
            stone_state,
        )
        .expect("seed far stone target")
        .expect("replace generated far top block");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 out-of-reach survival break resync".into(),
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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100FarBreak").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let dx = sync.x - 6.5;
    let dy = sync.y + 1.62 - (f64::from(seeded_y) + 0.5);
    let dz = sync.z - 0.5;
    assert!(
        dx * dx + dy * dy + dz * dz > 36.0,
        "seeded break target must be outside creative reach"
    );

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(6, seeded_y, 0),
            direction: Direction::Up,
            sequence: 26,
        })
        .await
        .expect("send out-of-reach survival start break");
    read_rejected_break_resync_before_ack(&mut client, 26, (6, seeded_y, 0), stone_state_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(6, seeded_y, 0),
            direction: Direction::Up,
            sequence: 27,
        })
        .await
        .expect("send out-of-reach creative start break");
    read_rejected_break_resync_before_ack(&mut client, 27, (6, seeded_y, 0), stone_state_id).await;
}

#[tokio::test]
async fn bedrock_break_resyncs_survival_and_succeeds_in_creative() {
    let data = embedded_play_data();
    let air = embedded_block_state(&data, "minecraft:air");
    let bedrock = embedded_block_state(&data, "minecraft:bedrock");
    let mut world = embedded_world(&data);
    let target_y = top_non_air_y(&mut world, 0, 0, air).expect("spawn column has terrain");
    world
        .set_block_at(
            mc_world::BlockPos {
                x: 0,
                y: target_y,
                z: 0,
            },
            bedrock,
        )
        .expect("seed creative bedrock target")
        .expect("replace generated spawn surface");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "creative bedrock break");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "CreativeBedrock").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let target = pack_block_pos(0, target_y, 0);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target,
            direction: Direction::Up,
            sequence: 28,
        })
        .await
        .expect("start survival bedrock break");
    wait_for_world_ticks(&mut client, 18).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: target,
            direction: Direction::Up,
            sequence: 29,
        })
        .await
        .expect("stop survival bedrock break");

    let mut saw_bedrock_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("survival bedrock break response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode bedrock resync");
            if unpack_block_pos(packet.position) == (0, target_y, 0) {
                assert_eq!(packet.state_id, bedrock.0 as i32);
                saw_bedrock_resync = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode survival bedrock ack");
            if packet.sequence == 29 {
                assert!(
                    saw_bedrock_resync,
                    "denied survival bedrock break must resync before ack"
                );
                break;
            }
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target,
            direction: Direction::Up,
            sequence: 30,
        })
        .await
        .expect("break creative bedrock target");

    let mut saw_air = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("creative bedrock break response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode bedrock BlockUpdate");
            if unpack_block_pos(packet.position) == (0, target_y, 0) {
                assert_eq!(packet.state_id, air.0 as i32);
                saw_air = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode bedrock ack");
            if packet.sequence == 30 {
                assert!(saw_air, "creative bedrock removal must precede its ack");
                break;
            }
        }
    }

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("creative bedrock server shutdown")
        .expect("creative bedrock server join")
        .expect("creative bedrock server serve");
}

#[tokio::test]
async fn far_out_of_reach_survival_break_does_not_load_target_before_ack() {
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

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 far survival break no load".into(),
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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "M100NoLoad").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let target = (96, sync.y.floor() as i32 - 2, 0);

    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: pack_block_pos(target.0, target.1, target.2),
            direction: Direction::Up,
            sequence: 27,
        })
        .await
        .expect("send far out-of-reach survival start break");
    read_ack_without_target_update(&mut client, 27, target).await;
}

async fn read_rejected_break_resync_before_ack(
    client: &mut Client,
    sequence: i32,
    target: (i32, i32, i32),
    expected_state_id: i32,
) {
    let mut saw_target_resync = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("rejected break resync");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode rejected break BlockUpdate");
            if unpack_block_pos(pkt.position) == target {
                assert_eq!(
                    pkt.state_id, expected_state_id,
                    "rejected break must resync the authoritative target state"
                );
                saw_target_resync = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode rejected break ack");
            if pkt.sequence == sequence {
                assert!(
                    saw_target_resync,
                    "rejected break ack arrived before target-cell BlockUpdate"
                );
                return;
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
    let seeded_y = top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column has terrain");
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
    assert_eq!(target_y, seeded_y, "client spawn should expose seeded target");
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
    assert!((1..=simulation.simulation_queue_capacity).contains(&simulation.simulation_queue_max_depth));
    assert!((1..=simulation.simulation_queue_max_depth).contains(&simulation.simulation_max_batch));
    assert_eq!(simulation.simulation_commands_rejected_full, 0);
    assert_eq!(simulation.simulation_commands_rejected_closed, 0);
    assert_eq!(simulation.simulation_commands_rejected_shutdown, 0);
    assert_eq!(simulation.simulation_commands_rejected_world_busy, 0);
    assert_eq!(
        simulation.simulation_commands_rejected_world_unavailable,
        0
    );
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
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
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
    wait_for_world_ticks(
        &mut client,
        vanilla_stop_destroy_ticks(0.5, 1.0, true),
    )
    .await;
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
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("dirt in registry");
    let stone_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("stone in registry");
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
    let stone_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .expect("stone item");
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

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

    let (mut client, sync) = connect_to_play(addr, "M100BadHotbar").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
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
    wait_for_world_ticks(
        &mut client,
        vanilla_stop_destroy_ticks(0.5, 1.0, true),
    )
    .await;
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
        entity_types: std::sync::Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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
