#[tokio::test]
async fn place_recipe_crafts_torch_from_authoritative_inventory() {
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
    let coal_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
        .expect("coal item");
    let stick_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stick").unwrap())
        .expect("stick item");
    let torch_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:torch").unwrap())
        .expect("torch item");
    let loaded_recipes = mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
        .expect("recipes load");
    let Some(torch_recipe) = (if loaded_recipes.is_empty() {
        Some(0)
    } else {
        loaded_recipes
            .iter()
            .position(|recipe| recipe.id.as_str() == "minecraft:torch")
            .and_then(|idx| i32::try_from(idx).ok())
    }) else {
        eprintln!("skipping: missing torch recipe");
        return;
    };

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 crafting".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(loaded_recipes),
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

    let (mut client, _) = connect_to_play(addr, "M23Crafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:coal 1 0".into(),
        })
        .await
        .expect("give coal");
    wait_for_slot_stack(&mut client, coal_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stick 1 1".into(),
        })
        .await
        .expect("give stick");
    wait_for_slot_stack(&mut client, stick_id, 1).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: torch_recipe,
            use_max_items: false,
        })
        .await
        .expect("place torch recipe");

    let mut saw_coal_consumed = false;
    let mut saw_stick_consumed = false;
    let mut saw_torches = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_coal_consumed && saw_stick_consumed && saw_torches) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("craft response");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode set slot");
            if pkt.slot == 36 && pkt.item_stack.is_empty() {
                saw_coal_consumed = true;
            } else if pkt.slot == 37 && pkt.item_stack.is_empty() {
                saw_stick_consumed = true;
            } else if pkt.item_stack.item_id == torch_id && pkt.item_stack.count == 4 {
                saw_torches = true;
            }
        }
    }
}

#[tokio::test]
async fn place_recipe_crafts_tag_based_planks_sticks_and_table() {
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

    let loaded_recipes = mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
        .expect("recipes load");
    let fallback_recipe_display_id = |id: &str| match id {
        "minecraft:oak_planks" => Some(1),
        "minecraft:stick" => Some(2),
        "minecraft:crafting_table" => Some(3),
        _ => None,
    };
    let recipe_display_id = |id: &str| {
        if loaded_recipes.is_empty() {
            fallback_recipe_display_id(id)
        } else {
            loaded_recipes
                .iter()
                .position(|recipe| recipe.id.as_str() == id)
                .and_then(|idx| i32::try_from(idx).ok())
        }
    };
    let Some(oak_planks_recipe) = recipe_display_id("minecraft:oak_planks") else {
        eprintln!("skipping: missing oak_planks recipe");
        return;
    };
    let Some(stick_recipe) = recipe_display_id("minecraft:stick") else {
        eprintln!("skipping: missing stick recipe");
        return;
    };
    let Some(crafting_table_recipe) = recipe_display_id("minecraft:crafting_table") else {
        eprintln!("skipping: missing crafting_table recipe");
        return;
    };

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
    let item_registry = mc_data::Identifier::parse("minecraft:item").unwrap();
    let oak_logs_tag = mc_data::Identifier::parse("minecraft:oak_logs").unwrap();
    let planks_tag = mc_data::Identifier::parse("minecraft:planks").unwrap();
    if !tags
        .registries
        .get(&item_registry)
        .is_some_and(|item_tags| {
            item_tags.contains_key(&oak_logs_tag) && item_tags.contains_key(&planks_tag)
        })
    {
        eprintln!("skipping: missing item oak_logs/planks tags");
        return;
    }
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let oak_log_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak_log item");
    let oak_planks_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak_planks item");
    let stick_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stick").unwrap())
        .expect("stick item");
    let crafting_table_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .expect("crafting_table item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 tag crafting".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(loaded_recipes.clone()),
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

    let (mut client, _) = connect_to_play(addr, "M23TagCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 0".into(),
        })
        .await
        .expect("give oak_log");
    wait_for_slot_stack(&mut client, oak_log_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("place oak_planks recipe");
    wait_for_slot_stack(&mut client, oak_planks_id, 8).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: crafting_table_recipe,
            use_max_items: false,
        })
        .await
        .expect("place crafting_table recipe");
    wait_for_slot_stack(&mut client, crafting_table_id, 1).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_planks 2 1".into(),
        })
        .await
        .expect("give oak_planks");
    wait_for_slot_stack(&mut client, oak_planks_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: stick_recipe,
            use_max_items: false,
        })
        .await
        .expect("place stick recipe");
    wait_for_slot_stack(&mut client, stick_id, 4).await;
}

async fn assert_no_slot_stack_for(client: &mut Client, item_id: u32) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "time set 1000".into(),
        })
        .await
        .expect("send absent-slot packet fence");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("absent-slot packet fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetTime::ID {
            let mut body = frame.body;
            let _time = ClientboundSetTime::decode(&mut body)
                .expect("decode absent-slot packet fence");
            return;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            assert!(
                pkt.item_stack.item_id != item_id || pkt.item_stack.count <= 0,
                "unexpected item {item_id} in slot {}: {:?}",
                pkt.slot,
                pkt.item_stack
            );
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundContainerSetContent::decode(&mut body).expect("decode SetContent");
            assert!(
                pkt.carried_item.item_id != item_id || pkt.carried_item.count <= 0,
                "unexpected item {item_id} carried: {:?}",
                pkt.carried_item
            );
            for (slot, stack) in pkt.items.iter().enumerate() {
                assert!(
                    stack.item_id != item_id || stack.count <= 0,
                    "unexpected item {item_id} in content slot {slot}: {stack:?}"
                );
            }
        }
    }
}

async fn wait_for_save_all_feedback(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let text = wait_for_system_chat_text(client, deadline).await;
        if text.starts_with("Saved ") {
            return;
        }
    }
}

async fn wait_for_system_chat_text(
    client: &mut Client,
    deadline: tokio::time::Instant,
) -> String {
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("system chat feedback");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let pkt = ClientboundSystemChat::decode(&mut body).expect("decode SystemChat");
            return system_chat_text(&pkt);
        }
    }
}

fn system_chat_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = bytes::Bytes::copy_from_slice(&packet.content_nbt);
    let tag = mc_nbt::read_network(&mut bytes).expect("read system chat nbt");
    let mc_nbt::Tag::Compound(fields) = tag else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("system chat component text")
}

async fn assert_no_position_correction(client: &mut Client, duration: Duration) {
    let deadline = tokio::time::Instant::now() + duration;
    let mut saw_liveness = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            if !saw_liveness {
                let id = prove_clientbound_liveness(client).await;
                assert_ne!(
                    id,
                    SynchronizePlayerPosition::ID,
                    "movement window should not require correction after liveness probe"
                );
            }
            return;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            if !saw_liveness {
                let id = prove_clientbound_liveness(client).await;
                assert_ne!(
                    id,
                    SynchronizePlayerPosition::ID,
                    "movement window should not require correction after liveness probe"
                );
            }
            return;
        };
        saw_liveness = true;
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt = SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
            panic!("movement window should not require correction: {pkt:?}");
        }
    }
}

async fn prove_clientbound_liveness(client: &mut Client) -> i32 {
    client
        .write_packet(&ServerboundChatCommand {
            command: "status".into(),
        })
        .await
        .expect("send liveness probe command");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out before liveness probe response"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for liveness probe response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        return frame.id;
    }
}

async fn wait_for_position_correction(
    client: &mut Client,
    duration: Duration,
) -> SynchronizePlayerPosition {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for position correction"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("wait for position correction");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            return SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
        }
    }
}

fn assert_position_near(
    correction: &SynchronizePlayerPosition,
    x: f64,
    y: f64,
    z: f64,
    tolerance: f64,
) {
    assert!(
        (correction.x - x).abs() <= tolerance,
        "correction x: expected {x}, got {}",
        correction.x
    );
    assert!(
        (correction.y - y).abs() <= tolerance,
        "correction y: expected {y}, got {}",
        correction.y
    );
    assert!(
        (correction.z - z).abs() <= tolerance,
        "correction z: expected {z}, got {}",
        correction.z
    );
}

async fn wait_for_chunk_pipeline_idle(
    metrics: &mc_net::ChunkPipelineResourceMetrics,
    duration: Duration,
) -> mc_net::ChunkPipelineResourceSnapshot {
    let deadline = tokio::time::Instant::now() + duration;
    loop {
        let snapshot = metrics.snapshot();
        if snapshot.active_cpu == 0 && snapshot.active_io == 0 {
            return snapshot;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "chunk pipeline did not go idle: {snapshot:?}"
        );
        tokio::task::yield_now().await;
    }
}

async fn mine_block_and_wait_for_stack(
    client: &mut Client,
    pos: (i32, i32, i32),
    start_sequence: i32,
    completion_ticks: i64,
    item_id: u32,
    count: i32,
) {
    let packed = pack_block_pos(pos.0, pos.1, pos.2);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: packed,
            direction: Direction::Up,
            sequence: start_sequence,
        })
        .await
        .expect("send survival start break");
    read_ack_without_target_update(client, start_sequence, pos).await;
    wait_for_world_ticks(client, completion_ticks).await;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StopDestroyBlock,
            position: packed,
            direction: Direction::Up,
            sequence: start_sequence + 1,
        })
        .await
        .expect("send survival stop break");

    let stop_sequence = start_sequence + 1;
    let mut saw_break_update = false;
    let mut saw_break_ack = false;
    let mut saw_drop_stack = false;
    let mut saw_matching_slot_count = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "mined block {pos:?} did not reach inventory count {count}: \
                     error={err}; break_update={saw_break_update}; \
                     break_ack={saw_break_ack}; drop_stack={saw_drop_stack}; \
                     matching_slot_count={saw_matching_slot_count:?}"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode break BlockUpdate");
            if unpack_block_pos(pkt.position) == pos {
                saw_break_update = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode break ack");
            if pkt.sequence == stop_sequence {
                saw_break_ack = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode entity data");
            saw_drop_stack |= pkt.values.iter().any(|value| {
                matches!(
                    value,
                    EntityDataValue::ItemStack { stack, .. }
                        if stack.item_id == item_id && stack.count > 0
                )
            });
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode SetSlot");
            if pkt.item_stack.item_id == item_id {
                saw_matching_slot_count = Some(pkt.item_stack.count);
                if pkt.item_stack.count == count {
                    return;
                }
            }
        }
    }
}

async fn move_without_position_correction(
    client: &mut Client,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x,
            y,
            z,
            yaw,
            pitch,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move generated-world client");

    client
        .write_packet(&ServerboundChatCommand {
            command: "time set 1000".into(),
        })
        .await
        .expect("send movement packet fence");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("movement packet fence");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let _response = ClientboundSystemChat::decode(&mut body)
                .expect("decode movement packet fence");
            return;
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let pkt =
                SynchronizePlayerPosition::decode(&mut body).expect("decode generated move sync");
            panic!("generated-world movement should not require correction: {pkt:?}");
        }
    }
}

struct EmbeddedPlayData {
    report: Vec<mc_data::blocks::BlockReport>,
    blocks: Arc<mc_world::BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    tags: Arc<mc_data::tags::TagsData>,
    recipes: Arc<Vec<mc_data::recipes::Recipe>>,
}

fn embedded_play_data() -> EmbeddedPlayData {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let tags = Arc::new(mc_data::tags::solaris_required_item_tags(&items));
    let recipes = Arc::new(mc_data::recipes::solaris_required_recipes());
    EmbeddedPlayData {
        report,
        blocks,
        items,
        tags,
        recipes,
    }
}

fn embedded_world(data: &EmbeddedPlayData) -> mc_world::WorldStorage {
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        0,
        Arc::clone(&data.blocks),
    ));
    mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&data.blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator)
}

fn embedded_disk_world(data: &EmbeddedPlayData, path: &std::path::Path) -> mc_world::WorldStorage {
    std::fs::create_dir_all(path.join("region")).expect("create region dir");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        0,
        Arc::clone(&data.blocks),
    ));
    mc_world::WorldStorage::open_with_capacity(
        path,
        Arc::clone(&data.blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("disk world opens")
    .with_item_registry(Arc::clone(&data.items))
    .with_generator(generator)
}

fn embedded_recipe_display_id(data: &EmbeddedPlayData, id: &str) -> i32 {
    data.recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == id)
        .and_then(|idx| i32::try_from(idx).ok())
        .unwrap_or_else(|| panic!("embedded recipe {id}"))
}

fn embedded_item_id(data: &EmbeddedPlayData, id: &str) -> u32 {
    data.items
        .id_of(&mc_data::Identifier::parse(id).unwrap())
        .unwrap_or_else(|| panic!("embedded item {id}"))
}

fn embedded_block_state(data: &EmbeddedPlayData, id: &str) -> mc_world::BlockStateId {
    data.blocks
        .block(&mc_data::Identifier::parse(id).unwrap())
        .map(|block| block.default)
        .unwrap_or_else(|| panic!("embedded block {id}"))
}

fn embedded_state_name(data: &EmbeddedPlayData, state: mc_world::BlockStateId) -> Option<&str> {
    data.blocks
        .by_id(state)
        .map(|block_state| block_state.block.id.as_str())
}

fn embedded_state_is_named(
    data: &EmbeddedPlayData,
    state: mc_world::BlockStateId,
    id: &str,
) -> bool {
    embedded_state_name(data, state) == Some(id)
}

fn generated_walkable_surface_y(
    world: &mut mc_world::WorldStorage,
    data: &EmbeddedPlayData,
    x: i32,
    z: i32,
    air_state: mc_world::BlockStateId,
) -> Option<i32> {
    for y in (mc_world::MIN_Y..mc_world::MAX_Y - 2).rev() {
        let support = world
            .get_block(mc_world::BlockPos { x, y, z })
            .expect("read generated support")?;
        if support == air_state {
            continue;
        }
        let support_name = embedded_state_name(data, support).unwrap_or_default();
        if support_name.ends_with("_log") || support_name.ends_with("_leaves") {
            continue;
        }
        let feet = world
            .get_block(mc_world::BlockPos { x, y: y + 1, z })
            .expect("read generated feet");
        let head = world
            .get_block(mc_world::BlockPos { x, y: y + 2, z })
            .expect("read generated head");
        if feet == Some(air_state) && head == Some(air_state) {
            return Some(y);
        }
    }
    None
}

#[derive(Debug, Clone)]
struct GeneratedTreeLoopTarget {
    logs: [(i32, i32, i32); 3],
    log_block_id: String,
    stand_x: i32,
    stand_surface_y: i32,
    stand_z: i32,
}

fn find_generated_tree_loop_target(
    world: &mut mc_world::WorldStorage,
    data: &EmbeddedPlayData,
    air_state: mc_world::BlockStateId,
) -> GeneratedTreeLoopTarget {
    const SEARCH_RADIUS: i32 = 64;
    let adjacent = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    for x in -SEARCH_RADIUS..=SEARCH_RADIUS {
        for z in -SEARCH_RADIUS..=SEARCH_RADIUS {
            if x * x + z * z > SEARCH_RADIUS * SEARCH_RADIUS {
                continue;
            }
            for y in mc_world::MIN_Y..mc_world::MAX_Y - 2 {
                let Some(base_state) = world
                    .get_block(mc_world::BlockPos { x, y, z })
                    .expect("read generated tree base")
                else {
                    continue;
                };
                let Some(log_block_id) = embedded_state_name(data, base_state) else {
                    continue;
                };
                if !log_block_id.ends_with("_log") {
                    continue;
                }
                let is_three_high_log = (1..3).all(|dy| {
                    world
                        .get_block(mc_world::BlockPos { x, y: y + dy, z })
                        .expect("read generated tree")
                        .is_some_and(|state| embedded_state_is_named(data, state, log_block_id))
                });
                if !is_three_high_log {
                    continue;
                }
                for (dx, dz) in adjacent {
                    let stand_x = x + dx;
                    let stand_z = z + dz;
                    if let Some(stand_surface_y) =
                        generated_walkable_surface_y(world, data, stand_x, stand_z, air_state)
                    {
                        return GeneratedTreeLoopTarget {
                            logs: [(x, y, z), (x, y + 1, z), (x, y + 2, z)],
                            log_block_id: log_block_id.to_string(),
                            stand_x,
                            stand_surface_y,
                            stand_z,
                        };
                    }
                }
            }
        }
    }
    panic!("generated seed-0 playable world should have a reachable oak trunk within 64 blocks");
}

fn embedded_playable_config(
    data: &EmbeddedPlayData,
    world: mc_world::WorldStorage,
    motd: &str,
) -> mc_net::ServerConfig {
    mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: motd.into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks: Arc::clone(&data.blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::clone(&data.tags),
        recipes: Arc::clone(&data.recipes),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items: Arc::clone(&data.items),
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            &data.report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    }
}

#[tokio::test]
async fn embedded_playable_flat_move_jump_input_and_wall_collision_behave() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let stone_state = embedded_block_state(&data, "minecraft:stone");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn column terrain");
    let player_y = surface_y + 2;
    for x in -1..=3 {
        for z in 8..=12 {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x,
                        y: player_y - 1,
                        z,
                    },
                    stone_state,
                )
                .expect("seed movement floor")
                .expect("replace movement floor");
            for y in player_y..=player_y + 2 {
                world
                    .set_block_at(mc_world::BlockPos { x, y, z }, air_state)
                    .expect("clear movement space")
                    .expect("replace movement space");
            }
        }
    }
    for z in 8..=12 {
        for y in player_y..=player_y + 1 {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x: 2,
                        y,
                        z,
                    },
                    stone_state,
                )
                .expect("seed collision wall")
                .expect("replace collision wall");
        }
    }

    let cfg = embedded_playable_config(&data, world, "P1 embedded movement");
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "P1MoveWall").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(sync.y.floor() as i32, player_y);

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 1.5,
            y: f64::from(player_y),
            z: 10.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move across embedded flat ground");
    assert_no_position_correction(&mut client, Duration::from_millis(300)).await;

    client
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput {
                jump: true,
                ..PlayerInput::default()
            },
        })
        .await
        .expect("send jump input");
    assert_no_position_correction(&mut client, Duration::from_millis(300)).await;

    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: 2.5,
            y: f64::from(player_y),
            z: 10.5,
            yaw: 90.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move into embedded wall");

    let correction = wait_for_position_correction(&mut client, Duration::from_secs(2)).await;
    assert_position_near(&correction, 1.5, f64::from(player_y), 10.5, 1.0e-6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embedded_playable_short_session_soak_keeps_clients_responsive() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let stone_state = embedded_block_state(&data, "minecraft:stone");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn column terrain");
    let player_y = surface_y + 2;
    for x in -2..=5 {
        for z in -2..=5 {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x,
                        y: player_y - 1,
                        z,
                    },
                    stone_state,
                )
                .expect("seed soak floor")
                .expect("replace soak floor");
            for y in player_y..=player_y + 2 {
                world
                    .set_block_at(mc_world::BlockPos { x, y, z }, air_state)
                    .expect("clear soak movement space")
                    .expect("replace soak movement space");
            }
        }
    }

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P2 embedded short soak");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let chunk_metrics = bound.chunk_pipeline_metrics();
    let outbound_pressure = bound.outbound_pressure_handle();
    let serve = tokio::spawn(async move { bound.serve().await });

    let mut tasks = Vec::new();
    for idx in 0..4 {
        tasks.push(tokio::spawn(async move {
            let (mut client, sync) = connect_to_play(addr, &format!("P2Soak{idx}")).await;
            drain_until_chunk(&mut client, (0, 0)).await;
            assert_eq!(sync.y.floor() as i32, player_y);

            for step in 0..8 {
                client
                    .write_packet(&ServerboundMovePlayerPosRot {
                        x: sync.x + (step as f64 * 0.08),
                        y: sync.y,
                        z: sync.z + (idx as f64 * 0.25),
                        yaw: 0.0,
                        pitch: 0.0,
                        flags: MovePlayerFlags::new(true, false),
                    })
                    .await
                    .expect("send soak movement");
                if step % 2 == 0 {
                    client
                        .write_packet(&ServerboundPlayerInput {
                            input: PlayerInput {
                                forward: true,
                                sprint: true,
                                jump: step == 2,
                                ..PlayerInput::default()
                            },
                        })
                        .await
                        .expect("send soak input");
                }
                assert_no_position_correction(&mut client, Duration::from_millis(200)).await;
            }

            let liveness = prove_clientbound_liveness(&mut client).await;
            assert_ne!(
                liveness,
                SynchronizePlayerPosition::ID,
                "soak client should still be responsive without a position correction"
            );
            (idx, client)
        }));
    }

    let mut completed = HashSet::new();
    let mut clients = Vec::new();
    for task in tasks {
        let (idx, client) = task.await.expect("soak client task joins");
        completed.insert(idx);
        clients.push(client);
    }
    assert_eq!(completed.len(), 4, "all soak clients should finish");

    let pressure = outbound_pressure.snapshot();
    assert_eq!(
        pressure.slow_client_write_timeouts, 0,
        "responsive playable soak clients should not hit slow-write timeouts: {pressure:?}"
    );
    assert_eq!(
        pressure.slow_client_pressure_sheds, 0,
        "responsive playable soak clients should not shed outbound pressure: {pressure:?}"
    );
    assert_eq!(
        pressure.best_effort_animation_drops, 0,
        "responsive playable soak clients should not drop cosmetic animations before disconnect: {pressure:?}"
    );
    assert_eq!(
        pressure.reliable_command_drops, 0,
        "responsive playable soak clients must not lose reliable commands before disconnect: {pressure:?}"
    );
    drop(clients);

    let (mut probe, _) = connect_to_play(addr, "P2SoakProbe").await;
    drain_until_chunk(&mut probe, (0, 0)).await;
    let liveness = prove_clientbound_liveness(&mut probe).await;
    assert_ne!(
        liveness,
        SynchronizePlayerPosition::ID,
        "server should accept a fresh client after the soak window"
    );
    drop(probe);

    let chunk_snapshot = wait_for_chunk_pipeline_idle(&chunk_metrics, Duration::from_secs(5)).await;
    assert_eq!(
        chunk_snapshot.active_cpu, 0,
        "chunk CPU work should drain after playable soak: {chunk_snapshot:?}"
    );
    assert_eq!(
        chunk_snapshot.active_io, 0,
        "chunk IO work should drain after playable soak: {chunk_snapshot:?}"
    );

    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("soak server shutdown")
        .expect("soak server join")
        .expect("soak server serve");
}

#[tokio::test]
async fn inventory_recipe_rejects_three_by_three_tool_without_crafting_table() {
    let data = embedded_play_data();
    let wooden_pickaxe_recipe = embedded_recipe_display_id(&data, "minecraft:wooden_pickaxe");
    let oak_planks_id = embedded_item_id(&data, "minecraft:oak_planks");
    let stick_id = embedded_item_id(&data, "minecraft:stick");
    let wooden_pickaxe_id = embedded_item_id(&data, "minecraft:wooden_pickaxe");

    let cfg = embedded_playable_config(
        &data,
        embedded_world(&data),
        "P2 embedded inventory recipe guard",
    );
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "P2RecipeGuard").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_planks 3 0".into(),
        })
        .await
        .expect("give planks");
    wait_for_slot_stack(&mut client, oak_planks_id, 3).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:stick 2 1".into(),
        })
        .await
        .expect("give sticks");
    wait_for_slot_stack(&mut client, stick_id, 2).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: wooden_pickaxe_recipe,
            use_max_items: false,
        })
        .await
        .expect("place wooden_pickaxe inventory recipe");
    assert_no_slot_stack_for(&mut client, wooden_pickaxe_id).await;
}

#[tokio::test]
async fn embedded_save_restart_rejoin_preserves_inventory_and_edited_block() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let dirt_state = embedded_block_state(&data, "minecraft:dirt");
    let crafting_table_state = embedded_block_state(&data, "minecraft:crafting_table");
    let dirt_id = embedded_item_id(&data, "minecraft:dirt");
    let crafting_table_id = embedded_item_id(&data, "minecraft:crafting_table");
    let oak_log_id = embedded_item_id(&data, "minecraft:oak_log");
    let world_dir = tempfile::tempdir().expect("temp world");

    let mut world = embedded_disk_world(&data, world_dir.path());
    let support_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn column terrain");
    let placed_y = support_y + 1;
    let table_support_y =
        top_non_air_y(&mut world, 1, 0, air_state).expect("crafting table support terrain");
    let table_y = table_support_y + 1;
    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P2 embedded persistence");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P2Persist").await;
    wait_for_inventory_content(&mut client, |pkt| pkt.container_id == 0).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 4 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut client, dirt_id, 4).await;
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
            sequence: 301,
        })
        .await
        .expect("place dirt");
    wait_for_block_update(&mut client, (0, placed_y, 0), dirt_state.0 as i32).await;
    wait_for_slot_stack(&mut client, dirt_id, 3).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:crafting_table 1 1".into(),
        })
        .await
        .expect("give crafting table for close settlement");
    wait_for_slot_stack(&mut client, crafting_table_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 1 2".into(),
        })
        .await
        .expect("give crafting input for close settlement");
    wait_for_slot_stack(&mut client, oak_log_id, 1).await;
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await
        .expect("select crafting table");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(1, table_support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 302,
        })
        .await
        .expect("place crafting table for close settlement");
    wait_for_block_update(
        &mut client,
        (1, table_y, 0),
        crafting_table_state.0 as i32,
    )
    .await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(1, table_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 303,
        })
        .await
        .expect("open crafting table for close settlement");
    let opened = wait_for_open_screen(&mut client, 12).await;
    let mut content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items.len() == 46 && pkt.items[39].item_id == oak_log_id
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 2,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move oak log into crafting grid");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[1].item_id == oak_log_id && pkt.items[39].is_empty()
    })
    .await;
    assert!(content.carried_item.is_empty());
    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close crafting table with unconsumed input");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: -1,
            slot_num: -999,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("request inventory after crafting close");
    let returned = wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt
                .items
                .iter()
                .any(|stack| stack.item_id == oak_log_id && stack.count == 1)
    })
    .await;
    assert_eq!(
        returned
            .items
            .iter()
            .filter(|stack| stack.item_id == oak_log_id)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1,
        "crafting close must return exactly one input"
    );
    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("save all");
    wait_for_save_all_feedback(&mut client).await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("first server shutdown")
        .expect("first server join")
        .expect("first server serve");

    let mut reopened = mc_world::WorldStorage::open(world_dir.path(), Arc::clone(&data.blocks))
        .expect("reopen saved world")
        .with_item_registry(Arc::clone(&data.items));
    let landed = reopened
        .get_block(mc_world::BlockPos {
            x: 0,
            y: placed_y,
            z: 0,
        })
        .expect("read placed block")
        .expect("placed block present");
    assert_eq!(landed, dirt_state, "edited block should survive restart");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(
        &data,
        embedded_disk_world(&data, world_dir.path()),
        "P2 embedded persistence rejoin",
    );
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("rebind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P2Persist").await;
    let restored = wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt.items.get(36).is_some_and(|stack| {
                stack.item_id == dirt_id && stack.count == 3 && stack.damage.is_none()
            })
            && pkt
                .items
                .iter()
                .any(|stack| stack.item_id == oak_log_id && stack.count == 1)
    })
    .await;
    assert_eq!(restored.items[36].item_id, dirt_id);
    assert_eq!(restored.items[36].count, 3);
    assert_eq!(
        restored
            .items
            .iter()
            .filter(|stack| stack.item_id == oak_log_id)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1,
        "returned crafting input should survive restart"
    );

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("second server shutdown")
        .expect("second server join")
        .expect("second server serve");
}

#[tokio::test]
async fn embedded_non_op_shutdown_restart_preserves_survival_edit_and_inventory() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let dirt_state = embedded_block_state(&data, "minecraft:dirt");
    let stone_state = embedded_block_state(&data, "minecraft:stone");
    let dirt_id = embedded_item_id(&data, "minecraft:dirt");
    let world_dir = tempfile::tempdir().expect("temp world");

    let mut world = embedded_disk_world(&data, world_dir.path());
    let surface_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn column terrain");
    for x in -1..=1 {
        world
            .set_block_at(mc_world::BlockPos { x, y: surface_y, z: 0 }, dirt_state)
            .expect("seed dirt resource")
            .expect("replace generated surface");
        for y in surface_y + 1..=surface_y + 3 {
            world
                .set_block_at(mc_world::BlockPos { x, y, z: 0 }, air_state)
                .expect("clear resource headroom")
                .expect("replace resource headroom");
        }
    }
    world
        .set_block_at(
            mc_world::BlockPos {
                x: 2,
                y: surface_y,
                z: 0,
            },
            stone_state,
        )
        .expect("seed placement support")
        .expect("replace generated support");
    for y in surface_y + 1..=surface_y + 3 {
        world
            .set_block_at(mc_world::BlockPos { x: 2, y, z: 0 }, air_state)
            .expect("clear placement headroom")
            .expect("replace placement headroom");
    }

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P2 non-op shutdown persistence");
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false);
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, sync) = connect_to_play(addr, "P2NoOpPersist").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    assert_eq!(sync.y.floor() as i32 - 2, surface_y);
    let hand_dirt_ticks = vanilla_stop_destroy_ticks(0.5, 1.0, true);
    mine_block_and_wait_for_stack(
        &mut client,
        (-1, surface_y, 0),
        401,
        hand_dirt_ticks,
        dirt_id,
        1,
    )
    .await;
    mine_block_and_wait_for_stack(
        &mut client,
        (1, surface_y, 0),
        403,
        hand_dirt_ticks,
        dirt_id,
        2,
    )
    .await;
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 0 })
        .await
        .expect("select mined dirt");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(2, surface_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 405,
        })
        .await
        .expect("place survival-mined dirt");
    wait_for_block_update(&mut client, (2, surface_y + 1, 0), dirt_state.0 as i32).await;
    wait_for_slot_stack(&mut client, dirt_id, 1).await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("first non-op server shutdown")
        .expect("first non-op server join")
        .expect("first non-op server serve");

    let mut reopened = mc_world::WorldStorage::open(world_dir.path(), Arc::clone(&data.blocks))
        .expect("reopen saved world")
        .with_item_registry(Arc::clone(&data.items));
    let persisted_block = reopened
        .get_block(mc_world::BlockPos {
            x: 2,
            y: surface_y + 1,
            z: 0,
        })
        .expect("read non-op placed block")
        .expect("non-op placed block present");
    assert_eq!(
        persisted_block, dirt_state,
        "survival-placed block should survive shutdown save"
    );

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(
        &data,
        embedded_disk_world(&data, world_dir.path()),
        "P2 non-op shutdown persistence rejoin",
    );
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false);
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("rebind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P2NoOpPersist").await;
    let restored = wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt.items.get(36).is_some_and(|stack| {
                stack.item_id == dirt_id && stack.count == 1 && stack.damage.is_none()
            })
    })
    .await;
    assert_eq!(restored.items[36].item_id, dirt_id);
    assert_eq!(restored.items[36].count, 1);

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("second non-op server shutdown")
        .expect("second non-op server join")
        .expect("second non-op server serve");
}

#[tokio::test]
async fn embedded_generated_seed_survival_crafts_tool_and_persists_without_debug() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let crafting_table_state = embedded_block_state(&data, "minecraft:crafting_table");
    let stick_id = embedded_item_id(&data, "minecraft:stick");
    let crafting_table_id = embedded_item_id(&data, "minecraft:crafting_table");
    let wooden_pickaxe_id = embedded_item_id(&data, "minecraft:wooden_pickaxe");
    let crafting_table_recipe = embedded_recipe_display_id(&data, "minecraft:crafting_table");
    let stick_recipe = embedded_recipe_display_id(&data, "minecraft:stick");
    let wooden_pickaxe_recipe = embedded_recipe_display_id(&data, "minecraft:wooden_pickaxe");
    let world_dir = tempfile::tempdir().expect("temp generated world");

    let mut world = embedded_disk_world(&data, world_dir.path());
    let target = find_generated_tree_loop_target(&mut world, &data, air_state);
    let log_item_id = embedded_item_id(&data, &target.log_block_id);
    let wood_family = target
        .log_block_id
        .strip_prefix("minecraft:")
        .and_then(|id| id.strip_suffix("_log"))
        .expect("generated tree target is a minecraft log");
    let planks_id = format!("minecraft:{wood_family}_planks");
    let planks_item_id = embedded_item_id(&data, &planks_id);
    let planks_recipe = embedded_recipe_display_id(&data, &planks_id);
    let table_support = mc_world::BlockPos {
        x: target.stand_x,
        y: target.stand_surface_y,
        z: target.stand_z,
    };
    let table_pos = mc_world::BlockPos {
        x: target.stand_x,
        y: target.stand_surface_y + 1,
        z: target.stand_z,
    };

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P2 generated seed wood tool");
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false);
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P2GeneratedWood").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    move_without_position_correction(
        &mut client,
        f64::from(target.stand_x) + 0.5,
        f64::from(target.stand_surface_y + 2),
        f64::from(target.stand_z) + 0.5,
        0.0,
        0.0,
    )
    .await;

    for (idx, pos) in target.logs.into_iter().enumerate() {
        mine_block_and_wait_for_stack(
            &mut client,
            pos,
            501 + (idx as i32 * 2),
            vanilla_stop_destroy_ticks(2.0, 1.0, true),
            log_item_id,
            idx as i32 + 1,
        )
        .await;
    }

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("craft generated planks");
    wait_for_slot_stack(&mut client, planks_item_id, 12).await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: crafting_table_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft generated table");
    let table_update = wait_for_slot_stack_update(&mut client, crafting_table_id, 1).await;
    let table_slot = table_update.slot;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: stick_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft generated sticks");
    let stick_update = wait_for_slot_stack_update(&mut client, stick_id, 4).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: stick_update.state_id,
            slot_num: table_slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: crafting_table_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up generated-world table");
    let content = wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.item_id == crafting_table_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: content.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move generated-world table to hotbar");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty()
            && pkt.items[36].item_id == crafting_table_id
            && pkt.items[36].count == 1
    })
    .await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(table_support.x, table_support.y, table_support.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 530,
        })
        .await
        .expect("place generated-world table");
    wait_for_block_update(
        &mut client,
        (table_pos.x, table_pos.y, table_pos.z),
        crafting_table_state.0 as i32,
    )
    .await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(table_pos.x, table_pos.y, table_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 531,
        })
        .await
        .expect("open generated-world table");
    let opened = wait_for_open_screen(&mut client, 12).await;
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| pkt.items.len() == 46).await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: opened.container_id,
            recipe_display_id: wooden_pickaxe_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft generated-world wooden pickaxe");
    wait_for_slot_stack(&mut client, wooden_pickaxe_id, 1).await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("generated-world server shutdown")
        .expect("generated-world server join")
        .expect("generated-world server serve");

    let mut reopened = mc_world::WorldStorage::open(world_dir.path(), Arc::clone(&data.blocks))
        .expect("reopen generated world")
        .with_item_registry(Arc::clone(&data.items));
    let persisted_table = reopened
        .get_block(table_pos)
        .expect("read generated-world table")
        .expect("generated-world table present");
    assert_eq!(
        persisted_table, crafting_table_state,
        "generated-world crafted table should survive shutdown"
    );

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(
        &data,
        embedded_disk_world(&data, world_dir.path()),
        "P2 generated seed wood tool rejoin",
    );
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false);
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("rebind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P2GeneratedWood").await;
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt.items.iter().any(|stack| {
                stack.item_id == wooden_pickaxe_id && stack.count == 1 && stack.damage.is_none()
            })
    })
    .await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("generated-world rejoin shutdown")
        .expect("generated-world rejoin join")
        .expect("generated-world rejoin serve");
}

#[tokio::test]
async fn embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let oak_log_state = embedded_block_state(&data, "minecraft:oak_log");
    let stone_state = embedded_block_state(&data, "minecraft:stone");
    let crafting_table_state = embedded_block_state(&data, "minecraft:crafting_table");
    let oak_log_id = embedded_item_id(&data, "minecraft:oak_log");
    let oak_planks_id = embedded_item_id(&data, "minecraft:oak_planks");
    let cobblestone_id = embedded_item_id(&data, "minecraft:cobblestone");
    let stick_id = embedded_item_id(&data, "minecraft:stick");
    let crafting_table_id = embedded_item_id(&data, "minecraft:crafting_table");
    let wooden_pickaxe_id = embedded_item_id(&data, "minecraft:wooden_pickaxe");
    let stone_pickaxe_id = embedded_item_id(&data, "minecraft:stone_pickaxe");
    let oak_planks_recipe = embedded_recipe_display_id(&data, "minecraft:oak_planks");
    let crafting_table_recipe = embedded_recipe_display_id(&data, "minecraft:crafting_table");
    let stick_recipe = embedded_recipe_display_id(&data, "minecraft:stick");
    let wooden_pickaxe_recipe = embedded_recipe_display_id(&data, "minecraft:wooden_pickaxe");
    let stone_pickaxe_recipe = embedded_recipe_display_id(&data, "minecraft:stone_pickaxe");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn column terrain");
    for x in [-1, 0, 1] {
        world
            .set_block_at(
                mc_world::BlockPos { x, y: surface_y, z: 0 },
                oak_log_state,
            )
            .expect("seed oak log")
            .expect("replace generated surface");
    }
    for y in [surface_y, surface_y + 1, surface_y + 2] {
        world
            .set_block_at(
                mc_world::BlockPos {
                    x: 0,
                    y,
                    z: 1,
                },
                stone_state,
            )
            .expect("seed stone")
            .expect("replace adjacent column");
    }
    let cfg = embedded_playable_config(&data, world, "P2 embedded wood to pickaxe");
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, sync) = connect_to_play(addr, "P2WoodPickaxe").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let target_y = sync.y.floor() as i32 - 2;
    assert_eq!(target_y, surface_y, "spawn target should be seeded logs");
    for (idx, x) in [-1, 0, 1].into_iter().enumerate() {
        mine_block_and_wait_for_stack(
            &mut client,
            (x, target_y, 0),
            201 + (idx as i32 * 2),
            vanilla_stop_destroy_ticks(2.0, 1.0, true),
            oak_log_id,
            idx as i32 + 1,
        )
        .await;
    }

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: oak_planks_recipe,
            use_max_items: true,
        })
        .await
        .expect("craft oak planks");
    wait_for_slot_stack(&mut client, oak_planks_id, 12).await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: crafting_table_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft table");
    let table_update = wait_for_slot_stack_update(&mut client, crafting_table_id, 1).await;
    let table_slot = table_update.slot;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: 0,
            recipe_display_id: stick_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft sticks");
    let stick_update = wait_for_slot_stack_update(&mut client, stick_id, 4).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: stick_update.state_id,
            slot_num: table_slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: crafting_table_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up crafted table");
    let content = wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.item_id == crafting_table_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: content.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move crafted table to hotbar");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty()
            && pkt.items[36].item_id == crafting_table_id
            && pkt.items[36].count == 1
    })
    .await;

    let support_y = target_y - 1;
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
            sequence: 230,
        })
        .await
        .expect("place crafted table");
    wait_for_block_update(
        &mut client,
        (0, target_y, 0),
        crafting_table_state.0 as i32,
    )
    .await;

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
            sequence: 231,
        })
        .await
        .expect("open crafted table");
    let opened = wait_for_open_screen(&mut client, 12).await;
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| pkt.items.len() == 46).await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: opened.container_id,
            recipe_display_id: wooden_pickaxe_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft wooden pickaxe at table");
    wait_for_slot_stack(&mut client, wooden_pickaxe_id, 1).await;
    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close crafting table before mining stone");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: -1,
            slot_num: -999,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("request inventory resync before moving pickaxe");
    let inventory = wait_for_inventory_content(&mut client, |pkt| {
        pkt.container_id == 0
            && pkt.items.iter().any(|stack| {
                stack.item_id == wooden_pickaxe_id && stack.count == 1 && stack.damage.is_none()
            })
    })
    .await;
    let wooden_pickaxe_slot = inventory
        .items
        .iter()
        .enumerate()
        .find_map(|(slot, stack)| {
            (stack.item_id == wooden_pickaxe_id && stack.count == 1 && stack.damage.is_none())
                .then_some(slot as i16)
        })
        .expect("crafted wooden pickaxe in inventory");
    let wooden_pickaxe_hotbar_slot = if (36..=44).contains(&wooden_pickaxe_slot) {
        wooden_pickaxe_slot - 36
    } else {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: inventory.state_id,
                slot_num: wooden_pickaxe_slot,
                button_num: 0,
                container_input: ContainerInput::Pickup,
                changed_slots: Vec::new(),
                carried_item: HashedStack::Actual {
                    item_id: wooden_pickaxe_id,
                    count: 1,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("pick up crafted wooden pickaxe");
        let content = wait_for_inventory_content(&mut client, |pkt| {
            pkt.carried_item.item_id == wooden_pickaxe_id && pkt.carried_item.count == 1
        })
        .await;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: 0,
                state_id: content.state_id,
                slot_num: 36,
                button_num: 0,
                container_input: ContainerInput::Pickup,
                changed_slots: Vec::new(),
                carried_item: HashedStack::empty(),
            })
            .await
            .expect("move crafted wooden pickaxe to hotbar");
        wait_for_inventory_content(&mut client, |pkt| {
            pkt.carried_item.is_empty()
                && pkt.items[36].item_id == wooden_pickaxe_id
                && pkt.items[36].count == 1
        })
        .await;
        0
    };
    client
        .write_packet(&ServerboundSetCarriedItem {
            slot: wooden_pickaxe_hotbar_slot,
        })
        .await
        .expect("select crafted wooden pickaxe");

    for (idx, y) in [target_y, target_y + 1, target_y + 2]
        .into_iter()
        .enumerate()
    {
        mine_block_and_wait_for_stack(
            &mut client,
            (0, y, 1),
            240 + (idx as i32 * 2),
            vanilla_stop_destroy_ticks(1.5, 2.0, true),
            cobblestone_id,
            idx as i32 + 1,
        )
        .await;
    }

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
            sequence: 250,
        })
        .await
        .expect("reopen crafted table");
    let reopened = wait_for_open_screen(&mut client, 12).await;
    wait_for_furnace_content(&mut client, reopened.container_id, |pkt| pkt.items.len() == 46).await;
    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: reopened.container_id,
            recipe_display_id: stone_pickaxe_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft stone pickaxe at table");
    wait_for_slot_stack(&mut client, stone_pickaxe_id, 1).await;
}

#[tokio::test]
async fn crafting_table_container_crafts_shapeless_and_shaped_results() {
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
    let crafting_table_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("crafting_table in registry");
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
    let crafting_table_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .expect("crafting_table item");
    let oak_log_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .expect("oak_log item");
    let oak_planks_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_planks").unwrap())
        .expect("oak_planks item");
    let recipes = Arc::new(
        mc_data::recipes::load_recipes(vanilla_dir.join("data/minecraft/recipe"))
            .expect("recipes load"),
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:oak_planks"),
        "oak planks recipe must come from sidecar"
    );
    assert!(
        recipes
            .iter()
            .any(|recipe| recipe.id.as_str() == "minecraft:crafting_table"),
        "crafting table recipe must come from sidecar"
    );
    let crafting_menu_id = 12;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M24 crafting table".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes,
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

    let (mut client, sync) = connect_to_play(addr, "M24TableCrafter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:crafting_table 1 0".into(),
        })
        .await
        .expect("give crafting table");
    wait_for_slot_stack(&mut client, crafting_table_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:oak_log 2 1".into(),
        })
        .await
        .expect("give oak log");
    wait_for_slot_stack(&mut client, oak_log_id, 2).await;

    let support_y = sync.y.floor() as i32 - 2;
    let table_y = support_y + 1;
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
            sequence: 101,
        })
        .await
        .expect("place crafting table");
    wait_for_block_update(&mut client, (0, table_y, 0), crafting_table_state_id).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, table_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 102,
        })
        .await
        .expect("open crafting table");
    let opened = wait_for_open_screen(&mut client, crafting_menu_id).await;
    let mut content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items.len() == 46 && pkt.items[0].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 38,
            button_num: 0,
            container_input: ContainerInput::Throw,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("throw one oak log from crafting window");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.is_empty() && pkt.items[38].item_id == oak_log_id && pkt.items[38].count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 1,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("swap oak log into crafting grid");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == oak_planks_id
            && pkt.items[0].count == 4
            && pkt.items[1].item_id == oak_log_id
            && pkt.items[38].is_empty()
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("shift-click planks result");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].is_empty()
            && pkt.items[1].is_empty()
            && pkt.items.iter().enumerate().any(|(slot, stack)| {
                slot >= 10 && stack.item_id == oak_planks_id && stack.count == 4
            })
    })
    .await;

    let planks_menu_slot = content
        .items
        .iter()
        .enumerate()
        .find_map(|(slot, stack)| {
            (slot >= 10 && stack.item_id == oak_planks_id && stack.count == 4)
                .then_some(slot as i16)
        })
        .expect("planks in table inventory");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: planks_menu_slot,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: oak_planks_id,
                count: 4,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up planks");
    content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == oak_planks_id && pkt.carried_item.count == 4
    })
    .await;
    for grid_slot in [1, 2, 4, 5] {
        let carried_after_click = content.carried_item.count - 1;
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: grid_slot,
                button_num: 1,
                container_input: ContainerInput::Pickup,
                changed_slots: Vec::new(),
                carried_item: if carried_after_click > 0 {
                    HashedStack::Actual {
                        item_id: oak_planks_id,
                        count: carried_after_click,
                        components: HashedStackComponentHashes::empty(),
                    }
                } else {
                    HashedStack::empty()
                },
            })
            .await
            .expect("place plank in shaped grid");
        content = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    }
    assert_eq!(content.items[0].item_id, crafting_table_id);
    assert_eq!(content.items[0].count, 1);
    assert!(content.carried_item.is_empty());

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: crafting_table_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("take shaped crafting result");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == crafting_table_id
            && pkt.carried_item.count == 1
            && pkt.items[0].is_empty()
            && (1..=9).all(|slot| pkt.items[slot].is_empty())
    })
    .await;
}

#[tokio::test]
async fn two_clients_stale_furnace_click_after_peer_update_resyncs() {
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("furnace in registry");
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
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 stale furnace resync".into(),
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

    let (mut actor, sync) = connect_to_play(addr, "M100FurnActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut actor, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 191,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut actor, (0, furnace_y, 0), furnace_state_id).await;

    let (mut observer, _) = connect_to_play(addr, "M100FurnObserve").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 192,
        })
        .await
        .expect("observer opens furnace");
    let observer_opened = wait_for_open_screen(&mut observer, furnace_menu_id).await;
    let observer_initial = wait_for_furnace_content(
        &mut observer,
        observer_opened.container_id,
        |pkt| pkt.items[0].is_empty() && pkt.carried_item.is_empty(),
    )
    .await;

    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:raw_iron 1 0".into(),
        })
        .await
        .expect("give raw iron");
    wait_for_slot_stack(&mut actor, raw_iron_id, 1).await;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 193,
        })
        .await
        .expect("actor opens furnace");
    let actor_opened = wait_for_open_screen(&mut actor, furnace_menu_id).await;
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |_| true)
        .await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 30,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: raw_iron_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up raw iron");
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.carried_item.item_id == raw_iron_id && pkt.carried_item.count == 1
    })
    .await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place raw iron input");
    wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    let observer_slot = wait_for_container_slot(
        &mut observer,
        observer_opened.container_id,
        0,
        |stack| stack.item_id == raw_iron_id && stack.count == 1,
    )
    .await;
    assert!(
        observer_slot.state_id > observer_initial.state_id,
        "peer furnace update should advance the shared container state"
    );

    observer
        .write_packet(&ServerboundContainerClick {
            container_id: observer_opened.container_id,
            state_id: observer_initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("send stale observer furnace click");
    let resync = wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.state_id == observer_slot.state_id
            && pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(resync.items[0].item_id, raw_iron_id);
    assert!(resync.carried_item.is_empty());
}

#[tokio::test]
async fn two_clients_stale_chest_click_after_peer_update_resyncs() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
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
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 stale chest resync".into(),
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

    let (mut actor, sync) = connect_to_play(addr, "M100ChestActor").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut actor, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 194,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut actor, (0, chest_y, 0), chest_state_id).await;

    let (mut observer, _) = connect_to_play(addr, "M100ChestObs").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, chest_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 195,
        })
        .await
        .expect("observer opens chest");
    let observer_opened = wait_for_open_screen(&mut observer, chest_menu_id).await;
    let observer_initial = wait_for_furnace_content(
        &mut observer,
        observer_opened.container_id,
        |pkt| pkt.items[0].is_empty() && pkt.carried_item.is_empty(),
    )
    .await;

    actor
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 1 0".into(),
        })
        .await
        .expect("give dirt");
    wait_for_slot_stack(&mut actor, dirt_id, 1).await;
    actor
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, chest_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 196,
        })
        .await
        .expect("actor opens chest");
    let actor_opened = wait_for_open_screen(&mut actor, chest_menu_id).await;
    let actor_content =
        wait_for_furnace_content(&mut actor, actor_opened.container_id, |_| true).await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 54,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt");
    let actor_content = wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.carried_item.item_id == dirt_id && pkt.carried_item.count == 1
    })
    .await;
    actor
        .write_packet(&ServerboundContainerClick {
            container_id: actor_opened.container_id,
            state_id: actor_content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place dirt in chest");
    wait_for_furnace_content(&mut actor, actor_opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id && pkt.items[0].count == 1 && pkt.carried_item.is_empty()
    })
    .await;
    let observer_slot = wait_for_container_slot(
        &mut observer,
        observer_opened.container_id,
        0,
        |stack| stack.item_id == dirt_id && stack.count == 1,
    )
    .await;
    assert!(
        observer_slot.state_id > observer_initial.state_id,
        "peer chest update should advance the shared container state"
    );

    observer
        .write_packet(&ServerboundContainerClick {
            container_id: observer_opened.container_id,
            state_id: observer_initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("send stale observer chest click");
    let resync = wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.state_id == observer_slot.state_id
            && pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(resync.items[0].item_id, dirt_id);
    assert!(resync.carried_item.is_empty());
}

#[tokio::test]
async fn server_origin_hopper_tick_updates_open_chests_and_comparator_over_tcp() {
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
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let hopper_id = mc_data::Identifier::parse("minecraft:hopper").unwrap();
    let comparator_id = mc_data::Identifier::parse("minecraft:comparator").unwrap();
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_state_id = blocks.block(&chest_id).expect("chest block").default;
    let hopper_state_id = blocks
        .by_name_and_props(
            &hopper_id,
            &[
                ("enabled".to_string(), "true".to_string()),
                ("facing".to_string(), "down".to_string()),
            ],
        )
        .expect("enabled down-facing hopper state");
    let comparator_off_state_id = blocks
        .by_name_and_props(
            &comparator_id,
            &[
                ("facing".to_string(), "west".to_string()),
                ("mode".to_string(), "compare".to_string()),
                ("powered".to_string(), "false".to_string()),
            ],
        )
        .expect("unpowered west-facing comparator state");
    let comparator_on_state_id = blocks
        .by_name_and_props(
            &comparator_id,
            &[
                ("facing".to_string(), "west".to_string()),
                ("mode".to_string(), "compare".to_string()),
                ("powered".to_string(), "true".to_string()),
            ],
        )
        .expect("powered west-facing comparator state");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 hopper TCP chest and comparator updates".into(),
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

    let (mut source_client, sync) = connect_to_play(addr, "M100HopperSource").await;
    drain_until_chunk(&mut source_client, (0, 0)).await;
    let hopper_pos = mc_world::BlockPos {
        x: 1,
        y: sync.y.floor() as i32 - 2,
        z: 0,
    };
    let source_pos = mc_world::BlockPos {
        y: hopper_pos.y + 1,
        ..hopper_pos
    };
    let target_pos = mc_world::BlockPos {
        y: hopper_pos.y - 1,
        ..hopper_pos
    };
    let comparator_pos = mc_world::BlockPos {
        x: target_pos.x + 1,
        ..target_pos
    };
    {
        let mut world = world_handle.lock().await;
        world
            .set_block_at(source_pos, chest_state_id)
            .expect("set source chest block")
            .expect("source chunk exists");
        world
            .set_block_at(hopper_pos, hopper_state_id)
            .expect("set hopper block")
            .expect("hopper chunk exists");
        world
            .set_block_at(target_pos, chest_state_id)
            .expect("set target chest block")
            .expect("target chunk exists");
        world
            .set_block_at(comparator_pos, comparator_off_state_id)
            .expect("set comparator block")
            .expect("comparator chunk exists");

        let mut source_chest = mc_world::ChestBlockEntity::default();
        source_chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 1,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(source_pos, source_chest)
            .expect("seed source chest entity");
        world
            .set_chest_block_entity(target_pos, mc_world::ChestBlockEntity::default())
            .expect("seed target chest entity");
    }

    source_client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(source_pos.x, source_pos.y, source_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 230,
        })
        .await
        .expect("source viewer opens chest");
    let source_opened = wait_for_open_screen(&mut source_client, chest_menu_id).await;
    let source_initial =
        wait_for_furnace_content(&mut source_client, source_opened.container_id, |pkt| {
            pkt.items[0].item_id == dirt_id
                && pkt.items[0].count == 1
                && pkt.carried_item.is_empty()
        })
        .await;

    let (mut target_client, _) = connect_to_play(addr, "M100HopperTarget").await;
    drain_until_chunk(&mut target_client, (0, 0)).await;
    target_client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(target_pos.x, target_pos.y, target_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 231,
        })
        .await
        .expect("target viewer opens chest");
    let target_opened = wait_for_open_screen(&mut target_client, chest_menu_id).await;
    let target_initial =
        wait_for_furnace_content(&mut target_client, target_opened.container_id, |pkt| {
            pkt.items[0].is_empty() && pkt.carried_item.is_empty()
        })
        .await;

    {
        let mut world = world_handle.lock().await;
        world
            .set_hopper_block_entity(hopper_pos, mc_world::HopperBlockEntity::default())
            .expect("seed hopper entity");
        assert!(
            world
                .schedule_block_tick(mc_world::ScheduledBlockTick::new(
                    hopper_pos,
                    hopper_id.clone(),
                    0,
                    0,
                ))
                .expect("schedule hopper tick"),
            "hopper tick should be newly scheduled after viewers open"
        );
    }

    let source_empty = wait_for_container_slot(
        &mut source_client,
        source_opened.container_id,
        0,
        mc_protocol::packets::play::ItemStack::is_empty,
    )
    .await;
    assert!(
        source_empty.state_id > source_initial.state_id,
        "server-origin hopper pull should advance the source chest state"
    );

    let target_dirt = wait_for_container_slot(
        &mut target_client,
        target_opened.container_id,
        0,
        |stack| stack.item_id == dirt_id && stack.count == 1,
    )
    .await;
    assert!(
        target_dirt.state_id > target_initial.state_id,
        "cooldown-delayed hopper eject should advance the target chest state"
    );
    wait_for_block_update(
        &mut target_client,
        (comparator_pos.x, comparator_pos.y, comparator_pos.z),
        comparator_on_state_id.0 as i32,
    )
    .await;
}

#[tokio::test]
async fn chest_quickcraft_left_drag_splits_carried_stack_across_empty_slots() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 chest quickcraft".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100QuickCraft").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
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
            sequence: 201,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 5,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 202,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let initial = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 5
            && pkt.items[1].is_empty()
            && pkt.items[2].is_empty()
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 5,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let carrying = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > initial.state_id
            && pkt.items[0].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 5
    })
    .await;

    for (slot_num, button_num, carried_count, changed_slots) in [
        (-999, 0, 5, Vec::new()),
        (
            1,
            1,
            5,
            vec![(
                1,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 2,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            2,
            1,
            5,
            vec![(
                2,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 2,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            -999,
            2,
            1,
            vec![
                (
                    1,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 2,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
                (
                    2,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 2,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
            ],
        ),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: carrying.state_id,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots,
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: carried_count,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send chest quickcraft stage");
    }

    let final_content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > carrying.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 2
            && pkt.items[2].item_id == dirt_id
            && pkt.items[2].count == 2
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 1
    })
    .await;
    assert!(
        final_content.state_id > carrying.state_id,
        "QuickCraft end should advance the chest state id after storage mutation"
    );

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert!(chest.slots[0].is_empty());
    assert_eq!(chest.slots[1].item_id, dirt_id);
    assert_eq!(chest.slots[1].count, 2);
    assert_eq!(chest.slots[2].item_id, dirt_id);
    assert_eq!(chest.slots[2].count, 2);
}

#[tokio::test]
async fn chest_quickcraft_right_drag_places_one_per_selected_slot_and_merges_partial_stack() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 chest right quickcraft".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100RightQC").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
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
            sequence: 203,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 5,
            damage: None,
            enchantments: Vec::new(),
        };
        chest.slots[1] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 63,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 204,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let initial = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id
            && pkt.items[0].count == 5
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 63
            && pkt.items[2].is_empty()
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: initial.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 5,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let carrying = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > initial.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 63
            && pkt.items[2].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 5
    })
    .await;

    for (slot_num, button_num, carried_count, changed_slots) in [
        (-999, 4, 5, Vec::new()),
        (
            1,
            5,
            5,
            vec![(
                1,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 64,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            2,
            5,
            5,
            vec![(
                2,
                HashedStack::Actual {
                    item_id: dirt_id,
                    count: 1,
                    components: HashedStackComponentHashes::empty(),
                },
            )],
        ),
        (
            -999,
            6,
            3,
            vec![
                (
                    1,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 64,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
                (
                    2,
                    HashedStack::Actual {
                        item_id: dirt_id,
                        count: 1,
                        components: HashedStackComponentHashes::empty(),
                    },
                ),
            ],
        ),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: carrying.state_id,
                slot_num,
                button_num,
                container_input: ContainerInput::QuickCraft,
                changed_slots,
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: carried_count,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send chest right quickcraft stage");
    }

    let final_content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.state_id > carrying.state_id
            && pkt.items[0].is_empty()
            && pkt.items[1].item_id == dirt_id
            && pkt.items[1].count == 64
            && pkt.items[2].item_id == dirt_id
            && pkt.items[2].count == 1
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 3
    })
    .await;
    assert!(
        final_content.state_id > carrying.state_id,
        "right QuickCraft end should advance the chest state id after storage mutation"
    );

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert!(chest.slots[0].is_empty());
    assert_eq!(chest.slots[1].item_id, dirt_id);
    assert_eq!(chest.slots[1].count, 64);
    assert_eq!(chest.slots[2].item_id, dirt_id);
    assert_eq!(chest.slots[2].count, 1);
}

#[tokio::test]
async fn unsupported_chest_click_modes_resync_without_trusting_client_slots() {
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
    let chest_id = mc_data::Identifier::parse("minecraft:chest").unwrap();
    let chest_state_id = i32::try_from(blocks.block(&chest_id).expect("chest block").default.0)
        .expect("chest state id fits i32");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let chest_item_id = items.id_of(&chest_id).expect("chest item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let chest_menu_id = 2;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed chest click".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100BadChest").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:chest 1 0".into(),
        })
        .await
        .expect("give chest");
    wait_for_slot_stack(&mut client, chest_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let chest_y = support_y + 1;
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
            sequence: 197,
        })
        .await
        .expect("place chest");
    wait_for_block_update(&mut client, (0, chest_y, 0), chest_state_id).await;

    let chest_pos = mc_world::BlockPos {
        x: 0,
        y: chest_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut chest = mc_world::ChestBlockEntity::default();
        chest.slots[0] = mc_world::FurnaceSlot {
            item_id: dirt_id,
            count: 3,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_chest_block_entity(chest_pos, chest)
            .expect("seed chest entity");
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 198,
        })
        .await
        .expect("open chest");
    let opened = wait_for_open_screen(&mut client, chest_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == dirt_id && pkt.items[0].count == 3 && pkt.carried_item.is_empty()
    })
    .await;

    for (container_input, button_num) in [
        (ContainerInput::QuickCraft, 0),
        (ContainerInput::Clone, 2),
        (ContainerInput::PickupAll, 0),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: 0,
                button_num,
                container_input,
                changed_slots: vec![(0, HashedStack::empty())],
                carried_item: HashedStack::Actual {
                    item_id: dirt_id,
                    count: 3,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send unsupported chest click");
        wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
            pkt.state_id == content.state_id
                && pkt.items[0].item_id == dirt_id
                && pkt.items[0].count == 3
                && pkt.carried_item.is_empty()
        })
        .await;
    }

    let mut world = world_handle.lock().await;
    let chest = world
        .chest_block_entity(chest_pos)
        .expect("read chest entity")
        .expect("chest entity present");
    assert_eq!(chest.slots[0].item_id, dirt_id);
    assert_eq!(chest.slots[0].count, 3);
}

#[tokio::test]
async fn malformed_furnace_clicks_resync_without_trusting_client_slots() {
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|block| block.default.0 as i32)
        .expect("furnace in registry");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
    let raw_iron_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:raw_iron").unwrap())
        .expect("raw_iron item");
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed furnace click".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M100BadFurn").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut client, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
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
            sequence: 199,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut client, (0, furnace_y, 0), furnace_state_id).await;

    let furnace_pos = mc_world::BlockPos {
        x: 0,
        y: furnace_y,
        z: 0,
    };
    {
        let mut world = world_handle.lock().await;
        let mut furnace = mc_world::FurnaceBlockEntity::default();
        furnace.slots[0] = mc_world::FurnaceSlot {
            item_id: raw_iron_id,
            count: 3,
            damage: None,
            enchantments: Vec::new(),
        };
        world
            .set_furnace_block_entity(furnace_pos, furnace)
            .expect("seed furnace entity");
    }

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(furnace_pos.x, furnace_pos.y, furnace_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 200,
        })
        .await
        .expect("open furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id && pkt.items[0].count == 3 && pkt.carried_item.is_empty()
    })
    .await;

    for (container_input, button_num) in [
        (ContainerInput::QuickCraft, 0),
        (ContainerInput::Clone, 2),
        (ContainerInput::PickupAll, 0),
    ] {
        client
            .write_packet(&ServerboundContainerClick {
                container_id: opened.container_id,
                state_id: content.state_id,
                slot_num: 0,
                button_num,
                container_input,
                changed_slots: vec![(0, HashedStack::empty())],
                carried_item: HashedStack::Actual {
                    item_id: raw_iron_id,
                    count: 3,
                    components: HashedStackComponentHashes::empty(),
                },
            })
            .await
            .expect("send unsupported furnace click");
        wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
            pkt.state_id == content.state_id
                && pkt.items[0].item_id == raw_iron_id
                && pkt.items[0].count == 3
                && pkt.carried_item.is_empty()
        })
        .await;
    }

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(0, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send pickup with impossible carried item");
    let resync = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    assert_eq!(
        resync.state_id, content.state_id,
        "malformed pickup should resync without advancing furnace state"
    );
    assert_eq!(resync.items[0].item_id, raw_iron_id);
    assert_eq!(resync.items[0].count, 3);
    assert!(
        resync.carried_item.is_empty(),
        "server cursor should stay authoritative instead of trusting client carried item"
    );
    assert!(
        resync
            .items
            .iter()
            .enumerate()
            .skip(3)
            .all(|(_, stack)| stack.item_id != raw_iron_id),
        "malformed pickup must not move furnace input into player inventory slots"
    );

    let mut world = world_handle.lock().await;
    let furnace = world
        .furnace_block_entity(furnace_pos)
        .expect("read furnace entity")
        .expect("furnace entity present");
    assert_eq!(furnace.slots[0].item_id, raw_iron_id);
    assert_eq!(furnace.slots[0].count, 3);
}

#[tokio::test]
async fn survival_furnace_container_smelts_input_with_fuel() {
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
    let furnace_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("furnace in registry");
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
    let raw_iron = mc_data::Identifier::parse("minecraft:raw_iron").unwrap();
    let iron_ingot = mc_data::Identifier::parse("minecraft:iron_ingot").unwrap();
    let furnace_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
        .expect("furnace item");
    let raw_iron_id = items
        .id_of(&raw_iron)
        .expect("raw_iron item");
    let coal_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:coal").unwrap())
        .expect("coal item");
    let iron_ingot_id = items
        .id_of(&iron_ingot)
        .expect("iron_ingot item");
    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: mc_data::Identifier::parse("minecraft:test_raw_iron_smelting").unwrap(),
        kind: mc_data::recipes::RecipeKind::Smelting(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(raw_iron)],
            },
            cooking_time: 4,
            experience_milli: 1_000,
        }),
        result: mc_data::recipes::RecipeResult {
            item: iron_ingot,
            count: 1,
        },
    }]);
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let experience_orb_type_id = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:experience_orb").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("experience orb entity type");
    let furnace_menu_id = 14;

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 smelting".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes,
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

    let (mut client, sync) = connect_to_play(addr, "M23Smelter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:furnace 1 0".into(),
        })
        .await
        .expect("give furnace");
    wait_for_slot_stack(&mut client, furnace_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let furnace_y = support_y + 1;
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
            sequence: 91,
        })
        .await
        .expect("place furnace");
    wait_for_block_update(&mut client, (0, furnace_y, 0), furnace_state_id).await;

    let (mut observer, observer_sync) = connect_to_play(addr, "M24FurnaceViewer").await;
    drain_until_chunk(&mut observer, (0, 0)).await;
    observer
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 90,
        })
        .await
        .expect("observer opens furnace");
    let observer_opened = wait_for_open_screen(&mut observer, furnace_menu_id).await;
    wait_for_furnace_content(&mut observer, observer_opened.container_id, |pkt| {
        pkt.items[0].is_empty() && pkt.items[1].is_empty() && pkt.items[2].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:raw_iron 1 0".into(),
        })
        .await
        .expect("give raw_iron");
    wait_for_slot_stack(&mut client, raw_iron_id, 1).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:coal 1 1".into(),
        })
        .await
        .expect("give coal");
    wait_for_slot_stack(&mut client, coal_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 92,
        })
        .await
        .expect("open furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |_| true).await;
    assert_eq!(content.items.len(), 39);

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 30,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: raw_iron_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up raw iron");
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == raw_iron_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place raw iron input");
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;
    wait_for_container_slot(&mut observer, observer_opened.container_id, 0, |stack| {
        stack.item_id == raw_iron_id && stack.count == 1
    })
    .await;

    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close furnace after input insert");
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, furnace_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 93,
        })
        .await
        .expect("reopen furnace");
    let opened = wait_for_open_screen(&mut client, furnace_menu_id).await;
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.items[0].item_id == raw_iron_id
            && pkt.items[0].count == 1
            && pkt.carried_item.is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 31,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: coal_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up coal");
    let content = wait_for_furnace_content(&mut client, opened.container_id, |pkt| {
        pkt.carried_item.item_id == coal_id && pkt.carried_item.count == 1
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place coal fuel");
    wait_for_container_slot(&mut observer, observer_opened.container_id, 1, |stack| {
        stack.item_id == coal_id && stack.count == 1
    })
    .await;

    wait_for_furnace_data(&mut client, opened.container_id, 2, |value| value > 0).await;
    wait_for_furnace_data(&mut observer, observer_opened.container_id, 2, |value| {
        value > 0
    })
    .await;
    let output = wait_for_container_slot(&mut client, opened.container_id, 2, |stack| {
        stack.item_id == iron_ingot_id && stack.count == 1
    })
    .await;
    let observer_x = observer_sync.x + 4.0;
    observer
        .write_packet(&ServerboundChatCommand {
            command: format!("tp {observer_x} {} {}", observer_sync.y, observer_sync.z),
        })
        .await
        .expect("move furnace observer outside XP pickup radius");
    let observer_position = wait_for_position_correction(&mut observer, Duration::from_secs(2)).await;
    assert_position_near(
        &observer_position,
        observer_x,
        observer_sync.y,
        observer_sync.z,
        0.001,
    );
    observer
        .write_packet(&ConfirmTeleportation {
            teleport_id: observer_position.teleport_id,
        })
        .await
        .expect("confirm furnace observer position");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: output.state_id,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("shift-click result");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_inventory_result = false;
    let mut saw_experience_orb = false;
    let mut saw_experience_credit = false;
    while !saw_inventory_result || !saw_experience_orb || !saw_experience_credit {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "furnace result/XP events failed: inventory={saw_inventory_result} \
                     orb={saw_experience_orb} credit={saw_experience_credit}: {error}"
                )
            });
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode furnace result content");
            saw_inventory_result |= packet.container_id == opened.container_id
                && packet.items[2].is_empty()
                && packet
                    .items
                    .iter()
                    .skip(3)
                    .any(|stack| stack.item_id == iron_ingot_id && stack.count == 1);
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode furnace experience orb");
            saw_experience_orb |= packet.entity_type_id == experience_orb_type_id
                && packet.data == 1;
        } else if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode furnace experience credit");
            saw_experience_credit |= packet.total_experience == 1;
        }
    }
}

#[tokio::test]
async fn survival_enchanting_table_applies_high_efficiency_sharpness_and_protection() {
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
    let enchanting_table = mc_data::Identifier::parse("minecraft:enchanting_table").unwrap();
    let enchanting_table_state_id = blocks
        .block(&enchanting_table)
        .expect("enchanting table block")
        .default;
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .expect("air block")
        .default;
    let bookshelf_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:bookshelf").unwrap())
        .expect("bookshelf block")
        .default;
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let mut storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let seeded_support_y =
        top_non_air_y(&mut storage, 0, 0, air_state_id).expect("spawn column terrain");
    let seeded_table_y = seeded_support_y + 1;
    let mut seeded_bookshelves = 0;
    'bookshelves: for x in -2_i32..=2 {
        for z in -2_i32..=2 {
            if x.abs() != 2 && z.abs() != 2 {
                continue;
            }
            if seeded_bookshelves == 15 {
                break 'bookshelves;
            }
            storage
                .set_block_at(
                    mc_world::BlockPos {
                        x,
                        y: seeded_table_y,
                        z,
                    },
                    bookshelf_state_id,
                )
                .expect("seed bookshelf")
                .expect("replace bookshelf position");
            storage
                .set_block_at(
                    mc_world::BlockPos {
                        x: x / 2,
                        y: seeded_table_y,
                        z: z / 2,
                    },
                    air_state_id,
                )
                .expect("clear bookshelf midpoint")
                .expect("replace bookshelf midpoint");
            seeded_bookshelves += 1;
        }
    }
    assert_eq!(seeded_bookshelves, 15);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let table_id = items.id_of(&enchanting_table).expect("enchanting table item");
    let pickaxe_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone_pickaxe").unwrap())
        .expect("stone pickaxe item");
    let sword_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:stone_sword").unwrap())
        .expect("stone sword item");
    let chestplate_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap())
        .expect("iron chestplate item");
    let lapis_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:lapis_lazuli").unwrap())
        .expect("lapis item");
    let sharpness = mc_data::Identifier::parse("minecraft:sharpness").unwrap();
    let sharpness_clue = i16::try_from(
        mc_data::required_registry_entry_id("minecraft:enchantment", &sharpness)
            .expect("sharpness registry id"),
    )
    .expect("sharpness clue fits i16");
    let protection = mc_data::Identifier::parse("minecraft:protection").unwrap();
    let protection_clue = i16::try_from(
        mc_data::required_registry_entry_id("minecraft:enchantment", &protection)
            .expect("protection registry id"),
    )
    .expect("protection clue fits i16");
    let item_facts = Arc::new(
        mc_data::item_components::load_item_facts(
            vanilla_dir.join("reports/minecraft/components/item"),
        )
        .expect("item facts load"),
    );
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "playable enchanting".into(),
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
        item_facts,
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
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

    let (mut client, sync) = connect_to_play(addr, "Enchanter").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    for (item, count, slot, item_id) in [
        ("minecraft:stone_pickaxe", 1, 0, pickaxe_id),
        ("minecraft:lapis_lazuli", 9, 1, lapis_id),
        ("minecraft:enchanting_table", 1, 2, table_id),
        ("minecraft:stone_sword", 1, 3, sword_id),
        ("minecraft:iron_chestplate", 1, 4, chestplate_id),
    ] {
        client
            .write_packet(&ServerboundChatCommand {
                command: format!("debug give {item} {count} {slot}"),
            })
            .await
            .expect("debug give enchanting input");
        wait_for_slot_stack(&mut client, item_id, count).await;
    }
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug survival xp 2203".into(),
        })
        .await
        .expect("grant thirty-six enchanting levels");
    wait_for_experience(&mut client, |xp| {
        xp.total_experience == 2_203 && xp.experience_level == 36
    })
    .await;

    let support_y = sync.y.floor() as i32 - 2;
    let table_y = support_y + 1;
    assert_eq!(support_y, seeded_support_y);
    assert_eq!(table_y, seeded_table_y);
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 2 })
        .await
        .expect("select enchanting table");
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
            sequence: 201,
        })
        .await
        .expect("place enchanting table");
    wait_for_block_update(
        &mut client,
        (0, table_y, 0),
        enchanting_table_state_id.0 as i32,
    )
    .await;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, table_y, 0),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 202,
        })
        .await
        .expect("open enchanting table");
    let opened = wait_for_open_screen(&mut client, 13).await;
    let mut content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items.len() == 38 && packet.items[0].is_empty() && packet.items[1].is_empty()
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move pickaxe into enchanting slot");
    content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].item_id == pickaxe_id && packet.items[29].is_empty()
    })
    .await;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 1,
            button_num: 1,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move lapis into enchanting slot");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[1].item_id == lapis_id
            && packet.items[1].count == 9
            && packet.items[30].is_empty()
    })
    .await;

    let mut enchanting_data = [None; 10];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while enchanting_data.iter().any(Option::is_none) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("bookshelf-powered enchanting data");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundContainerSetData::decode(&mut body).expect("decode enchanting data");
            if packet.container_id == opened.container_id
                && let Ok(index) = usize::try_from(packet.id)
                && index < enchanting_data.len()
            {
                enchanting_data[index] = Some(packet.value);
            }
        }
    }
    assert_eq!(
        enchanting_data,
        [
            Some(1),
            Some(10),
            Some(30),
            Some(0),
            Some(8),
            Some(8),
            Some(8),
            Some(1),
            Some(2),
            Some(3),
        ]
    );

    client
        .write_packet(&ServerboundContainerButtonClick {
            container_id: opened.container_id,
            button_id: 2,
        })
        .await
        .expect("select high Efficiency offer");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut xp_spent = false;
    let mut enchanted_content = None;
    while !xp_spent || enchanted_content.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("enchanting result events");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode enchanting experience");
            xp_spent |= packet.total_experience == 2_203 && packet.experience_level == 33;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode enchanting content");
            let efficiency = packet.items.first().and_then(|stack| {
                stack
                    .enchantments
                    .iter()
                    .find(|enchantment| enchantment.id.as_str() == "minecraft:efficiency")
            });
            if packet.container_id == opened.container_id
                && packet.items[1].item_id == lapis_id
                && packet.items[1].count == 6
                && efficiency.is_some_and(|enchantment| enchantment.level == 3)
            {
                enchanted_content = Some(packet);
            }
        }
    }
    content = enchanted_content.unwrap();

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("return enchanted pickaxe to player");
    content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].is_empty()
            && packet.items.iter().skip(2).any(|stack| {
                stack.item_id == pickaxe_id
                    && stack.enchantments.iter().any(|enchantment| {
                        enchantment.id.as_str() == "minecraft:efficiency"
                            && enchantment.level == 3
                    })
            })
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 3,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move sword into enchanting slot");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].item_id == sword_id && packet.items[0].enchantments.is_empty()
    })
    .await;

    let mut sharpness_data = [None; 10];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while sharpness_data.iter().any(Option::is_none) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("sharpness enchanting data");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundContainerSetData::decode(&mut body).expect("decode sharpness data");
            if packet.container_id == opened.container_id
                && let Ok(index) = usize::try_from(packet.id)
                && index < sharpness_data.len()
            {
                sharpness_data[index] = Some(packet.value);
            }
        }
    }
    assert_eq!(&sharpness_data[0..3], &[Some(1), Some(10), Some(30)]);
    assert!(sharpness_data[3].is_some(), "enchantment seed is present");
    assert_eq!(
        &sharpness_data[4..7],
        &[Some(sharpness_clue), Some(sharpness_clue), Some(sharpness_clue)]
    );
    assert_eq!(&sharpness_data[7..10], &[Some(1), Some(2), Some(3)]);

    client
        .write_packet(&ServerboundContainerButtonClick {
            container_id: opened.container_id,
            button_id: 2,
        })
        .await
        .expect("select high Sharpness offer");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut sharpness_xp_spent = false;
    let mut sharpness_content = None;
    while !sharpness_xp_spent || sharpness_content.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("sharpness result events");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetExperience::decode(&mut body).expect("decode sharpness experience");
            sharpness_xp_spent |=
                packet.total_experience == 2_203 && packet.experience_level == 30;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode sharpness content");
            let applied = packet.items.first().and_then(|stack| {
                stack
                    .enchantments
                    .iter()
                    .find(|enchantment| enchantment.id == sharpness)
            });
            if packet.container_id == opened.container_id
                && packet.items[1].item_id == lapis_id
                && packet.items[1].count == 3
                && applied.is_some_and(|enchantment| enchantment.level == 3)
            {
                sharpness_content = Some(packet);
            }
        }
    }
    content = sharpness_content.unwrap();

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("return enchanted sword to player");
    content = wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].is_empty()
            && packet.items.iter().skip(2).any(|stack| {
                stack.item_id == sword_id
                    && stack.enchantments.iter().any(|enchantment| {
                        enchantment.id == sharpness && enchantment.level == 3
                    })
            })
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 4,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move chestplate into enchanting slot");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].item_id == chestplate_id && packet.items[0].enchantments.is_empty()
    })
    .await;

    let mut protection_data = [None; 10];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while protection_data.iter().any(Option::is_none) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("protection enchanting data");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundContainerSetData::decode(&mut body).expect("decode protection data");
            if packet.container_id == opened.container_id
                && let Ok(index) = usize::try_from(packet.id)
                && index < protection_data.len()
            {
                protection_data[index] = Some(packet.value);
            }
        }
    }
    assert_eq!(&protection_data[0..3], &[Some(1), Some(10), Some(30)]);
    assert!(protection_data[3].is_some(), "enchantment seed is present");
    assert_eq!(
        &protection_data[4..7],
        &[
            Some(protection_clue),
            Some(protection_clue),
            Some(protection_clue),
        ]
    );
    assert_eq!(&protection_data[7..10], &[Some(1), Some(2), Some(3)]);

    client
        .write_packet(&ServerboundContainerButtonClick {
            container_id: opened.container_id,
            button_id: 2,
        })
        .await
        .expect("select high Protection offer");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut protection_xp_spent = false;
    let mut protection_content = None;
    while !protection_xp_spent || protection_content.is_none() {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("protection result events");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSetExperience::ID {
            let mut body = frame.body;
            let packet = ClientboundSetExperience::decode(&mut body)
                .expect("decode protection experience");
            protection_xp_spent |=
                packet.total_experience == 2_203 && packet.experience_level == 27;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode protection content");
            let applied = packet.items.first().and_then(|stack| {
                stack
                    .enchantments
                    .iter()
                    .find(|enchantment| enchantment.id == protection)
            });
            if packet.container_id == opened.container_id
                && packet.items[1].is_empty()
                && applied.is_some_and(|enchantment| enchantment.level == 3)
            {
                protection_content = Some(packet);
            }
        }
    }
    content = protection_content.unwrap();

    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: content.state_id,
            slot_num: 0,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("return enchanted chestplate to player");
    wait_for_furnace_content(&mut client, opened.container_id, |packet| {
        packet.items[0].is_empty()
            && packet.items.iter().skip(2).any(|stack| {
                stack.item_id == chestplate_id
                    && stack.enchantments.iter().any(|enchantment| {
                        enchantment.id == protection && enchantment.level == 3
                    })
            })
    })
    .await;
}

#[tokio::test]
async fn survival_specialized_furnaces_open_vanilla_menu_types() {
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
    let block_state = |name: &str| {
        blocks
            .block(&mc_data::Identifier::parse(name).unwrap())
            .map(|block| block.default.0 as i32)
            .expect("block in registry")
    };
    let smoker_state_id = block_state("minecraft:smoker");
    let blast_furnace_state_id = block_state("minecraft:blast_furnace");
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M71 specialized furnaces".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M71Furnaces").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let furnace_y = sync.y.floor() as i32 - 1;
    let cases = [
        (
            smoker_state_id,
            22,
            mc_world::BlockPos {
                x: 1,
                y: furnace_y,
                z: 0,
            },
            171,
        ),
        (
            blast_furnace_state_id,
            10,
            mc_world::BlockPos {
                x: 2,
                y: furnace_y,
                z: 0,
            },
            172,
        ),
    ];
    {
        let mut storage = world_handle.lock().await;
        for &(state_id, _, pos, _) in &cases {
            storage
                .set_block_at(pos, mc_world::BlockStateId(state_id as u32))
                .expect("seed specialized furnace block")
                .expect("generated spawn chunk exists");
        }
    }

    for (_, menu_id, pos, sequence) in cases {
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(pos.x, pos.y, pos.z),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence: sequence + 10,
            })
            .await
            .expect("open specialized furnace");
        let opened = wait_for_open_screen(&mut client, menu_id).await;
        client
            .write_packet(&ServerboundContainerClose {
                container_id: opened.container_id,
            })
            .await
            .expect("close specialized furnace");
    }
}

#[tokio::test]
async fn survival_container_click_moves_stack_through_server_cursor() {
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
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M23 container click".into(),
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

    let (mut client, _) = connect_to_play(addr, "M23Clicker").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 10 0".into(),
        })
        .await
        .expect("give dirt");
    let slot = wait_for_slot_stack_update(&mut client, dirt_id, 10).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: slot.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 10,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("pick up dirt stack");
    let content = wait_for_inventory_content(&mut client, |pkt| {
        pkt.items[36].is_empty()
            && pkt.carried_item.item_id == dirt_id
            && pkt.carried_item.count == 10
    })
    .await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: content.state_id,
            slot_num: 37,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("place dirt stack");
    wait_for_inventory_content(&mut client, |pkt| {
        pkt.carried_item.is_empty()
            && pkt.items[36].is_empty()
            && pkt.items[37].item_id == dirt_id
            && pkt.items[37].count == 10
    })
    .await;
}

#[tokio::test]
async fn malformed_inventory_click_resyncs_without_advancing_state() {
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
    let dirt_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 malformed inventory click".into(),
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

    let (mut client, _) = connect_to_play(addr, "M100BadInv").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 10 0".into(),
        })
        .await
        .expect("give dirt");
    let slot = wait_for_slot_stack_update(&mut client, dirt_id, 10).await;

    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: slot.state_id,
            slot_num: 36,
            button_num: 0,
            container_input: ContainerInput::Pickup,
            changed_slots: vec![(36, HashedStack::empty())],
            carried_item: HashedStack::Actual {
                item_id: dirt_id,
                count: 1,
                components: HashedStackComponentHashes::empty(),
            },
        })
        .await
        .expect("send pickup with impossible carried item");
    let resync = wait_for_inventory_content(&mut client, |pkt| {
        pkt.items[36].item_id == dirt_id && pkt.items[36].count == 10 && pkt.carried_item.is_empty()
    })
    .await;
    assert_eq!(
        resync.state_id, slot.state_id,
        "malformed inventory click should resync without advancing inventory state"
    );
    assert!(
        resync
            .items
            .iter()
            .enumerate()
            .filter(|(slot, _)| *slot != 36)
            .all(|(_, stack)| stack.item_id != dirt_id),
        "malformed inventory click must not move dirt into another inventory slot"
    );
}

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
    let station_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:smithing_table").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("smithing table in registry");
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
    let station_y = support_y + 1;
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
            sequence: 701,
        })
        .await
        .expect("place smithing table");
    wait_for_block_update(&mut client, (0, station_y, 0), station_state_id).await;

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
            position: pack_block_pos(0, station_y, 0),
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
    read_station_noop_ack(&mut client, 702, (0, station_y + 1, 0), dirt_state_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    for (sequence, x) in [(703, 1), (704, 2)] {
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(x, support_y, 0),
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
        wait_for_block_update(&mut client, (x, station_y, 0), dirt_state_id).await;
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
                assert_eq!(target_updates, 1, "torch placement publishes one target update");
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
            let packet = ClientboundContainerSetSlot::decode(&mut body).expect("decode torch debit");
            if packet.container_id == 0 && packet.slot == 36 && packet.item_stack.is_empty() {
                assert!(saw_ack, "torch debit follows acknowledgement");
                debits += 1;
                assert_eq!(debits, 1, "torch placement debits exactly once");
            }
        }
    }
}
