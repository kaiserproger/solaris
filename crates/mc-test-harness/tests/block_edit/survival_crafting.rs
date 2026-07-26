#[test]
fn embedded_generated_seed_survival_crafts_tool_and_persists_without_debug() {
    let test = std::thread::Builder::new()
        .name("embedded_generated_seed_survival_crafts_tool_and_persists_without_debug".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build generated survival crafting runtime")
                .block_on(embedded_generated_seed_survival_crafts_tool_and_persists_without_debug_inner());
        })
        .expect("spawn generated survival crafting runtime");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn embedded_generated_seed_survival_crafts_tool_and_persists_without_debug_inner() {
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
    let serve = tokio::spawn(async move { bound.serve_and_save().await });

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
    let serve = tokio::spawn(async move { bound.serve_and_save().await });

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

#[test]
fn embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table() {
    let test = std::thread::Builder::new()
        .name("embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build survival crafting table runtime")
                .block_on(embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table_inner());
        })
        .expect("spawn survival crafting table runtime");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn embedded_survival_mines_logs_and_crafts_wooden_pickaxe_at_table_inner() {
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
