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

