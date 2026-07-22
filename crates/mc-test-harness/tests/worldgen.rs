//! M7.e — integration test for the baseline worldgen fallback.
//!
//! Boots `mc_net::run` against an empty tempdir (no `.mca` files
//! anywhere) attached to a `TerrainGenerator`. After the spawn
//! burst the test consults the shared world handle directly and
//! asserts a chunk at an arbitrary far position resolves to terrain
//! that contains the expected layers (bedrock at the bottom, a valid
//! biome surface, and water for ocean columns). The test does *not* drain the full spawn
//! burst on the wire to keep its wall-clock reasonable — driving
//! the burst is already covered by the M3.g / M4.f / M5.f / M6.g
//! harnesses.
//!
//! Skipped silently when the vanilla data sidecars are missing.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundCommands, ClientboundContainerSetContent, ClientboundKeepAlive,
    ClientboundOpenScreen, ClientboundSystemChat, ConfirmTeleportation, ContainerInput, Direction,
    GameEvent, HashedStack, InteractionHand, LevelChunkWithLight, MovePlayerFlags,
    ServerboundChatCommand, ServerboundContainerClick, ServerboundKeepAlive,
    ServerboundMovePlayerPosRot, ServerboundMovePlayerStatusOnly, ServerboundUseItemOn,
    SetCenterChunk, SetDefaultSpawnPosition, SynchronizePlayerPosition, pack_block_pos,
};
use mc_test_harness::client::Client;
use mc_world::ChunkGenerator;

#[test]
fn seed_zero_playable_ruin_is_deterministic_and_contains_fixed_loot() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("embedded block registry"),
    );
    let items = mc_data::items::solaris_required_items();
    let generator = mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)).with_structures(
        mc_worldgen::StructureRules::solaris_playable_ruin(&blocks, &items)
            .expect("playable ruin resolves embedded data"),
    );

    let ruin_chunk = mc_world::ChunkPos { x: 4, z: 0 };
    let first = generator.generate(ruin_chunk);
    let second = generator.generate(ruin_chunk);

    for y in mc_world::MIN_Y..mc_world::MAX_Y {
        for x in 0..16 {
            for z in 0..16 {
                assert_eq!(
                    first.get_block(x, y, z),
                    second.get_block(x, y, z),
                    "seed-zero ruin block mismatch at ({x}, {y}, {z})"
                );
            }
        }
    }
    assert_eq!(first.chests, second.chests);

    let (chest_pos, chest) = first
        .chests
        .iter()
        .next()
        .expect("seed-zero ruin chest in chunk (4, 0)");
    assert_eq!(first.chests.len(), 1);
    assert_eq!(chest_pos.x.div_euclid(16), 4);
    assert_eq!(chest_pos.z.div_euclid(16), 0);
    assert_eq!(
        chest.slots[0].item_id,
        items
            .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
            .expect("diamond item"),
    );
    assert_eq!(chest.slots[0].count, 1);
    assert_eq!(
        chest.slots[1].item_id,
        items
            .id_of(&mc_data::Identifier::parse("minecraft:lapis_lazuli").unwrap())
            .expect("lapis item"),
    );
    assert_eq!(chest.slots[1].count, 4);
    assert_eq!(
        chest.slots[2].item_id,
        items
            .id_of(&mc_data::Identifier::parse("minecraft:bread").unwrap())
            .expect("bread item"),
    );
    assert_eq!(chest.slots[2].count, 2);

    let unrelated = generator.generate(mc_world::ChunkPos { x: -4, z: 4 });
    assert!(unrelated.chests.is_empty());
}

#[tokio::test]
async fn fresh_seed_server_spawn_is_dry_with_clear_body_space() {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry"));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &report,
    ));
    let collision_shapes = mc_data::collision_shapes::vanilla_collision_shapes();

    for (index, seed) in [i64::MIN, -1_000_003, 31, 999_983, i64::MAX]
        .into_iter()
        .enumerate()
    {
        let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
            seed,
            Arc::clone(&blocks),
        ));
        let storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 25)
            .with_item_registry(Arc::clone(&items))
            .with_generator(generator as Arc<dyn ChunkGenerator>);
        let world = Arc::new(tokio::sync::Mutex::new(storage));
        let shutdown = mc_net::ShutdownHandle::default();
        let config = mc_net::ServerConfig {
            bind_address: "127.0.0.1:0".parse().expect("loopback address"),
            motd: format!("fresh seed spawn {seed}"),
            max_players: 1,
            view_distance: 0,
            data: Arc::new(mc_data::solaris_required_data()),
            blocks: Arc::clone(&blocks),
            world: Some(Arc::clone(&world)),
            tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
            recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
            loot: Arc::new(mc_data::loot::builtin().clone()),
            block_light: None,
            items: Arc::clone(&items),
            item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
            block_facts: Arc::clone(&block_facts),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
            chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
            random_tick: mc_net::RandomTickPolicy::default(),
            command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
            shutdown: shutdown.clone(),
        };
        let bound = mc_net::bind(config).await.expect("bind fresh seed server");
        let addr = bound.local_addr().expect("fresh seed server address");
        let serve = tokio::spawn(async move { bound.serve().await });
        let (client, sync) = connect_worldgen_client(addr, &format!("SeedSpawn{index}")).await;

        let x = sync.x.floor() as i32;
        let y = sync.y.floor() as i32;
        let z = sync.z.floor() as i32;
        let storage = world.lock().await;
        let support = storage
            .get_cached_block(mc_world::BlockPos { x, y: y - 2, z })
            .unwrap_or_else(|| panic!("seed {seed} spawn support is missing"));
        let support_state = blocks
            .by_id(support)
            .unwrap_or_else(|| panic!("seed {seed} spawn support state is unknown"));
        let support_shape = collision_shapes
            .get_for_state(
                support.0,
                &support_state.block.id,
                &support_state.properties,
            )
            .unwrap_or_else(|| panic!("seed {seed} spawn support shape is missing"));
        assert!(
            block_facts.fluid(support.0).is_none(),
            "seed {seed} selected fluid support at ({x},{},{z})",
            y - 2
        );
        assert!(
            !support_shape.is_empty(),
            "seed {seed} selected passable support {} at ({x},{},{z})",
            support_state.block.id,
            y - 2
        );
        assert!(
            !is_spawn_hazard(support_state.block.id.path()),
            "seed {seed} selected hazardous support {} at ({x},{},{z})",
            support_state.block.id,
            y - 2
        );
        for body_y in (y - 1)..=y + 1 {
            let state = storage
                .get_cached_block(mc_world::BlockPos { x, y: body_y, z })
                .unwrap_or_else(|| panic!("seed {seed} spawn body cell is missing"));
            let block_state = blocks
                .by_id(state)
                .unwrap_or_else(|| panic!("seed {seed} spawn body state is unknown"));
            let shape = collision_shapes
                .get_for_state(state.0, &block_state.block.id, &block_state.properties)
                .unwrap_or_else(|| panic!("seed {seed} spawn body shape is missing"));
            assert!(
                block_facts.fluid(state.0).is_none(),
                "seed {seed} spawned in fluid at ({x},{body_y},{z})"
            );
            assert!(
                shape.is_empty(),
                "seed {seed} spawned inside collidable {} at ({x},{body_y},{z})",
                block_state.block.id
            );
            assert!(
                !is_spawn_hazard(block_state.block.id.path()),
                "seed {seed} spawned inside hazardous {} at ({x},{body_y},{z})",
                block_state.block.id
            );
        }
        drop(storage);
        drop(client);
        shutdown.request();
        tokio::time::timeout(Duration::from_secs(5), serve)
            .await
            .expect("fresh seed server shutdown")
            .expect("fresh seed server joins")
            .expect("fresh seed server exits cleanly");
    }
}

fn is_spawn_hazard(path: &str) -> bool {
    matches!(
        path,
        "cactus"
            | "campfire"
            | "fire"
            | "magma_block"
            | "powder_snow"
            | "soul_campfire"
            | "soul_fire"
            | "sweet_berry_bush"
    )
}

#[tokio::test]
async fn generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart() {
    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry"));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let generator = Arc::new(
        mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)).with_structures(
            mc_worldgen::StructureRules::solaris_playable_ruin(&blocks, &items)
                .expect("playable ruin resolves embedded data"),
        ),
    );
    let ruin_chunk = mc_world::ChunkPos { x: 4, z: 0 };
    let generated = generator.generate(ruin_chunk);
    let chest_pos = generated
        .chests
        .keys()
        .next()
        .copied()
        .expect("seed-zero ruin chest");
    assert_eq!(generated.chests.len(), 1);

    let diamond_id = ruin_item_id(&items, "minecraft:diamond");
    let lapis_id = ruin_item_id(&items, "minecraft:lapis_lazuli");
    let bread_id = ruin_item_id(&items, "minecraft:bread");
    let world_dir = tempfile::tempdir().expect("fresh ruin world");

    let shutdown = mc_net::ShutdownHandle::default();
    let bound = mc_net::bind(ruin_server_config(
        &report,
        Arc::clone(&blocks),
        Arc::clone(&items),
        Arc::clone(&generator),
        world_dir.path(),
        shutdown.clone(),
        "generated ruin chest first run",
    ))
    .await
    .expect("bind first ruin server");
    let addr = bound.local_addr().expect("first ruin server address");
    let serve = tokio::spawn(async move { bound.serve().await });

    let mut client = connect_ruin_client(addr, "RuinLootGate").await;
    move_to_ruin_and_wait_for_resident_chunk(&mut client, chest_pos).await;
    let opened = open_ruin_chest(&mut client, chest_pos, 71).await;
    let mut content = wait_for_container_content(&mut client, opened.container_id, |packet| {
        packet.items.len() >= 27
            && packet.items[0].item_id == diamond_id
            && packet.items[0].count == 1
            && packet.items[1].item_id == lapis_id
            && packet.items[1].count == 4
            && packet.items[2].item_id == bread_id
            && packet.items[2].count == 2
            && packet.items[3..27].iter().all(|stack| stack.is_empty())
    })
    .await;

    content = quick_move_ruin_slot(&mut client, content, 0, diamond_id, 1).await;
    content = quick_move_ruin_slot(&mut client, content, 1, lapis_id, 4).await;
    content = quick_move_ruin_slot(&mut client, content, 2, bread_id, 2).await;
    assert!(
        content.items[..27].iter().all(|stack| stack.is_empty()),
        "all generated chest slots must be empty after quick-moving the loot"
    );
    for (item_id, expected_count) in [(diamond_id, 1), (lapis_id, 4), (bread_id, 2)] {
        assert_eq!(
            container_item_count(&content, item_id),
            expected_count,
            "player inventory must contain the quick-moved ruin loot"
        );
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("request save-all");
    wait_for_save_all_feedback(&mut client).await;
    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("first ruin server shutdown")
        .expect("first ruin server joins")
        .expect("first ruin server exits cleanly");

    let shutdown = mc_net::ShutdownHandle::default();
    let bound = mc_net::bind(ruin_server_config(
        &report,
        Arc::clone(&blocks),
        Arc::clone(&items),
        generator,
        world_dir.path(),
        shutdown.clone(),
        "generated ruin chest restart",
    ))
    .await
    .expect("bind restarted ruin server");
    let addr = bound.local_addr().expect("restarted ruin server address");
    let serve = tokio::spawn(async move { bound.serve().await });

    let mut client = connect_ruin_client(addr, "RuinLootGate").await;
    let restored_inventory = wait_for_container_content(&mut client, 0, |packet| {
        container_item_count(packet, diamond_id) == 1
            && container_item_count(packet, lapis_id) == 4
            && container_item_count(packet, bread_id) == 2
    })
    .await;
    assert_eq!(container_item_count(&restored_inventory, diamond_id), 1);
    assert_eq!(container_item_count(&restored_inventory, lapis_id), 4);
    assert_eq!(container_item_count(&restored_inventory, bread_id), 2);

    move_to_ruin_and_wait_for_resident_chunk(&mut client, chest_pos).await;
    let opened = open_ruin_chest(&mut client, chest_pos, 72).await;
    let empty = wait_for_container_content(&mut client, opened.container_id, |packet| {
        packet.items.len() >= 27 && packet.items[..27].iter().all(|stack| stack.is_empty())
    })
    .await;
    assert!(empty.items[..27].iter().all(|stack| stack.is_empty()));

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("restarted ruin server shutdown")
        .expect("restarted ruin server joins")
        .expect("restarted ruin server exits cleanly");
}

fn ruin_server_config(
    report: &[mc_data::blocks::BlockReport],
    blocks: Arc<mc_world::BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    generator: Arc<mc_worldgen::TerrainGenerator>,
    world_dir: &std::path::Path,
    shutdown: mc_net::ShutdownHandle,
    motd: &str,
) -> mc_net::ServerConfig {
    std::fs::create_dir_all(world_dir.join("region")).expect("create ruin region directory");
    let world = mc_world::WorldStorage::open_with_capacity(world_dir, Arc::clone(&blocks), 49)
        .expect("open ruin disk world")
        .with_item_registry(Arc::clone(&items))
        .with_generator(generator as Arc<dyn ChunkGenerator>);
    mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().expect("loopback address"),
        motd: motd.into(),
        max_players: 8,
        view_distance: 2,
        data: Arc::new(mc_data::solaris_required_data()),
        blocks,
        world: Some(Arc::new(tokio::sync::Mutex::new(world))),
        tags: Arc::new(mc_data::tags::solaris_required_item_tags(&items)),
        recipes: Arc::new(mc_data::recipes::solaris_required_recipes()),
        loot: Arc::new(mc_data::loot::builtin().clone()),
        block_light: None,
        items,
        item_facts: Arc::new(mc_data::item_components::solaris_required_item_facts()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
            report,
        )),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown,
    }
}

async fn connect_ruin_client(addr: std::net::SocketAddr, name: &str) -> Client {
    connect_worldgen_client(addr, name).await.0
}

async fn connect_worldgen_client(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("connect ruin client");
    let _ = client
        .drive_login(addr, name)
        .await
        .expect("login ruin client");
    client
        .drive_configuration()
        .await
        .expect("configure ruin client");
    let _ = client.read_play_login().await.expect("enter Play");
    let _: ClientboundCommands = client.read_typed().await.expect("commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("spawn sync");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("world border");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("time");
    let _: SetDefaultSpawnPosition = client.read_typed().await.expect("spawn position");
    let _: GameEvent = client.read_typed().await.expect("spawn game event");
    let _: SetCenterChunk = client.read_typed().await.expect("spawn center chunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("confirm spawn teleport");
    client
        .write_packet(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("report grounded spawn");
    (client, sync)
}

async fn move_to_ruin_and_wait_for_resident_chunk(
    client: &mut Client,
    chest_pos: mc_world::BlockPos,
) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: f64::from(chest_pos.x) - 0.5,
            y: f64::from(chest_pos.y),
            z: f64::from(chest_pos.z) + 0.5,
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("send authoritative movement beside ruin chest");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("ruin chunk residency");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let correction = SynchronizePlayerPosition::decode(&mut body)
                .expect("decode ruin movement correction");
            panic!("movement beside generated ruin was rejected: {correction:?}");
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let chunk = LevelChunkWithLight::decode(&mut body).expect("decode ruin chunk");
            if (chunk.chunk_x, chunk.chunk_z) == (4, 0) {
                return;
            }
        }
    }
}

async fn open_ruin_chest(
    client: &mut Client,
    chest_pos: mc_world::BlockPos,
    sequence: i32,
) -> ClientboundOpenScreen {
    client
        .write_packet(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: pack_block_pos(chest_pos.x, chest_pos.y, chest_pos.z),
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("open generated ruin chest");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated ruin chest screen");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            let mut body = frame.body;
            let screen = ClientboundOpenScreen::decode(&mut body).expect("decode chest screen");
            if screen.menu_type == 2 {
                return screen;
            }
        }
    }
}

async fn quick_move_ruin_slot(
    client: &mut Client,
    content: ClientboundContainerSetContent,
    slot: i16,
    item_id: u32,
    count: i32,
) -> ClientboundContainerSetContent {
    client
        .write_packet(&ServerboundContainerClick {
            container_id: content.container_id,
            state_id: content.state_id,
            slot_num: slot,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("quick-move generated ruin loot");
    let previous_state_id = content.state_id;
    wait_for_container_content(client, content.container_id, |packet| {
        packet.state_id > previous_state_id
            && packet.items[usize::try_from(slot).expect("non-negative chest slot")].is_empty()
            && container_item_count(packet, item_id) == count
    })
    .await
}

async fn wait_for_container_content(
    client: &mut Client,
    container_id: i32,
    predicate: impl Fn(&ClientboundContainerSetContent) -> bool,
) -> ClientboundContainerSetContent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("container content");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode ContainerSetContent");
            if packet.container_id == container_id && predicate(&packet) {
                return packet;
            }
        }
    }
}

async fn wait_for_save_all_feedback(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("save-all feedback");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundSystemChat::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSystemChat::decode(&mut body).expect("decode save-all feedback");
            if system_chat_text(&packet).starts_with("Saved ") {
                return;
            }
        }
    }
}

async fn handle_keepalive(client: &mut Client, id: i32, body: &bytes::Bytes) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body.clone();
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode keepalive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo keepalive");
    true
}

fn container_item_count(packet: &ClientboundContainerSetContent, item_id: u32) -> i32 {
    packet
        .items
        .iter()
        .filter(|stack| stack.item_id == item_id)
        .map(|stack| stack.count)
        .sum()
}

fn ruin_item_id(items: &mc_data::items::ItemRegistry, id: &str) -> u32 {
    items
        .id_of(&mc_data::Identifier::parse(id).expect("valid ruin item identifier"))
        .unwrap_or_else(|| panic!("canonical item registry contains {id}"))
}

fn system_chat_text(packet: &ClientboundSystemChat) -> String {
    let mut bytes = bytes::Bytes::copy_from_slice(&packet.content_nbt);
    let mc_nbt::Tag::Compound(fields) = mc_nbt::read_network(&mut bytes).expect("read chat NBT")
    else {
        panic!("system chat component root must be a compound");
    };
    fields
        .into_iter()
        .find_map(|(name, tag)| match (name.as_str(), tag) {
            ("text", mc_nbt::Tag::String(text)) => Some(text),
            _ => None,
        })
        .expect("system chat component text")
}

#[tokio::test]
async fn empty_world_plus_generator_produces_terrain_on_demand() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return;
    }

    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry"));

    // Empty world: just create the expected dir layout.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();

    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        12345,
        Arc::clone(&blocks),
    ));
    let storage = mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_generator(generator.clone() as Arc<dyn ChunkGenerator>);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));

    // Resolve the layer ids so the assertions don't pin numerics.
    let bedrock_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:bedrock").unwrap())
        .map(|b| b.default)
        .unwrap();
    let grass_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:grass_block").unwrap())
        .map(|b| b.default)
        .unwrap();
    let sand_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:sand").unwrap())
        .map(|b| b.default)
        .unwrap();
    let water_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:water").unwrap())
        .map(|b| b.default)
        .unwrap();

    // Pull a far chunk through the storage; the generator runs
    // because no region file exists. Assert it has the expected
    // five-layer shape.
    {
        let mut storage = world_handle.lock().await;
        let cpos = mc_world::ChunkPos { x: 50, z: -50 };
        let chunk = storage
            .get_chunk(cpos)
            .expect("storage get_chunk OK")
            .expect("generator produced a chunk");

        // Bedrock at the bottom.
        assert_eq!(
            chunk.get_block(0, mc_world::MIN_Y, 0),
            Some(bedrock_state_id)
        );

        // Find the surface column and assert the biome-aware top.
        let height = generator.surface_height(50 * 16, -50 * 16);
        let surface = chunk.get_block(0, height, 0);
        assert_eq!(
            surface,
            Some(if height < mc_worldgen::terrain::SEA_LEVEL {
                sand_state_id
            } else {
                grass_state_id
            }),
            "generator output: column (0,{height},0) of chunk (50,-50) has unexpected surface"
        );
        if height < mc_worldgen::terrain::SEA_LEVEL {
            assert_eq!(
                chunk.get_block(0, mc_worldgen::terrain::SEA_LEVEL, 0),
                Some(water_state_id),
                "ocean column should be water-filled to sea level"
            );
        }
        // Chunk is dirty so the M6 flush will persist it.
        assert!(chunk.dirty);
    }

    // Flush + reopen: the generated chunk now lives on disk and the
    // generator doesn't run a second time.
    {
        let mut storage = world_handle.lock().await;
        let n = storage.flush_dirty().expect("flush_dirty");
        assert!(
            n >= 1,
            "at least one generated chunk should have been flushed"
        );
    }
    drop(world_handle);

    // Fresh open with no generator: chunks already written must
    // still be readable. Pick a far chunk we just generated.
    let mut fresh = mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks)).unwrap();
    let cpos = mc_world::ChunkPos { x: 50, z: -50 };
    let chunk = fresh
        .get_chunk(cpos)
        .expect("storage get_chunk OK")
        .expect("region holds generated chunk after flush");
    assert_eq!(
        chunk.get_block(0, mc_world::MIN_Y, 0),
        Some(bedrock_state_id)
    );
}
