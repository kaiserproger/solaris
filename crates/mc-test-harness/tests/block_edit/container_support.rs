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

