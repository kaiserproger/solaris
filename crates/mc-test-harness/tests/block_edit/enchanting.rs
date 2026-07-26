#[test]
fn survival_enchanting_table_applies_high_efficiency_sharpness_and_protection() {
    let test = std::thread::Builder::new()
        .name("survival_enchanting_table_applies_high_efficiency_sharpness_and_protection".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build enchanting integration runtime")
                .block_on(survival_enchanting_table_applies_high_efficiency_sharpness_and_protection_inner());
        })
        .expect("spawn enchanting integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn survival_enchanting_table_applies_high_efficiency_sharpness_and_protection_inner() {
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
    let table_id = items
        .id_of(&enchanting_table)
        .expect("enchanting table item");
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
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
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
            let packet =
                ClientboundSetExperience::decode(&mut body).expect("decode enchanting experience");
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
                        enchantment.id.as_str() == "minecraft:efficiency" && enchantment.level == 3
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
        &[
            Some(sharpness_clue),
            Some(sharpness_clue),
            Some(sharpness_clue)
        ]
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
            sharpness_xp_spent |= packet.total_experience == 2_203 && packet.experience_level == 30;
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
                    && stack
                        .enchantments
                        .iter()
                        .any(|enchantment| enchantment.id == sharpness && enchantment.level == 3)
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
            let packet =
                ClientboundSetExperience::decode(&mut body).expect("decode protection experience");
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
                    && stack
                        .enchantments
                        .iter()
                        .any(|enchantment| enchantment.id == protection && enchantment.level == 3)
            })
    })
    .await;
}
