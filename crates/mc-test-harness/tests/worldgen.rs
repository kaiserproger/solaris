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
    AGEABLE_ENTITY_DATA_BABY_INDEX, AddEntity, ClientboundCommands, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundMerchantOffers,
    ClientboundOpenScreen, ClientboundSetEntityData, ClientboundSystemChat, ConfirmTeleportation,
    ContainerInput, Direction, EntityDataValue, EntityEvent, EntityVec3, GameEvent, HashedStack,
    InteractionHand, LevelChunkWithLight, MovePlayerFlags, PlayerActionKind, ServerboundAttack,
    ServerboundChatCommand, ServerboundContainerClick, ServerboundContainerClose,
    ServerboundInteract, ServerboundKeepAlive, ServerboundMovePlayerPosRot,
    ServerboundMovePlayerStatusOnly, ServerboundPlayerAction, ServerboundSelectTrade,
    ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk, SetDefaultSpawnPosition,
    SynchronizePlayerPosition, pack_block_pos,
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

#[test]
fn fresh_seed_server_spawn_is_dry_with_clear_body_space() {
    let test = std::thread::Builder::new()
        .name("fresh_seed_server_spawn_is_dry_with_clear_body_space".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build worldgen integration runtime")
                .block_on(fresh_seed_server_spawn_is_dry_with_clear_body_space_inner());
        })
        .expect("spawn worldgen integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn fresh_seed_server_spawn_is_dry_with_clear_body_space_inner() {
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
        let generator = Arc::new(mc_worldgen::TerrainGenerator::with_worldgen_mode(
            seed,
            Arc::clone(&blocks),
            mc_worldgen::WorldgenMode::TellusLike(mc_worldgen::TellusWorldgenSettings::default()),
        ));
        let located_spawn = generator
            .locate_safe_spawn()
            .unwrap_or_else(|| panic!("seed {seed} has no bounded natural spawn"));
        let spawn = mc_world::WorldSpawn::new(located_spawn.block_x, located_spawn.block_z);
        let storage = mc_world::WorldStorage::in_memory_with_capacity(Arc::clone(&blocks), 25)
            .with_item_registry(Arc::clone(&items))
            .with_spawn(spawn)
            .with_generator(Arc::clone(&generator) as Arc<dyn ChunkGenerator>);
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
            loader_manifest: None,
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

#[test]
fn generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart() {
    let test = std::thread::Builder::new()
        .name("generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build worldgen integration runtime")
                .block_on(generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart_inner());
        })
        .expect("spawn worldgen integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn generated_playable_ruin_chest_loot_moves_to_inventory_and_survives_restart_inner() {
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

fn ruin_server_config<G>(
    report: &[mc_data::blocks::BlockReport],
    blocks: Arc<mc_world::BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    generator: Arc<G>,
    world_dir: &std::path::Path,
    shutdown: mc_net::ShutdownHandle,
    motd: &str,
) -> mc_net::ServerConfig
where
    G: ChunkGenerator + 'static,
{
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
        loader_manifest: None,
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

#[test]
fn generated_village_villager_wire_and_restart_are_stable() {
    let test = std::thread::Builder::new()
        .name("generated_village_villager_wire_and_restart_are_stable".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build generated village integration runtime")
                .block_on(generated_village_villager_wire_and_restart_are_stable_inner());
        })
        .expect("spawn generated village integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn generated_village_villager_wire_and_restart_are_stable_inner() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let required = [
        vanilla_dir
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt"),
        vanilla_dir.join("data/minecraft/structure/village/plains/houses/plains_small_house_1.nbt"),
        vanilla_dir.join("data/minecraft/structure/village/plains/houses/plains_tool_smith_1.nbt"),
    ];
    if required.iter().any(|path| !path.is_file())
        || !vanilla_dir.join("data/minecraft/worldgen").is_dir()
    {
        eprintln!("skipping generated village gate: vanilla structure sidecars are missing");
        return;
    }

    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry"));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let parts = [
        mc_worldgen::PlainsVillagePrototypePart::Fountain,
        mc_worldgen::PlainsVillagePrototypePart::SmallHouse,
        mc_worldgen::PlainsVillagePrototypePart::Toolsmith,
    ];
    let rules = mc_worldgen::StructureRules::plains_village_prototype_with_plan(
        &vanilla_dir,
        &blocks,
        &parts,
        vec![mc_worldgen::StructureInhabitant {
            id: "resident".to_owned(),
            entity_type: "minecraft:villager".to_owned(),
            villager_kind: "plains".to_owned(),
            profession: "toolsmith".to_owned(),
            level: 1,
        }],
    )
    .expect("load fixed village prototype")
    .with_fixed_center((72, 8));
    let generator =
        Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)).with_structures(rules));

    let mut markers = Vec::new();
    let mut vacant_homes = Vec::new();
    for chunk_x in 3..=5 {
        for chunk_z in -1..=1 {
            let chunk = generator.generate(mc_world::ChunkPos {
                x: chunk_x,
                z: chunk_z,
            });
            markers.extend(chunk.settlement_inhabitants());
            vacant_homes.extend(chunk.settlement_vacant_homes());
        }
    }
    assert_eq!(
        markers.len(),
        1,
        "one planned inhabitant must yield one marker"
    );
    assert_eq!(
        vacant_homes.len(),
        1,
        "the first unused villager slot must yield one bounded vacant home"
    );
    assert!(!vacant_homes[0].claim.is_empty());
    assert!(
        vacant_homes[0]
            .position
            .iter()
            .all(|component| component.is_finite())
    );
    let marker = markers.remove(0);
    assert!(marker.home.is_some());
    assert!(marker.job_site.is_some());
    assert!(marker.meeting_point.is_some());

    let villager_type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("villager entity type");
    let world_dir = tempfile::tempdir().expect("generated village disk world");
    let fixture = GeneratedVillageFixture {
        report: &report,
        blocks,
        items,
        generator,
        world_dir: world_dir.path(),
        marker: &marker,
        villager_type_id,
    };

    let first = run_generated_village_observation(&fixture, "VillageGate", true, true).await;
    let second = run_generated_village_observation(&fixture, "VillageGate", false, false).await;

    assert_eq!(
        second.villager.uuid, first.villager.uuid,
        "restart must restore the same villager identity"
    );
    assert_eq!(first.merchant_uses, 1);
    assert_eq!(second.merchant_uses, 1);
    assert_eq!(second.merchant_xp, 2);
}

struct PopulationFixtureGenerator {
    inner: Arc<mc_worldgen::TerrainGenerator>,
}

impl ChunkGenerator for PopulationFixtureGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = self.inner.generate(pos);
        let mut inhabitants = chunk.settlement_inhabitants();
        if inhabitants.len() >= 2 {
            inhabitants.sort_by(|left, right| left.claim.cmp(&right.claim));
            let anchor = inhabitants[0].position;
            inhabitants[0].home = Some(anchor);
            inhabitants[1].position = [anchor[0] + 1.0, anchor[1], anchor[2]];
            inhabitants[1].home = Some(anchor);
            chunk.set_settlement_inhabitants(&inhabitants);
        }
        chunk
    }
}

#[test]
fn generated_village_food_share_birth_wire_and_restart_are_stable() {
    let test = std::thread::Builder::new()
        .name("generated_village_food_share_birth_wire_and_restart_are_stable".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build generated village population runtime")
                .block_on(generated_village_food_share_birth_wire_and_restart_are_stable_inner());
        })
        .expect("spawn generated village population thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn generated_village_food_share_birth_wire_and_restart_are_stable_inner() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let required = [
        vanilla_dir
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt"),
        vanilla_dir.join("data/minecraft/structure/village/plains/houses/plains_small_house_1.nbt"),
    ];
    if required.iter().any(|path| !path.is_file())
        || !vanilla_dir.join("data/minecraft/worldgen").is_dir()
    {
        eprintln!("skipping generated village population gate: vanilla sidecars are missing");
        return;
    }

    let report = mc_data::blocks::solaris_required_blocks_report();
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry"));
    let items = Arc::new(mc_data::items::solaris_required_items());
    let rules = mc_worldgen::StructureRules::plains_village_prototype_with_plan(
        &vanilla_dir,
        &blocks,
        &[
            mc_worldgen::PlainsVillagePrototypePart::Fountain,
            mc_worldgen::PlainsVillagePrototypePart::SmallHouse,
        ],
        vec![
            mc_worldgen::StructureInhabitant {
                id: "population-parent-a".to_owned(),
                entity_type: "minecraft:villager".to_owned(),
                villager_kind: "plains".to_owned(),
                profession: "none".to_owned(),
                level: 1,
            },
            mc_worldgen::StructureInhabitant {
                id: "population-parent-b".to_owned(),
                entity_type: "minecraft:villager".to_owned(),
                villager_kind: "plains".to_owned(),
                profession: "none".to_owned(),
                level: 1,
            },
        ],
    )
    .expect("load fixed village population prototype")
    .with_fixed_center((72, 8));
    let generator = Arc::new(PopulationFixtureGenerator {
        inner: Arc::new(
            mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)).with_structures(rules),
        ),
    });

    let mut markers = Vec::new();
    let mut vacant_homes = Vec::new();
    for chunk_x in 3..=5 {
        for chunk_z in -1..=1 {
            let chunk = generator.generate(mc_world::ChunkPos {
                x: chunk_x,
                z: chunk_z,
            });
            markers.extend(chunk.settlement_inhabitants());
            vacant_homes.extend(chunk.settlement_vacant_homes());
        }
    }
    assert_eq!(markers.len(), 2, "population fixture needs two parents");
    assert_eq!(
        vacant_homes.len(),
        1,
        "population fixture needs one vacant home"
    );
    let parent_distance_squared = marker_distance_squared(&markers[0], &markers[1]);
    assert!(
        parent_distance_squared <= 26.0,
        "fountain parents must start within one ordinary movement step of the exact breeding radius: {parent_distance_squared}; markers={markers:?}; vacant={vacant_homes:?}"
    );
    let midpoint = [
        (markers[0].position[0] + markers[1].position[0]) * 0.5,
        (markers[0].position[1] + markers[1].position[1]) * 0.5,
        (markers[0].position[2] + markers[1].position[2]) * 0.5,
    ];

    let villager_type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("villager entity type");
    let carrot_id = ruin_item_id(&items, "minecraft:carrot");
    let world_dir = tempfile::tempdir().expect("generated village population disk world");

    let shutdown = mc_net::ShutdownHandle::default();
    let bound = mc_net::bind(ruin_server_config(
        &report,
        Arc::clone(&blocks),
        Arc::clone(&items),
        Arc::clone(&generator),
        world_dir.path(),
        shutdown.clone(),
        "generated village population first run",
    ))
    .await
    .expect("bind village population server");
    let addr = bound
        .local_addr()
        .expect("village population server address");
    let serve = tokio::spawn(async move { bound.serve().await });
    let mut client = connect_ruin_client(addr, "VillagePopGate").await;
    let parents = observe_villager_population(&mut client, midpoint, villager_type_id, 2).await;
    let parent_ids = parents
        .values()
        .map(|entity| entity.entity_id)
        .collect::<std::collections::HashSet<_>>();
    let parent_uuids = parents
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let child = drop_carrots_and_wait_for_villager_food_share_birth(
        &mut client,
        &parent_ids,
        villager_type_id,
        carrot_id,
        midpoint,
    )
    .await;
    assert!(!parent_uuids.contains(&child.uuid));

    client
        .write_packet(&ServerboundChatCommand {
            command: "save-all".into(),
        })
        .await
        .expect("save village population world");
    wait_for_save_all_feedback(&mut client).await;
    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("village population first shutdown")
        .expect("village population first server join")
        .expect("village population first server result");

    let shutdown = mc_net::ShutdownHandle::default();
    let bound = mc_net::bind(ruin_server_config(
        &report,
        blocks,
        items,
        generator,
        world_dir.path(),
        shutdown.clone(),
        "generated village population restart",
    ))
    .await
    .expect("bind restarted village population server");
    let addr = bound
        .local_addr()
        .expect("restarted village population server address");
    let serve = tokio::spawn(async move { bound.serve().await });
    let mut client = connect_ruin_client(addr, "VillagePopGate").await;
    let restored = observe_villager_population(&mut client, midpoint, villager_type_id, 3).await;
    assert_eq!(
        restored.len(),
        3,
        "restart must expose exactly two parents and one child"
    );
    for parent_uuid in &parent_uuids {
        assert!(restored.contains_key(parent_uuid));
    }
    assert_eq!(
        restored.keys().filter(|uuid| **uuid == child.uuid).count(),
        1,
        "restart must restore the same child UUID exactly once"
    );

    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("village population restart shutdown")
        .expect("village population restart server join")
        .expect("village population restart server result");
}

fn marker_distance_squared(
    first: &mc_world::SettlementInhabitantMarker,
    second: &mc_world::SettlementInhabitantMarker,
) -> f64 {
    let dx = first.position[0] - second.position[0];
    let dy = first.position[1] - second.position[1];
    let dz = first.position[2] - second.position[2];
    dx.mul_add(dx, dy.mul_add(dy, dz * dz))
}

async fn observe_villager_population(
    client: &mut Client,
    position: [f64; 3],
    villager_type_id: i32,
    expected_count: usize,
) -> std::collections::BTreeMap<uuid::Uuid, AddEntity> {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: position[0],
            y: position[1],
            z: position[2],
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to generated village population fixture");
    let target_chunk = (
        (position[0].floor() as i32).div_euclid(16),
        (position[2].floor() as i32).div_euclid(16),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut quiet_deadline = None;
    let mut chunk_loaded = false;
    let mut villagers = std::collections::BTreeMap::new();
    loop {
        let now = tokio::time::Instant::now();
        let active_deadline = quiet_deadline.unwrap_or(deadline);
        if now >= active_deadline {
            break;
        }
        let frame = match client
            .read_frame_with_timeout(active_deadline.saturating_duration_since(now))
            .await
        {
            Ok(frame) => frame,
            Err(error) if quiet_deadline.is_some() => {
                let _ = error;
                break;
            }
            Err(error) => panic!("generated village population visibility: {error}"),
        };
        let mut frame = frame;
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let correction = SynchronizePlayerPosition::decode(&mut frame.body)
                .expect("decode population movement correction");
            panic!("movement to generated village population was rejected: {correction:?}");
        }
        if frame.id == LevelChunkWithLight::ID {
            let chunk = LevelChunkWithLight::decode(&mut frame.body)
                .expect("decode generated population chunk");
            chunk_loaded |= (chunk.chunk_x, chunk.chunk_z) == target_chunk;
        } else if frame.id == AddEntity::ID {
            let entity =
                AddEntity::decode(&mut frame.body).expect("decode generated population villager");
            if entity.entity_type_id == villager_type_id {
                villagers.insert(entity.uuid, entity);
            }
        }
        if quiet_deadline.is_none() && chunk_loaded && villagers.len() >= expected_count {
            quiet_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(750));
        }
    }
    assert!(chunk_loaded, "generated village population chunk must load");
    assert_eq!(
        villagers.len(),
        expected_count,
        "generated village population must expose exactly {expected_count} unique villager UUIDs"
    );
    villagers
}

async fn drop_carrots_and_wait_for_villager_food_share_birth(
    client: &mut Client,
    parent_ids: &std::collections::HashSet<i32>,
    villager_type_id: i32,
    carrot_id: u32,
    midpoint: [f64; 3],
) -> AddEntity {
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:carrot 48 0".into(),
        })
        .await
        .expect("give carrots for generated villager food sharing");
    let held_slot = wait_for_hotbar_inventory_item(client, carrot_id, 48).await;
    client
        .write_packet(&ServerboundSetCarriedItem { slot: held_slot })
        .await
        .expect("select carrot hotbar slot for generated villager birth");
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::DropAllItems,
            position: 0,
            direction: Direction::Down,
            sequence: 901,
        })
        .await
        .expect("drop carrots for generated villager birth");
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: midpoint[0] + 4.0,
            y: midpoint[1],
            z: midpoint[2],
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move away from dropped villager carrots");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(50);
    let mut heart_parents = std::collections::HashSet::new();
    let mut child: Option<AddEntity> = None;
    let mut non_villager_entities = std::collections::HashSet::new();
    let mut saw_player_drop = false;
    let mut saw_villager_share = false;
    let mut baby_metadata = std::collections::HashSet::new();
    let mut birth_events = std::collections::HashSet::new();
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "generated villager food-share birth visibility: {error}; player_drop={saw_player_drop}; villager_share={saw_villager_share}; non_villagers={non_villager_entities:?}; hearts={heart_parents:?}; child={child:?}; baby={baby_metadata:?}; birth_events={birth_events:?}"
                )
            });
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityEvent::ID {
            let event = EntityEvent::decode(&mut frame.body)
                .expect("decode generated villager population event");
            if event.event_id == 18 && parent_ids.contains(&event.entity_id) {
                heart_parents.insert(event.entity_id);
            }
            if event.event_id == 12 {
                birth_events.insert(event.entity_id);
            }
        } else if frame.id == AddEntity::ID {
            let entity = AddEntity::decode(&mut frame.body)
                .expect("decode generated population AddEntity");
            if entity.entity_type_id == villager_type_id && !parent_ids.contains(&entity.entity_id)
            {
                match &child {
                    Some(existing) => assert_eq!(
                        existing.uuid, entity.uuid,
                        "one courtship must not publish two child identities"
                    ),
                    None => child = Some(entity),
                }
            } else if entity.entity_type_id != villager_type_id {
                non_villager_entities.insert(entity.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let data = ClientboundSetEntityData::decode(&mut frame.body)
                .expect("decode generated villager population metadata");
            if non_villager_entities.contains(&data.entity_id) {
                saw_player_drop |= data.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { stack, .. }
                            if stack.item_id == carrot_id && stack.count == 48
                    )
                });
                saw_villager_share |= data.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { stack, .. }
                            if stack.item_id == carrot_id && stack.count == 24
                    )
                });
            }
            if data.values.iter().any(|value| {
                matches!(
                    value,
                    EntityDataValue::Boolean { index, value: true }
                        if *index == AGEABLE_ENTITY_DATA_BABY_INDEX
                )
            }) {
                baby_metadata.insert(data.entity_id);
            }
        }
        if let Some(child) = child.as_ref()
            && saw_player_drop
            && saw_villager_share
            && heart_parents == *parent_ids
            && baby_metadata.contains(&child.entity_id)
            && birth_events.contains(&child.entity_id)
        {
            return child.clone();
        }
    }
}

struct GeneratedVillageFixture<'a> {
    report: &'a [mc_data::blocks::BlockReport],
    blocks: Arc<mc_world::BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    generator: Arc<mc_worldgen::TerrainGenerator>,
    world_dir: &'a std::path::Path,
    marker: &'a mc_world::SettlementInhabitantMarker,
    villager_type_id: i32,
}

struct GeneratedVillageObservation {
    villager: AddEntity,
    merchant_uses: i32,
    merchant_xp: i32,
}

async fn run_generated_village_observation(
    fixture: &GeneratedVillageFixture<'_>,
    player_name: &str,
    trade: bool,
    save: bool,
) -> GeneratedVillageObservation {
    let shutdown = mc_net::ShutdownHandle::default();
    let bound = mc_net::bind(ruin_server_config(
        fixture.report,
        Arc::clone(&fixture.blocks),
        Arc::clone(&fixture.items),
        Arc::clone(&fixture.generator),
        fixture.world_dir,
        shutdown.clone(),
        "generated village villager gate",
    ))
    .await
    .expect("bind generated village server");
    let addr = bound
        .local_addr()
        .expect("generated village server address");
    let serve = tokio::spawn(async move { bound.serve().await });
    let mut client = connect_ruin_client(addr, player_name).await;
    let (villager, spawn_count) =
        observe_generated_village_villager(&mut client, fixture.marker, fixture.villager_type_id)
            .await;
    assert_eq!(
        spawn_count, 1,
        "chunk marker and restored entity must not duplicate villager"
    );
    let (merchant_uses, merchant_xp) =
        exercise_generated_toolsmith(&mut client, villager.entity_id, &fixture.items, trade).await;

    if save {
        client
            .write_packet(&ServerboundChatCommand {
                command: "save-all".into(),
            })
            .await
            .expect("save generated village");
        wait_for_save_all_feedback(&mut client).await;
    }
    drop(client);
    shutdown.request();
    tokio::time::timeout(Duration::from_secs(5), serve)
        .await
        .expect("generated village shutdown")
        .expect("generated village server join")
        .expect("generated village server result");
    GeneratedVillageObservation {
        villager,
        merchant_uses,
        merchant_xp,
    }
}

async fn exercise_generated_toolsmith(
    client: &mut Client,
    villager_entity_id: i32,
    items: &mc_data::items::ItemRegistry,
    trade: bool,
) -> (i32, i32) {
    let coal_id = ruin_item_id(items, "minecraft:coal");
    let emerald_id = ruin_item_id(items, "minecraft:emerald");
    if trade {
        client
            .write_packet(&ServerboundChatCommand {
                command: "give minecraft:coal 32".into(),
            })
            .await
            .expect("give coal for generated toolsmith trade");
        wait_for_inventory_item(client, coal_id, 32).await;
    }

    client
        .write_packet(&ServerboundInteract {
            entity_id: villager_entity_id,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("open generated toolsmith merchant");
    let opened = wait_for_merchant_screen(client).await;
    let mut content = wait_for_container_content(client, opened.container_id, |packet| {
        packet.items.len() == 39
    })
    .await;
    let offers = wait_for_merchant_offers(client, opened.container_id, |_| true).await;
    assert_eq!(offers.offers.len(), 5);
    assert_eq!(offers.offers[0].cost_a.item_id, coal_id);
    assert_eq!(offers.offers[0].cost_a.count, 15);
    assert_eq!(offers.offers[0].result.item_id, emerald_id);
    assert_eq!(offers.offers[0].result.count, 1);

    if !trade {
        assert_eq!(container_item_count(&content, coal_id), 17);
        assert_eq!(
            offers.offers[0].special_price, 2,
            "same player must retain the persisted villager-hurt surcharge after restart"
        );
        return (offers.offers[0].uses, offers.villager_xp);
    }
    assert_eq!(offers.offers[0].uses, 0);
    assert_eq!(offers.villager_xp, 0);

    client
        .write_packet(&ServerboundSelectTrade { offer_index: 0 })
        .await
        .expect("select generated toolsmith coal trade");
    content = wait_for_container_content(client, opened.container_id, |packet| {
        packet.items.len() == 39
            && packet.items[0].item_id == coal_id
            && packet.items[0].count == 32
            && packet.items[2].item_id == emerald_id
            && packet.items[2].count == 1
    })
    .await;

    let selected_state_id = content.state_id;
    client
        .write_packet(&ServerboundContainerClick {
            container_id: opened.container_id,
            state_id: selected_state_id,
            slot_num: 2,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: HashedStack::empty(),
        })
        .await
        .expect("quick-move generated toolsmith result");
    content = wait_for_container_content(client, opened.container_id, |packet| {
        packet.state_id > selected_state_id
            && packet.items.len() == 39
            && packet.items[0].item_id == coal_id
            && packet.items[0].count == 17
            && packet.items[2].item_id == emerald_id
            && packet.items[2].count == 1
            && packet.items[3..]
                .iter()
                .filter(|stack| stack.item_id == emerald_id)
                .map(|stack| stack.count)
                .sum::<i32>()
                == 1
    })
    .await;
    let _ = wait_for_merchant_offers(client, opened.container_id, |packet| {
        packet.offers[0].uses == 1 && packet.villager_xp == 2
    })
    .await;
    assert_eq!(container_item_count(&content, coal_id), 17);

    client
        .write_packet(&ServerboundContainerClose {
            container_id: opened.container_id,
        })
        .await
        .expect("close generated toolsmith merchant before attack");
    client
        .write_packet(&ServerboundAttack {
            entity_id: villager_entity_id,
        })
        .await
        .expect("attack generated toolsmith for hurt gossip");
    client
        .write_packet(&ServerboundInteract {
            entity_id: villager_entity_id,
            hand: InteractionHand::MainHand,
            location: EntityVec3::ZERO,
            using_secondary_action: false,
        })
        .await
        .expect("reopen generated toolsmith after attack");
    let punished_screen = wait_for_merchant_screen(client).await;
    let punished_offers =
        wait_for_merchant_offers(client, punished_screen.container_id, |packet| {
            packet.offers[0].uses == 1
                && packet.villager_xp == 2
                && packet.offers[0].special_price == 2
        })
        .await;
    client
        .write_packet(&ServerboundSelectTrade { offer_index: 0 })
        .await
        .expect("select punished generated toolsmith trade");
    let punished_content =
        wait_for_container_content(client, punished_screen.container_id, |packet| {
            packet.items.len() == 39
                && packet.items[0].item_id == coal_id
                && packet.items[0].count == 17
                && packet.items[2].item_id == emerald_id
                && packet.items[2].count == 1
        })
        .await;
    assert_eq!(container_item_count(&punished_content, coal_id), 17);
    (punished_offers.offers[0].uses, punished_offers.villager_xp)
}

async fn wait_for_inventory_item(client: &mut Client, item_id: u32, count: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated toolsmith inventory grant");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let packet = ClientboundContainerSetSlot::decode(&mut frame.body)
                .expect("decode generated toolsmith inventory slot");
            if packet.container_id == 0
                && packet.item_stack.item_id == item_id
                && packet.item_stack.count == count
            {
                return;
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let packet = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode generated toolsmith inventory content");
            if packet.container_id == 0 && container_item_count(&packet, item_id) == count {
                return;
            }
        }
    }
}

async fn wait_for_hotbar_inventory_item(client: &mut Client, item_id: u32, count: i32) -> i16 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated villager hotbar inventory grant");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundContainerSetSlot::ID {
            let packet = ClientboundContainerSetSlot::decode(&mut frame.body)
                .expect("decode generated villager hotbar slot");
            if packet.item_stack.item_id == item_id && packet.item_stack.count == count {
                if packet.container_id == -2 && (0..=8).contains(&packet.slot) {
                    return packet.slot;
                }
                if packet.container_id == 0 && (36..=44).contains(&packet.slot) {
                    return packet.slot - 36;
                }
            }
        } else if frame.id == ClientboundContainerSetContent::ID {
            let packet = ClientboundContainerSetContent::decode(&mut frame.body)
                .expect("decode generated villager hotbar inventory content");
            if packet.container_id == 0 {
                for (hotbar_slot, stack) in packet.items.iter().skip(36).take(9).enumerate() {
                    if stack.item_id == item_id && stack.count == count {
                        return i16::try_from(hotbar_slot).expect("hotbar slot fits i16");
                    }
                }
            }
        }
    }
}

async fn wait_for_merchant_screen(client: &mut Client) -> ClientboundOpenScreen {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated toolsmith merchant screen");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundOpenScreen::ID {
            let packet = ClientboundOpenScreen::decode(&mut frame.body)
                .expect("decode generated toolsmith merchant screen");
            if packet.menu_type == 19 {
                return packet;
            }
        }
    }
}

async fn wait_for_merchant_offers(
    client: &mut Client,
    container_id: i32,
    predicate: impl Fn(&ClientboundMerchantOffers) -> bool,
) -> ClientboundMerchantOffers {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated toolsmith merchant offers");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == ClientboundMerchantOffers::ID {
            let packet = ClientboundMerchantOffers::decode(&mut frame.body)
                .expect("decode generated toolsmith merchant offers");
            if packet.container_id == container_id && predicate(&packet) {
                return packet;
            }
        }
    }
}

async fn observe_generated_village_villager(
    client: &mut Client,
    marker: &mc_world::SettlementInhabitantMarker,
    villager_type_id: i32,
) -> (AddEntity, usize) {
    client
        .write_packet(&ServerboundMovePlayerPosRot {
            x: marker.position[0],
            y: marker.position[1],
            z: marker.position[2],
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move to generated village");

    let target_chunk = (
        (marker.position[0].floor() as i32).div_euclid(16),
        (marker.position[2].floor() as i32).div_euclid(16),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    let mut chunk_loaded = false;
    let mut villager = None;
    let mut spawn_count = 0_usize;
    let mut matching_metadata = std::collections::HashSet::new();
    loop {
        let mut frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("generated village visibility");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let correction = SynchronizePlayerPosition::decode(&mut frame.body)
                .expect("decode village movement correction");
            panic!("movement to generated village was rejected: {correction:?}");
        }
        if frame.id == LevelChunkWithLight::ID {
            let chunk = LevelChunkWithLight::decode(&mut frame.body)
                .expect("decode generated village chunk");
            chunk_loaded |= (chunk.chunk_x, chunk.chunk_z) == target_chunk;
        } else if frame.id == AddEntity::ID {
            let entity = AddEntity::decode(&mut frame.body).expect("decode village villager");
            if entity.entity_type_id == villager_type_id {
                spawn_count += 1;
                villager.get_or_insert(entity);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let data = ClientboundSetEntityData::decode(&mut frame.body)
                .expect("decode generated villager metadata");
            if data.values.iter().any(|value| {
                matches!(
                    value,
                    EntityDataValue::VillagerData {
                        index: 19,
                        villager_type: 2,
                        profession: 13,
                        level: 1,
                    }
                )
            }) {
                matching_metadata.insert(data.entity_id);
            }
        }
        if chunk_loaded
            && villager
                .as_ref()
                .is_some_and(|entity| matching_metadata.contains(&entity.entity_id))
        {
            break;
        }
    }

    let settle_deadline = tokio::time::Instant::now() + Duration::from_millis(750);
    while tokio::time::Instant::now() < settle_deadline {
        let remaining = settle_deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Ok(mut frame)) = tokio::time::timeout(remaining, client.read_frame()).await else {
            break;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let entity = AddEntity::decode(&mut frame.body).expect("decode late village entity");
            spawn_count += usize::from(entity.entity_type_id == villager_type_id);
        }
    }

    (villager.expect("generated villager AddEntity"), spawn_count)
}

#[test]
fn empty_world_plus_generator_produces_terrain_on_demand() {
    let test = std::thread::Builder::new()
        .name("empty_world_plus_generator_produces_terrain_on_demand".to_owned())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build worldgen integration runtime")
                .block_on(empty_world_plus_generator_produces_terrain_on_demand_inner());
        })
        .expect("spawn worldgen integration thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn empty_world_plus_generator_produces_terrain_on_demand_inner() {
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
