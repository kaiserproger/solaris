const ITEM_LOCK_GATE_ACTIONS: usize = 200;
const ITEM_LOCK_GATE_COLUMNS: usize = 14;
const ITEM_LOCK_GATE_MAX_LOCK_US: u64 = 5_000;
const ITEM_LOCK_GATE_MAX_TICK_US: u64 = 50_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-candidate 200-action break/drop/pickup lock and latency gate"]
async fn two_hundred_torch_break_drop_pickups_stay_below_lock_and_tick_budgets() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let stone_state = embedded_block_state(&data, "minecraft:stone");
    let torch_state = embedded_block_state(&data, "minecraft:torch");
    let torch_item = embedded_item_id(&data, "minecraft:torch");

    let mut world = embedded_world(&data);
    let generated_surface =
        top_non_air_y(&mut world, 0, 0, air_state).expect("generated spawn column terrain");
    let floor_y = generated_surface + 1;
    let torch_y = floor_y + 1;
    let rows = ITEM_LOCK_GATE_ACTIONS.div_ceil(ITEM_LOCK_GATE_COLUMNS);
    let mut targets = Vec::with_capacity(ITEM_LOCK_GATE_ACTIONS);
    for row in 0..rows {
        for offset in 0..ITEM_LOCK_GATE_COLUMNS {
            if targets.len() == ITEM_LOCK_GATE_ACTIONS {
                break;
            }
            let column = if row % 2 == 0 {
                offset
            } else {
                ITEM_LOCK_GATE_COLUMNS - 1 - offset
            };
            let x = i32::try_from(column).expect("bounded gate column");
            let z = i32::try_from(row).expect("bounded gate row");
            world
                .set_block_at(mc_world::BlockPos { x, y: floor_y, z }, stone_state)
                .expect("seed item lock gate floor");
            world
                .set_block_at(mc_world::BlockPos { x, y: torch_y, z }, torch_state)
                .expect("seed item lock gate torch");
            for y in (torch_y + 1)..=(torch_y + 2) {
                world
                    .set_block_at(mc_world::BlockPos { x, y, z }, air_state)
                    .expect("clear item lock gate body space");
            }
            targets.push((x, z));
        }
    }

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P0 item lock latency gate");
    cfg.loot = Arc::new(mc_data::loot::LootTables::from_maps(
        std::collections::BTreeMap::new(),
        std::collections::BTreeMap::from([(
            mc_data::Identifier::parse("minecraft:torch").unwrap(),
            mc_data::Identifier::parse("minecraft:torch").unwrap(),
        )]),
    ));
    cfg.shutdown = shutdown.clone();
    let item_entity_type = cfg
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .expect("embedded item entity type") as i32;
    let bound = mc_net::bind(cfg).await.expect("bind item lock gate server");
    let addr = bound.local_addr().expect("item lock gate local address");
    let telemetry = bound.runtime_telemetry_handle();
    let mut ticks = telemetry.subscribe_simulation_ticks();
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, sync) = connect_to_play(addr, "P0ItemLockGate").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let initial_window = wait_for_item_lock_tick_window(&telemetry, &mut ticks, 1).await;
    let baseline_source_tick = initial_window.source_tick;
    let baseline_telemetry = telemetry.snapshot();
    mc_net::reset_lock_pressure_metrics();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    for (index, (x, z)) in targets.into_iter().enumerate() {
        client
            .write_packet(&ServerboundMovePlayerPosRot {
                x: f64::from(x) + 0.5,
                y: sync.y,
                z: f64::from(z) + 0.5,
                yaw: 0.0,
                pitch: 90.0,
                flags: MovePlayerFlags::new(true, false),
            })
            .await
            .expect("move to item lock gate torch");
        let sequence = i32::try_from(index + 1).expect("bounded gate sequence");
        let target = (x, torch_y, z);
        client
            .write_packet(&ServerboundPlayerAction {
                action: PlayerActionKind::StartDestroyBlock,
                position: pack_block_pos(target.0, target.1, target.2),
                direction: Direction::Up,
                sequence,
            })
            .await
            .expect("break item lock gate torch");
        wait_for_item_lock_pickup(
            &mut client,
            deadline,
            sequence,
            target,
            air_state.0 as i32,
            item_entity_type,
            torch_item,
        )
        .await;
    }

    let action_end_tick = *ticks.borrow();
    let final_source_tick = baseline_source_tick + 1_200;
    assert!(
        action_end_tick <= final_source_tick,
        "200 item actions exceeded the 1,200-tick metric window: baseline={baseline_source_tick} end={action_end_tick}"
    );
    let final_window =
        wait_for_item_lock_tick_window(&telemetry, &mut ticks, final_source_tick).await;
    assert_eq!(
        final_window.source_tick, final_source_tick,
        "the exact 1,200-tick item gate window must publish without a skipped interval"
    );
    assert_eq!(final_window.tick.samples, 1_200);
    assert!(
        final_window.tick.max_us < ITEM_LOCK_GATE_MAX_TICK_US,
        "item gate tick exceeded 50 ms: {:?}",
        final_window.tick
    );

    let locks = mc_net::lock_pressure_snapshot();
    assert!(
        locks.session_registry.hold_count >= ITEM_LOCK_GATE_ACTIONS as u64,
        "item gate did not exercise session publication locks: {:?}",
        locks.session_registry
    );
    assert!(
        locks.player_persistence.hold_count >= ITEM_LOCK_GATE_ACTIONS as u64,
        "item gate did not exercise player-persistence credit locks: {:?}",
        locks.player_persistence
    );
    for (name, metric) in [
        ("session_registry", locks.session_registry),
        ("player_persistence", locks.player_persistence),
    ] {
        assert!(
            metric.max_wait_us < ITEM_LOCK_GATE_MAX_LOCK_US,
            "{name} wait exceeded 5 ms: {metric:?}"
        );
        assert!(
            metric.max_hold_us < ITEM_LOCK_GATE_MAX_LOCK_US,
            "{name} hold exceeded 5 ms: {metric:?}"
        );
    }

    let final_telemetry = telemetry.snapshot();
    assert_eq!(
        final_telemetry
            .simulation_block_edits_processed
            .saturating_sub(baseline_telemetry.simulation_block_edits_processed),
        ITEM_LOCK_GATE_ACTIONS as u64
    );
    assert_eq!(
        final_telemetry
            .simulation_item_pickups_processed
            .saturating_sub(baseline_telemetry.simulation_item_pickups_processed),
        ITEM_LOCK_GATE_ACTIONS as u64
    );
    assert_eq!(
        final_telemetry
            .entity_take_dispatches
            .saturating_sub(baseline_telemetry.entity_take_dispatches),
        ITEM_LOCK_GATE_ACTIONS as u64
    );
    assert_eq!(
        final_telemetry
            .entity_remove_dispatches
            .saturating_sub(baseline_telemetry.entity_remove_dispatches),
        ITEM_LOCK_GATE_ACTIONS as u64
    );

    eprintln!(
        "P0 item lock gate actions={} ticks={:?} session={:?} player={:?}",
        ITEM_LOCK_GATE_ACTIONS,
        final_window.tick,
        locks.session_registry,
        locks.player_persistence,
    );

    shutdown.request();
    drop(client);
    tokio::time::timeout(Duration::from_secs(10), serve)
        .await
        .expect("item lock gate server shutdown timeout")
        .expect("item lock gate server task joins")
        .expect("item lock gate server exits cleanly");
}

async fn wait_for_item_lock_pickup(
    client: &mut Client,
    deadline: tokio::time::Instant,
    sequence: i32,
    target: (i32, i32, i32),
    air_state_id: i32,
    item_entity_type: i32,
    expected_item_id: u32,
) {
    let mut saw_air = false;
    let mut saw_ack = false;
    let mut entity_id = None;
    let mut saw_stack = false;
    let mut saw_take = false;
    let mut saw_remove = false;
    while !(saw_air && saw_ack && saw_stack && saw_take && saw_remove) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("item lock gate action response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode item gate block update");
            if unpack_block_pos(packet.position) == target {
                assert_eq!(packet.state_id, air_state_id);
                saw_air = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode item gate block ack");
            if packet.sequence == sequence {
                saw_ack = true;
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode item gate entity spawn");
            if packet.entity_type_id == item_entity_type {
                assert!(entity_id.replace(packet.entity_id).is_none());
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetEntityData::decode(&mut body).expect("decode item gate entity data");
            if Some(packet.entity_id) == entity_id {
                let stack = packet.values.iter().find_map(|value| match value {
                    EntityDataValue::ItemStack { index, stack }
                        if *index == ITEM_ENTITY_DATA_ITEM_INDEX =>
                    {
                        Some(stack)
                    }
                    _ => None,
                });
                if let Some(stack) = stack {
                    assert_eq!(stack.item_id, expected_item_id);
                    assert_eq!(stack.count, 1);
                    saw_stack = true;
                }
            }
        } else if frame.id == ClientboundTakeItemEntity::ID {
            let mut body = frame.body;
            let packet =
                ClientboundTakeItemEntity::decode(&mut body).expect("decode item gate pickup");
            if Some(packet.item_entity_id) == entity_id {
                assert_eq!(packet.amount, 1);
                saw_take = true;
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode item gate removal");
            if entity_id.is_some_and(|id| packet.entity_ids.contains(&id)) {
                saw_remove = true;
            }
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let correction = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode unexpected item gate correction");
            panic!("item gate movement was corrected: {correction:?}");
        }
    }
}

async fn wait_for_item_lock_tick_window(
    telemetry: &mc_net::RuntimeTelemetryHandle,
    ticks: &mut tokio::sync::watch::Receiver<u64>,
    minimum_source_tick: u64,
) -> mc_net::RuntimeTickPercentiles {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(70);
    loop {
        if let Some(window) = telemetry.snapshot().tick_percentiles
            && window.source_tick >= minimum_source_tick
        {
            return window;
        }
        tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            ticks.changed(),
        )
        .await
        .expect("runtime tick percentile publication timeout")
        .expect("simulation tick publisher closed");
    }
}
