#[tokio::test]
async fn survival_bonemeal_grows_oak_sapling_into_tree() {
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
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));

    let oak_sapling = sapling_test_state(&blocks, "minecraft:oak_sapling", &[]);
    let oak_log = sapling_test_state(&blocks, "minecraft:oak_log", &[("axis", "y")]);
    let oak_leaves = sapling_test_state(
        &blocks,
        "minecraft:oak_leaves",
        &[
            ("distance", "1"),
            ("persistent", "false"),
            ("waterlogged", "false"),
        ],
    );
    let bone_meal = mc_data::Identifier::parse("minecraft:bone_meal").unwrap();
    let bone_meal_item_id = items.id_of(&bone_meal).expect("bone meal item");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M60 sapling growth".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::clone(&world)),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
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

    let (mut client, sync) = connect_to_play(addr, "M60SaplingGrowth").await;
    drain_until_chunk(&mut client, (0, 0)).await;

    let support_y = sync.y.floor() as i32 - 2;
    let sapling_pos = (0, support_y + 1, 2);
    let leaf_pos = (0, support_y + 5, 2);
    {
        let mut storage = world.lock().await;
        sapling_test_set(&mut storage, sapling_pos, oak_sapling);
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:bone_meal 2 0".into(),
        })
        .await
        .expect("give bone meal");
    wait_for_slot_stack(&mut client, bone_meal_item_id, 2).await;

    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(sapling_pos.0, sapling_pos.1, sapling_pos.2),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence: 601,
        })
        .await
        .expect("bonemeal oak sapling");

    wait_for_sapling_tree_growth(
        &mut client,
        sapling_pos,
        oak_log.0 as i32,
        leaf_pos,
        oak_leaves.0 as i32,
        bone_meal_item_id,
        601,
    )
    .await;
}

fn sapling_test_state(
    blocks: &mc_world::BlockRegistry,
    name: &str,
    props: &[(&str, &str)],
) -> mc_world::BlockStateId {
    let id = mc_data::Identifier::parse(name).expect("static identifier");
    if props.is_empty() {
        return blocks
            .block(&id)
            .unwrap_or_else(|| panic!("missing block {name}"))
            .default;
    }
    let props = props
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .unwrap_or_else(|| panic!("missing block state {name} {props:?}"))
}

fn sapling_test_set(
    storage: &mut mc_world::WorldStorage,
    pos: (i32, i32, i32),
    state: mc_world::BlockStateId,
) {
    storage
        .set_block_at(
            mc_world::BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            state,
        )
        .expect("sapling fixture block edit succeeds");
}

async fn wait_for_sapling_tree_growth(
    client: &mut Client,
    log_pos: (i32, i32, i32),
    log_state_id: i32,
    leaf_pos: (i32, i32, i32),
    leaf_state_id: i32,
    bone_meal_item_id: u32,
    sequence: i32,
) {
    let mut saw_log = false;
    let mut saw_leaf = false;
    let mut saw_bonemeal_decrement = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_log && saw_leaf && saw_bonemeal_decrement && saw_ack) {
        let frame = match client
            .read_frame_with_timeout(deadline.saturating_duration_since(tokio::time::Instant::now()))
            .await
        {
            Ok(frame) => frame,
            Err(err) => panic!(
                "sapling tree growth response timed out: log={saw_log} leaf={saw_leaf} \
                 bonemeal={saw_bonemeal_decrement} ack={saw_ack}: {err}"
            ),
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode sapling BlockUpdate");
            let pos = unpack_block_pos(pkt.position);
            if pos == log_pos {
                assert_eq!(pkt.state_id, log_state_id);
                saw_log = true;
            } else if pos == leaf_pos {
                assert_eq!(pkt.state_id, leaf_state_id);
                saw_leaf = true;
            }
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt =
                SectionBlocksUpdate::decode(&mut body).expect("decode sapling SectionBlocksUpdate");
            if sapling_section_pos_matches(pkt.section_pos, log_pos) {
                let relative = pack_section_relative_pos(log_pos.0, log_pos.1, log_pos.2);
                for change in &pkt.changes {
                    if change.relative_pos == relative {
                        assert_eq!(change.state_id, log_state_id);
                        saw_log = true;
                    }
                }
            }
            if sapling_section_pos_matches(pkt.section_pos, leaf_pos) {
                let relative = pack_section_relative_pos(leaf_pos.0, leaf_pos.1, leaf_pos.2);
                for change in &pkt.changes {
                    if change.relative_pos == relative {
                        assert_eq!(change.state_id, leaf_state_id);
                        saw_leaf = true;
                    }
                }
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode sapling bonemeal SetSlot");
            saw_bonemeal_decrement |=
                pkt.item_stack.item_id == bone_meal_item_id && pkt.item_stack.count == 1;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode sapling ack");
            saw_ack |= pkt.sequence == sequence;
        }
    }
}

fn sapling_section_pos_matches(section_pos: i64, target: (i32, i32, i32)) -> bool {
    let sx = sapling_unpack_signed_section_coord(section_pos >> 42, 22);
    let sy = sapling_unpack_signed_section_coord(section_pos, 20);
    let sz = sapling_unpack_signed_section_coord(section_pos >> 20, 22);
    sx == target.0.div_euclid(16) && sy == target.1.div_euclid(16) && sz == target.2.div_euclid(16)
}

fn sapling_unpack_signed_section_coord(value: i64, bits: u8) -> i32 {
    let mask = (1_i64 << bits) - 1;
    let sign = 1_i64 << (bits - 1);
    let value = value & mask;
    let signed = if value & sign == 0 {
        value
    } else {
        value - (1_i64 << bits)
    };
    signed as i32
}
