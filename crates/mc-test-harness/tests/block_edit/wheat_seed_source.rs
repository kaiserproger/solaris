#[tokio::test]
async fn embedded_short_grass_break_delivers_wheat_seeds_over_wire() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let short_grass_state = embedded_block_state(&data, "minecraft:short_grass");
    let wheat_seeds_id = embedded_item_id(&data, "minecraft:wheat_seeds");

    let mut world = embedded_world(&data);
    let spawn_surface_y =
        top_non_air_y(&mut world, 1, 1, air_state).expect("target column terrain");
    let target = (1, spawn_surface_y + 1, 1);
    world
        .set_block_at(
            mc_world::BlockPos {
                x: target.0,
                y: target.1,
                z: target.2,
            },
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

    wait_for_slot_stack(&mut client, wheat_seeds_id, 1).await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("seed source server shutdown")
        .expect("seed source server join")
        .expect("seed source server serve");
}
