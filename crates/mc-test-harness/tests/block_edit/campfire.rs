#[tokio::test]
async fn survival_campfire_cooks_held_input_into_item_entity() {
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
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

    let ident = |value: &str| mc_data::Identifier::parse(value).unwrap();
    let campfire_id = ident("minecraft:campfire");
    let porkchop = ident("minecraft:porkchop");
    let cooked_porkchop = ident("minecraft:cooked_porkchop");
    let campfire_state_id =
        i32::try_from(blocks.block(&campfire_id).expect("campfire block").default.0)
            .expect("campfire state id fits i32");
    let campfire_item_id = items.id_of(&campfire_id).expect("campfire item");
    let porkchop_id = items.id_of(&porkchop).expect("porkchop item");
    let cooked_porkchop_id = items
        .id_of(&cooked_porkchop)
        .expect("cooked porkchop item");
    let porkchop_name = porkchop.as_str().to_string();
    let item_entity_type = entity_types
        .id_of(&ident("minecraft:item"))
        .expect("item entity type") as i32;

    let recipes = Arc::new(vec![mc_data::recipes::Recipe {
        id: ident("minecraft:cooked_porkchop_from_campfire_cooking"),
        kind: mc_data::recipes::RecipeKind::CampfireCooking(mc_data::recipes::SmeltingRecipe {
            ingredient: mc_data::recipes::Ingredient {
                alternatives: vec![mc_data::recipes::IngredientAlternative::Item(porkchop)],
            },
            cooking_time: 4,
        }),
        result: mc_data::recipes::RecipeResult {
            item: cooked_porkchop,
            count: 1,
        },
    }]);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M55 campfire cooking".into(),
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

    let (mut client, sync) = connect_to_play(addr, "M55CampfireCook").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:campfire 1 0".into(),
        })
        .await
        .expect("give campfire");
    wait_for_slot_stack(&mut client, campfire_item_id, 1).await;

    let support_y = sync.y.floor() as i32 - 2;
    let campfire_y = support_y + 1;
    let campfire_z = 2;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, support_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 101,
        })
        .await
        .expect("place campfire");
    wait_for_block_update(&mut client, (0, campfire_y, campfire_z), campfire_state_id).await;

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:porkchop 1 0".into(),
        })
        .await
        .expect("give porkchop");
    wait_for_slot_stack(&mut client, porkchop_id, 1).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(0, campfire_y, campfire_z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 102,
        })
        .await
        .expect("start campfire cooking");
    let campfire_pos = pack_block_pos(0, campfire_y, campfire_z);
    wait_for_campfire_input_visual_and_slot(&mut client, campfire_pos, &porkchop_name).await;

    let mut item_entity_id = None;
    let mut saw_cooked_stack = false;
    let mut saw_empty_visual = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(saw_cooked_stack && saw_empty_visual) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("campfire cooked output");
        if handle_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode campfire ack");
            assert_eq!(pkt.sequence, 102);
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode campfire item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entity_id = Some(pkt.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body)
                .expect("decode campfire item metadata");
            if Some(pkt.entity_id) == item_entity_id {
                saw_cooked_stack = pkt.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { index, stack }
                            if *index == ITEM_ENTITY_DATA_ITEM_INDEX
                                && stack.item_id == cooked_porkchop_id
                                && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode campfire pickup slot");
            saw_cooked_stack = pkt.item_stack.item_id == cooked_porkchop_id && pkt.item_stack.count >= 1;
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode campfire clear BlockEntityData");
            if pkt.position == campfire_pos {
                saw_empty_visual = campfire_items(&pkt.nbt).is_some_and(|items| items.is_empty());
            }
        }
    }
}

async fn wait_for_campfire_input_visual_and_slot(
    client: &mut Client,
    campfire_pos: i64,
    input_item: &str,
) {
    let mut saw_slot_empty = false;
    let mut saw_input_visual = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_slot_empty && saw_input_visual) {
        let frame = client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
            .expect("campfire input visual");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode campfire input SetSlot");
            saw_slot_empty |= pkt.container_id == 0 && pkt.slot == 36 && pkt.item_stack.is_empty();
        } else if frame.id == ClientboundBlockEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundBlockEntityData::decode(&mut body)
                .expect("decode campfire input BlockEntityData");
            if pkt.position == campfire_pos {
                saw_input_visual = campfire_items(&pkt.nbt)
                    .is_some_and(|items| items.iter().any(|item| campfire_item_matches(item, input_item)));
            }
        }
    }
}

fn campfire_items(nbt: &mc_nbt::Tag) -> Option<&[mc_nbt::Tag]> {
    let mc_nbt::Tag::Compound(fields) = nbt else {
        return None;
    };
    let items = fields
        .iter()
        .find_map(|(name, value)| (name == "Items").then_some(value))?;
    let mc_nbt::Tag::List(list) = items else {
        return None;
    };
    Some(&list.elements)
}

fn campfire_item_matches(item: &mc_nbt::Tag, expected_id: &str) -> bool {
    let mc_nbt::Tag::Compound(fields) = item else {
        return false;
    };
    let field = |name: &str| {
        fields
            .iter()
            .find_map(|(field, value)| (field == name).then_some(value))
    };
    matches!(field("Slot"), Some(mc_nbt::Tag::Int(0)))
        && matches!(field("id"), Some(mc_nbt::Tag::String(id)) if id == expected_id)
        && matches!(field("count"), Some(mc_nbt::Tag::Int(1)))
}
