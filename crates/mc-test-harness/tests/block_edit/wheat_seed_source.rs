#[tokio::test]
async fn embedded_short_grass_break_round_trips_update_ack_over_wire() {
    // P24 seed probability (1/8 per break) is covered deterministically by the
    // fixed-seed corpus in mc-data/tests/plant_loot.rs. Asserting a seed drop
    // here would fail ~7/8 runs (pre-existing flake, see 2026-08-30 evidence);
    // this wire test pins the deterministic break transaction instead.
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let short_grass_state = embedded_block_state(&data, "minecraft:short_grass");

    let mut world = embedded_world(&data);
    let spawn_surface_y =
        top_non_air_y(&mut world, 1, 1, air_state).expect("target column terrain");
    let target = (1, spawn_surface_y + 1, 1);
    world
        .set_block_at(
            mc_world::BlockPos { x: target.0, y: target.1, z: target.2 },
            short_grass_state,
        )
        .expect("seed short grass target");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P24 renewable wheat seed source");
    cfg.command_permissions = mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false);
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P24WheatSeeds").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    move_without_position_correction(
        &mut client,
        f64::from(target.0) + 0.5,
        f64::from(target.1),
        f64::from(target.2) + 0.5,
        0.0,
        0.0,
    )
    .await;

    let packed = pack_block_pos(target.0, target.1, target.2);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: packed,
            direction: Direction::Up,
            sequence: 701,
        })
        .await
        .expect("start breaking short grass");

    let mut saw_break = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_break && saw_ack) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("short grass break update and ack");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode grass BlockUpdate");
            if unpack_block_pos(pkt.position) == target {
                assert_eq!(pkt.state_id, air_state.0 as i32, "grass broke to air");
                saw_break = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode grass break ack");
            if pkt.sequence == 701 {
                saw_ack = true;
            }
        }
    }

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("seed source server shutdown")
        .expect("seed source server join")
        .expect("seed source server serve");
}
