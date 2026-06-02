#[tokio::test]
async fn survival_random_tick_grows_visible_vertical_plant_columns() {
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

    let sand = crop_test_state(&blocks, "minecraft:sand", &[]);
    let water = crop_test_state(&blocks, "minecraft:water", &[]);
    let cases = [
        (
            "sugar_cane",
            crop_test_state(&blocks, "minecraft:sugar_cane", &[]),
            true,
            false,
        ),
        (
            "cactus",
            crop_test_state(&blocks, "minecraft:cactus", &[]),
            false,
            true,
        ),
        (
            "bamboo",
            crop_test_state(&blocks, "minecraft:bamboo", &[]),
            false,
            false,
        ),
    ];
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &report,
    ));

    for (name, plant, needs_water, needs_spacing) in cases {
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
        let storage = mc_world::WorldStorage::in_memory_with_capacity(
            Arc::clone(&blocks),
            ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
        )
        .with_generator(generator);
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let cfg = mc_net::ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: format!("M76 {name} growth"),
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
            block_facts: Arc::clone(&block_facts),
            entity_types: Arc::clone(&entity_types),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
            random_tick: mc_net::RandomTickPolicy {
                random_tick_speed: 512,
                chunk_budget: ((2 * VIEW_DISTANCE + 1) as usize).pow(2),
                ..mc_net::RandomTickPolicy::default()
            },
            command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
            shutdown: mc_net::ShutdownHandle::default(),
        };
        let bound = mc_net::bind(cfg).await.expect("bind");
        let addr = bound.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let _ = bound.serve().await;
        });

        let username = format!("M76{name}");
        let (mut client, sync) = connect_to_play(addr, &username).await;
        drain_until_chunk(&mut client, (0, 0)).await;

        let support_y = sync.y.floor() as i32 - 2;
        let plant_y = support_y + 1;
        let growth_y = support_y + 2;
        {
            let mut storage = world.lock().await;
            let x_step = if needs_spacing { 2 } else { 1 };
            let z_step = if needs_water || needs_spacing { 2 } else { 1 };
            for x in (0..16).step_by(x_step) {
                for z in (0..16).step_by(z_step) {
                    crop_test_set(&mut storage, (x, support_y, z), sand);
                    crop_test_set(&mut storage, (x, plant_y, z), plant);
                    if needs_water && z + 1 < 16 {
                        crop_test_set(&mut storage, (x, support_y, z + 1), water);
                    }
                }
            }
        }

        wait_for_any_vertical_growth_delta(&mut client, name, growth_y, plant.0 as i32).await;
    }
}

async fn wait_for_any_vertical_growth_delta(
    client: &mut Client,
    case_name: &str,
    y: i32,
    state_id: i32,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|err| panic!("{case_name} vertical plant growth delta: {err}"));
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode vertical plant BlockUpdate");
            if unpack_block_pos(pkt.position).1 == y && pkt.state_id == state_id {
                return;
            }
            continue;
        }
        if frame.id == SectionBlocksUpdate::ID {
            let mut body = frame.body;
            let pkt = SectionBlocksUpdate::decode(&mut body)
                .expect("decode vertical plant SectionBlocksUpdate");
            if section_update_contains_y_state(&pkt, y, state_id) {
                return;
            }
        }
    }
}

fn section_update_contains_y_state(pkt: &SectionBlocksUpdate, y: i32, state_id: i32) -> bool {
    let section_y = y.div_euclid(16);
    let relative_y = (y as u16) & 15;
    let section_y_mask = pkt.section_pos & 0xF_FFFF;
    if section_y_mask != (i64::from(section_y) & 0xF_FFFF) {
        return false;
    }
    pkt.changes.iter().any(|change| {
        let change_y = change.relative_pos & 15;
        change_y == relative_y && change.state_id == state_id
    })
}
