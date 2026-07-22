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
    let floor_y = surface_y + 1;
    // Solaris' safe spawn keeps one full clearance block above the support.
    let player_y = floor_y + 2;
    for x in -2..=5 {
        for z in -2..=5 {
            world
                .set_block_at(
                    mc_world::BlockPos {
                        x,
                        y: floor_y,
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
