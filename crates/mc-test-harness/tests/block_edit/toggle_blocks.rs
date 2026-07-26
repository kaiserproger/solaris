#[tokio::test]
async fn survival_hand_use_toggles_wood_and_copper_but_not_iron_doors_and_trapdoors() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping toggle-block TCP test; missing {} or {}",
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
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M100 toggle block hand-use parity".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
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

    let (mut client, sync) = connect_to_play(addr, "M100ToggleBlocks").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    client
        .write_packet(&ServerboundPlayerInput {
            input: PlayerInput::default(),
        })
        .await
        .expect("send unshifted player input");

    let base_y = sync.y.floor() as i32 - 1;
    let oak_door_lower = mc_world::BlockPos {
        x: 1,
        y: base_y,
        z: 0,
    };
    let iron_door_lower = mc_world::BlockPos {
        x: -1,
        y: base_y,
        z: 0,
    };
    let copper_door_lower = mc_world::BlockPos {
        x: 0,
        y: base_y,
        z: 1,
    };
    let oak_trapdoor_pos = mc_world::BlockPos {
        x: 1,
        y: base_y,
        z: 1,
    };
    let iron_trapdoor_pos = mc_world::BlockPos {
        x: -1,
        y: base_y,
        z: 1,
    };
    let copper_trapdoor_pos = mc_world::BlockPos {
        x: 0,
        y: base_y,
        z: 2,
    };

    let oak_door_closed = door_state(&blocks, "minecraft:oak_door", "lower", false);
    let oak_door_upper_closed = door_state(&blocks, "minecraft:oak_door", "upper", false);
    let oak_door_open = door_state(&blocks, "minecraft:oak_door", "lower", true);
    let oak_door_upper_open = door_state(&blocks, "minecraft:oak_door", "upper", true);
    let iron_door_closed = door_state(&blocks, "minecraft:iron_door", "lower", false);
    let iron_door_upper_closed = door_state(&blocks, "minecraft:iron_door", "upper", false);
    let iron_door_open = door_state(&blocks, "minecraft:iron_door", "lower", true);
    let iron_door_upper_open = door_state(&blocks, "minecraft:iron_door", "upper", true);
    let copper_door_closed = door_state(&blocks, "minecraft:copper_door", "lower", false);
    let copper_door_upper_closed = door_state(&blocks, "minecraft:copper_door", "upper", false);
    let copper_door_open = door_state(&blocks, "minecraft:copper_door", "lower", true);
    let copper_door_upper_open = door_state(&blocks, "minecraft:copper_door", "upper", true);
    let oak_trapdoor_closed = trapdoor_state(&blocks, "minecraft:oak_trapdoor", false);
    let oak_trapdoor_open = trapdoor_state(&blocks, "minecraft:oak_trapdoor", true);
    let iron_trapdoor_closed = trapdoor_state(&blocks, "minecraft:iron_trapdoor", false);
    let iron_trapdoor_open = trapdoor_state(&blocks, "minecraft:iron_trapdoor", true);
    let copper_trapdoor_closed = trapdoor_state(&blocks, "minecraft:copper_trapdoor", false);
    let copper_trapdoor_open = trapdoor_state(&blocks, "minecraft:copper_trapdoor", true);

    {
        let mut world = world_handle.lock().await;
        seed_door(
            &mut world,
            oak_door_lower,
            oak_door_closed,
            oak_door_upper_closed,
        );
        seed_door(
            &mut world,
            iron_door_lower,
            iron_door_closed,
            iron_door_upper_closed,
        );
        seed_door(
            &mut world,
            copper_door_lower,
            copper_door_closed,
            copper_door_upper_closed,
        );
        seed_block(&mut world, oak_trapdoor_pos, oak_trapdoor_closed);
        seed_block(&mut world, iron_trapdoor_pos, iron_trapdoor_closed);
        seed_block(&mut world, copper_trapdoor_pos, copper_trapdoor_closed);
        assert_eq!(
            world
                .get_block(oak_door_lower)
                .expect("read seeded oak door"),
            Some(oak_door_closed)
        );
    }

    use_block(&mut client, oak_door_lower, 801).await;
    read_toggle_response(
        &mut client,
        801,
        &[
            (oak_door_lower, oak_door_open.0 as i32),
            (
                mc_world::BlockPos {
                    y: base_y + 1,
                    ..oak_door_lower
                },
                oak_door_upper_open.0 as i32,
            ),
        ],
        &[],
    )
    .await;

    use_block(&mut client, iron_door_lower, 802).await;
    read_toggle_response(
        &mut client,
        802,
        &[],
        &[
            (iron_door_lower, iron_door_open.0 as i32),
            (
                mc_world::BlockPos {
                    y: base_y + 1,
                    ..iron_door_lower
                },
                iron_door_upper_open.0 as i32,
            ),
        ],
    )
    .await;

    use_block(&mut client, copper_door_lower, 803).await;
    read_toggle_response(
        &mut client,
        803,
        &[
            (copper_door_lower, copper_door_open.0 as i32),
            (
                mc_world::BlockPos {
                    y: base_y + 1,
                    ..copper_door_lower
                },
                copper_door_upper_open.0 as i32,
            ),
        ],
        &[],
    )
    .await;

    use_block(&mut client, oak_trapdoor_pos, 804).await;
    read_toggle_response(
        &mut client,
        804,
        &[(oak_trapdoor_pos, oak_trapdoor_open.0 as i32)],
        &[],
    )
    .await;

    use_block(&mut client, iron_trapdoor_pos, 805).await;
    read_toggle_response(
        &mut client,
        805,
        &[],
        &[(iron_trapdoor_pos, iron_trapdoor_open.0 as i32)],
    )
    .await;

    use_block(&mut client, copper_trapdoor_pos, 806).await;
    read_toggle_response(
        &mut client,
        806,
        &[(copper_trapdoor_pos, copper_trapdoor_open.0 as i32)],
        &[],
    )
    .await;
}

fn door_state(
    blocks: &mc_world::BlockRegistry,
    id: &str,
    half: &str,
    open: bool,
) -> mc_world::BlockStateId {
    block_state(
        blocks,
        id,
        &[
            ("facing", "north"),
            ("half", half),
            ("hinge", "left"),
            ("open", if open { "true" } else { "false" }),
            ("powered", "false"),
        ],
    )
}

fn trapdoor_state(
    blocks: &mc_world::BlockRegistry,
    id: &str,
    open: bool,
) -> mc_world::BlockStateId {
    block_state(
        blocks,
        id,
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("open", if open { "true" } else { "false" }),
            ("powered", "false"),
            ("waterlogged", "false"),
        ],
    )
}

fn block_state(
    blocks: &mc_world::BlockRegistry,
    id: &str,
    props: &[(&str, &str)],
) -> mc_world::BlockStateId {
    let id = mc_data::Identifier::parse(id).expect("static block identifier");
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    blocks
        .by_name_and_props(&id, &props)
        .unwrap_or_else(|| panic!("missing block state {id} {props:?}"))
}

fn seed_door(
    world: &mut mc_world::WorldStorage,
    lower: mc_world::BlockPos,
    lower_state: mc_world::BlockStateId,
    upper_state: mc_world::BlockStateId,
) {
    seed_block(world, lower, lower_state);
    seed_block(
        world,
        mc_world::BlockPos {
            y: lower.y + 1,
            ..lower
        },
        upper_state,
    );
}

fn seed_block(
    world: &mut mc_world::WorldStorage,
    pos: mc_world::BlockPos,
    state: mc_world::BlockStateId,
) {
    world
        .set_block_at(pos, state)
        .expect("seed toggle block")
        .expect("toggle test chunk exists");
}

async fn use_block(client: &mut Client, pos: mc_world::BlockPos, sequence: i32) {
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(pos.x, pos.y, pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("send toggle UseItemOn");
}

async fn read_toggle_response(
    client: &mut Client,
    sequence: i32,
    expected_updates: &[(mc_world::BlockPos, i32)],
    forbidden_updates: &[(mc_world::BlockPos, i32)],
) {
    let mut seen = vec![false; expected_updates.len()];
    let mut observed_updates = Vec::new();
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_ack && seen.iter().all(|seen| *seen)) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("toggle response");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode toggle BlockUpdate");
            observe_toggle_update(
                &mut seen,
                &mut observed_updates,
                expected_updates,
                forbidden_updates,
                unpack_block_pos(pkt.position),
                pkt.state_id,
            );
        } else if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt =
                SectionBlocksUpdate::decode(&mut body).expect("decode toggle SectionBlocksUpdate");
            for (index, (expected_pos, _)) in expected_updates.iter().enumerate() {
                let section = section_pos_for_block(*expected_pos);
                let relative =
                    pack_section_relative_pos(expected_pos.x, expected_pos.y, expected_pos.z);
                if pkt.section_pos == section
                    && let Some(change) = pkt.changes.iter().find(|change| {
                        change.relative_pos == relative
                            && change.state_id == expected_updates[index].1
                    })
                {
                    observe_toggle_update(
                        &mut seen,
                        &mut observed_updates,
                        expected_updates,
                        forbidden_updates,
                        (expected_pos.x, expected_pos.y, expected_pos.z),
                        change.state_id,
                    );
                }
            }
            for (forbidden_pos, forbidden_state) in forbidden_updates {
                if pkt.section_pos != section_pos_for_block(*forbidden_pos) {
                    continue;
                }
                let relative =
                    pack_section_relative_pos(forbidden_pos.x, forbidden_pos.y, forbidden_pos.z);
                assert!(
                    !pkt.changes.iter().any(|change| {
                        change.relative_pos == relative && change.state_id == *forbidden_state
                    }),
                    "hand use must not emit forbidden section toggle update at {forbidden_pos:?}"
                );
            }
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode toggle ack");
            if pkt.sequence == sequence {
                assert!(
                    seen.iter().all(|seen| *seen),
                    "toggle ack {sequence} arrived before expected updates; seen={seen:?}; observed_updates={observed_updates:?}"
                );
                saw_ack = true;
            }
        }
    }
}

fn observe_toggle_update(
    seen: &mut [bool],
    observed_updates: &mut Vec<(mc_world::BlockPos, i32)>,
    expected_updates: &[(mc_world::BlockPos, i32)],
    forbidden_updates: &[(mc_world::BlockPos, i32)],
    pos_tuple: (i32, i32, i32),
    state_id: i32,
) {
    let pos = mc_world::BlockPos {
        x: pos_tuple.0,
        y: pos_tuple.1,
        z: pos_tuple.2,
    };
    observed_updates.push((pos, state_id));
    assert!(
        !forbidden_updates
            .iter()
            .any(|(blocked_pos, blocked_state)| *blocked_pos == pos && *blocked_state == state_id),
        "hand use must not emit forbidden toggle update at {pos:?}"
    );
    for (index, (expected_pos, expected_state)) in expected_updates.iter().enumerate() {
        if *expected_pos == pos && *expected_state == state_id {
            seen[index] = true;
        }
    }
}

fn section_pos_for_block(pos: mc_world::BlockPos) -> i64 {
    pack_section_pos(
        pos.x.div_euclid(16),
        pos.y.div_euclid(16),
        pos.z.div_euclid(16),
    )
}
