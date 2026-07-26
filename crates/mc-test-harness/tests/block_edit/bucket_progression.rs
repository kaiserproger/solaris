#[test]
fn embedded_bucket_recipe_composes_with_water_pickup_and_placement() {
    let test = std::thread::Builder::new()
        .name("embedded_bucket_recipe_composes_with_water_pickup_and_placement".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build bucket progression runtime")
                .block_on(embedded_bucket_recipe_composes_with_water_pickup_and_placement_inner());
        })
        .expect("spawn bucket progression thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn embedded_bucket_recipe_composes_with_water_pickup_and_placement_inner() {
    let data = embedded_play_data();
    let air_state = embedded_block_state(&data, "minecraft:air");
    let dirt_state = embedded_block_state(&data, "minecraft:dirt");
    let table_state = embedded_block_state(&data, "minecraft:crafting_table");
    let water_state = embedded_block_state(&data, "minecraft:water");
    let iron_id = embedded_item_id(&data, "minecraft:iron_ingot");
    let bucket_id = embedded_item_id(&data, "minecraft:bucket");
    let water_bucket_id = embedded_item_id(&data, "minecraft:water_bucket");
    let bucket_recipe =
        embedded_recipe_display_id(&data, "minecraft:zz_playable_zzzzzzzz_bucket");

    let mut world = embedded_world(&data);
    let surface_y = top_non_air_y(&mut world, 0, 0, air_state).expect("spawn surface");
    let table_pos = mc_world::BlockPos {
        x: 1,
        y: surface_y + 1,
        z: 0,
    };
    let source_pos = mc_world::BlockPos {
        x: -1,
        y: surface_y + 1,
        z: 0,
    };
    for x in -1..=1 {
        world
            .set_block_at(
                mc_world::BlockPos {
                    x,
                    y: surface_y,
                    z: 0,
                },
                dirt_state,
            )
            .expect("seed flat support");
        for y in surface_y + 1..=surface_y + 2 {
            world
                .set_block_at(mc_world::BlockPos { x, y, z: 0 }, air_state)
                .expect("clear interaction space");
        }
    }
    world
        .set_block_at(table_pos, table_state)
        .expect("seed crafting table");
    world
        .set_block_at(source_pos, water_state)
        .expect("seed water source");

    let shutdown = mc_net::ShutdownHandle::default();
    let mut cfg = embedded_playable_config(&data, world, "P24 earned iron bucket utility");
    cfg.shutdown = shutdown.clone();
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let serve = tokio::spawn(async move { bound.serve().await });

    let (mut client, _) = connect_to_play(addr, "P24IronBucket").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:iron_ingot 3 0".into(),
        })
        .await
        .expect("seed three iron ingots");
    wait_for_slot_stack(&mut client, iron_id, 3).await;

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
            sequence: 801,
        })
        .await
        .expect("open crafting table");
    let opened = wait_for_open_screen(&mut client, 12).await;
    wait_for_furnace_content(&mut client, opened.container_id, |pkt| pkt.items.len() == 46).await;

    client
        .write_packet(&ServerboundPlaceRecipe {
            container_id: opened.container_id,
            recipe_display_id: bucket_recipe,
            use_max_items: false,
        })
        .await
        .expect("craft bucket");
    let crafted = wait_for_slot_stack_update(&mut client, bucket_id, 1).await;
    assert_eq!(crafted.slot, 9, "crafted bucket should use first empty inventory slot");
    let consumed = wait_for_empty_inventory_slot(&mut client, 36).await;

    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close crafting table");
    client
        .write_packet(&ServerboundContainerClick {
            container_id: 0,
            state_id: consumed.state_id,
            slot_num: 9,
            button_num: 0,
            container_input: ContainerInput::Swap,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("move crafted bucket to selected hotbar slot");
    wait_for_inventory_content(&mut client, |packet| {
        packet.container_id == 0
            && packet.items[9].is_empty()
            && packet.items[36].item_id == bucket_id
            && packet.items[36].count == 1
    })
    .await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(source_pos.x, source_pos.y, source_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 802,
        })
        .await
        .expect("pick up water source");
    wait_for_bucket_transition(
        &mut client,
        802,
        source_pos,
        air_state,
        water_bucket_id,
    )
    .await;

    let support_pos = mc_world::BlockPos {
        y: surface_y,
        ..source_pos
    };
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(support_pos.x, support_pos.y, support_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 803,
        })
        .await
        .expect("place water source");
    wait_for_bucket_transition(&mut client, 803, source_pos, water_state, bucket_id).await;

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("bucket server shutdown")
        .expect("bucket server join")
        .expect("bucket server serve");
}

async fn wait_for_empty_inventory_slot(
    client: &mut Client,
    slot: i16,
) -> ClientboundContainerSetSlot {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("empty inventory slot update");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode empty inventory slot");
            if packet.container_id == 0 && packet.slot == slot && packet.item_stack.is_empty() {
                return packet;
            }
        }
    }
}

async fn wait_for_bucket_transition(
    client: &mut Client,
    sequence: i32,
    position: mc_world::BlockPos,
    block_state: mc_world::BlockStateId,
    held_item_id: u32,
) {
    let mut saw_block = false;
    let mut saw_slot = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block && saw_slot && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("bucket transition packets");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode bucket BlockUpdate");
            if unpack_block_pos(packet.position) == (position.x, position.y, position.z) {
                assert_eq!(packet.state_id, block_state.0 as i32);
                saw_block = true;
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode bucket ContainerSetSlot");
            if packet.slot == 36
                && packet.item_stack.item_id == held_item_id
                && packet.item_stack.count == 1
            {
                saw_slot = true;
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode bucket ack");
            if packet.sequence == sequence {
                saw_ack = true;
            }
        }
    }
}
