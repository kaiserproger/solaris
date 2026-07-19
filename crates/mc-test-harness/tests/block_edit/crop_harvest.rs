struct CropHarvestCase {
    name: &'static str,
    block: &'static str,
    age: &'static str,
    support: &'static str,
    drops: &'static [(&'static str, i32)],
}

struct ExpectedCropDrop {
    item_id: u32,
    count: i32,
    seen: bool,
}

#[tokio::test]
async fn survival_break_mature_common_crops_drops_deterministic_items() {
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
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

    const CASES: &[CropHarvestCase] = &[
        CropHarvestCase {
            name: "carrot",
            block: "minecraft:carrots",
            age: "7",
            support: "minecraft:farmland",
            drops: &[("minecraft:carrot", 2)],
        },
        CropHarvestCase {
            name: "potato",
            block: "minecraft:potatoes",
            age: "7",
            support: "minecraft:farmland",
            drops: &[("minecraft:potato", 2)],
        },
        CropHarvestCase {
            name: "beetroot",
            block: "minecraft:beetroots",
            age: "3",
            support: "minecraft:farmland",
            drops: &[("minecraft:beetroot", 1), ("minecraft:beetroot_seeds", 1)],
        },
        CropHarvestCase {
            name: "nether_wart",
            block: "minecraft:nether_wart",
            age: "3",
            support: "minecraft:soul_sand",
            drops: &[("minecraft:nether_wart", 2)],
        },
    ];

    let air_state_id = crop_test_state(&blocks, "minecraft:air", &[]).0 as i32;
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .expect("item entity type") as i32;

    for (idx, case) in CASES.iter().enumerate() {
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
        let storage = mc_world::WorldStorage::in_memory_with_capacity(
            Arc::clone(&blocks),
            ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
        )
        .with_generator(generator);
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let cfg = mc_net::ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: format!("M72 {} harvest drops", case.name),
            max_players: 8,
            view_distance: VIEW_DISTANCE,
            data: Arc::clone(&data),
            blocks: Arc::clone(&blocks),
            world: Some(Arc::clone(&world)),
            tags: Arc::clone(&tags),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::clone(&items),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::clone(&entity_types),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
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

        let username = format!("M72Crop{idx}");
        let (mut client, sync) = connect_to_play(addr, &username).await;
        drain_until_chunk(&mut client, (0, 0)).await;

        let support_y = sync.y.floor() as i32 - 2;
        let crop_pos = (0, support_y + 1, 2);
        {
            let mut storage = world.lock().await;
            let support = crop_test_state(&blocks, case.support, &[]);
            let crop = crop_test_state(&blocks, case.block, &[("age", case.age)]);
            crop_test_set(&mut storage, (0, support_y, 2), support);
            crop_test_set(&mut storage, crop_pos, crop);
        }

        let sequence = 720 + idx as i32 * 2;
        let mut drops = expected_crop_drops(&items, case.drops);
        break_crop_and_wait_for_drops(
            &mut client,
            crop_pos,
            air_state_id,
            item_entity_type,
            sequence,
            None,
            case.name,
            &mut drops,
        )
        .await;
    }
}

fn expected_crop_drops(
    items: &mc_data::items::ItemRegistry,
    drops: &[(&str, i32)],
) -> Vec<ExpectedCropDrop> {
    drops
        .iter()
        .map(|(name, count)| {
            let id = mc_data::Identifier::parse(*name).expect("static item identifier");
            let item_id = items
                .id_of(&id)
                .unwrap_or_else(|| panic!("missing item {name}"));
            ExpectedCropDrop {
                item_id,
                count: *count,
                seen: false,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn break_crop_and_wait_for_drops(
    client: &mut Client,
    crop_pos: (i32, i32, i32),
    air_state_id: i32,
    item_entity_type: i32,
    start_sequence: i32,
    stop_after_ticks: Option<i64>,
    case_name: &str,
    expected: &mut [ExpectedCropDrop],
) {
    let target_pos = pack_block_pos(crop_pos.0, crop_pos.1, crop_pos.2);
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence: start_sequence,
        })
        .await
        .unwrap_or_else(|err| panic!("send {case_name} start break: {err}"));
    let completion_sequence = if let Some(ticks) = stop_after_ticks {
        read_ack_without_target_update(client, start_sequence, crop_pos).await;
        wait_for_world_ticks(client, ticks).await;
        let stop_sequence = start_sequence + 1;
        client
            .write_packet(&ServerboundPlayerAction {
                action: PlayerActionKind::StopDestroyBlock,
                position: target_pos,
                direction: Direction::Up,
                sequence: stop_sequence,
            })
            .await
            .unwrap_or_else(|err| panic!("send {case_name} stop break: {err}"));
        stop_sequence
    } else {
        start_sequence
    };

    wait_for_crop_harvest_drops(
        client,
        crop_pos,
        air_state_id,
        item_entity_type,
        completion_sequence,
        case_name,
        expected,
    )
    .await;
}

async fn wait_for_crop_harvest_drops(
    client: &mut Client,
    crop_pos: (i32, i32, i32),
    air_state_id: i32,
    item_entity_type: i32,
    sequence: i32,
    case_name: &str,
    expected: &mut [ExpectedCropDrop],
) {
    let mut item_entities = HashSet::new();
    let mut saw_block_break = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_break && saw_ack && expected.iter().all(|drop| drop.seen)) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| {
                let drops = expected
                    .iter()
                    .map(|drop| format!("{}:{} seen={}", drop.item_id, drop.count, drop.seen))
                    .collect::<Vec<_>>()
                    .join(", ");
                panic!(
                    "{case_name} harvest drop response: {err}; block={saw_block_break} ack={saw_ack} drops=[{drops}]"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode crop BlockUpdate");
            if unpack_block_pos(pkt.position) == crop_pos {
                assert_eq!(pkt.state_id, air_state_id, "{case_name} broke to air");
                saw_block_break = true;
            }
            continue;
        }
        if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode crop break ack");
            if pkt.sequence == sequence {
                saw_ack = true;
            }
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode crop item AddEntity");
            if pkt.entity_type_id == item_entity_type {
                item_entities.insert(pkt.entity_id);
            }
            continue;
        }
        if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetEntityData::decode(&mut body).expect("decode crop item data");
            if !item_entities.contains(&pkt.entity_id) {
                continue;
            }
            for value in pkt.values {
                mark_expected_crop_drop_from_entity(value, expected);
            }
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode crop pickup SetSlot");
            mark_expected_crop_pickup(expected, pkt.item_stack.item_id, pkt.item_stack.count);
        }
    }
}

fn mark_expected_crop_drop_from_entity(
    value: EntityDataValue,
    expected: &mut [ExpectedCropDrop],
) {
    let EntityDataValue::ItemStack { index, stack } = value else {
        return;
    };
    if index == ITEM_ENTITY_DATA_ITEM_INDEX {
        mark_expected_crop_drop(expected, stack.item_id, stack.count);
    }
}

fn mark_expected_crop_drop(expected: &mut [ExpectedCropDrop], item_id: u32, count: i32) {
    if let Some(drop) = expected
        .iter_mut()
        .find(|drop| !drop.seen && drop.item_id == item_id && drop.count == count)
    {
        drop.seen = true;
    }
}

fn mark_expected_crop_pickup(expected: &mut [ExpectedCropDrop], item_id: u32, count: i32) {
    if let Some(drop) = expected
        .iter_mut()
        .find(|drop| !drop.seen && drop.item_id == item_id && count >= drop.count)
    {
        drop.seen = true;
    }
}
