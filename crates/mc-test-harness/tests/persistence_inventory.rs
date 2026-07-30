//! M6.g — raw-TCP integration test for the persistence + inventory
//! round-trip.
//!
//! Two scenarios in one test:
//!
//! 1. After login the server emits `ClientboundContainerSetContent`
//!    with the authoritative player inventory.
//!    The hotbar slots 36..=39 must contain stone, dirt, oak planks,
//!    torch.
//! 2. Send `ServerboundSetCarriedItem{slot=1}` to select dirt, then
//!    a `ServerboundUseItemOn` on the grass cell at world (0, -61, 0)
//!    with `direction = Up` so the placed block lands at (0, -60, 0).
//!    Assert the resulting `BlockUpdate` carries the dirt block-state
//!    id (not stone, the M5 fallback) and a `ContainerSetSlot` ships
//!    with the slot 36-not-actually-touched-but-37-decremented update.
//! 3. Stop the live server through its production save/drain path, await
//!    exact server task completion, re-open the same path, and assert the
//!    placed dirt block is visible at (0, -60, 0) — proves the edit
//!    landed on disk, not just in the LRU.
//!
//! Skipped silently when required vanilla data sidecars are missing.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use flate2::Compression as GzipCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mc_nbt::Tag;
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, BlockUpdate, ClientboundContainerSetContent,
    ClientboundContainerSetSlot, ClientboundKeepAlive, ClientboundSetEntityData,
    ClientboundSetHeldSlot, ConfirmTeleportation, Direction, EntityDataValue, GameEvent,
    LevelChunkWithLight, MovePlayerFlags, PlayerActionKind, RemoveEntities, ServerboundChatCommand,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundPlayerAction,
    ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition,
    pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;
use std::collections::HashSet;

const VIEW_DISTANCE: i32 = 2;

#[test]
#[ignore = "requires local data/vanilla sidecars"]
fn place_dirt_persists_through_flush_to_disk() {
    let test = std::thread::Builder::new()
        .name("persistence-inventory".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build persistence test runtime")
                .block_on(place_dirt_persists_through_flush_to_disk_inner());
        })
        .expect("spawn persistence test thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn place_dirt_persists_through_flush_to_disk_inner() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        panic!(
            "prerequisite failed: prerequisites missing under {}",
            vanilla_dir.display()
        );
    }

    let tmp_world = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp_world.path().join("region")).unwrap();

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::open_with_capacity(
        tmp_world.path(),
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("temp world opens")
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            panic!(
                "prerequisite failed: {} ({err})",
                block_light_path.display()
            );
        }
    };

    // Resolve the dirt + stone state ids so the test isn't pinned to
    // 26.1.2 numerics. Dirt is what the server *should* place from
    // the held item; stone is the M5 fallback we must NOT see.
    let dirt_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default)
        .expect("dirt in registry");
    let stone_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default)
        .expect("stone in registry");
    let air_state = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default)
        .expect("air in registry");
    let dirt_state_id = dirt_state.0 as i32;
    let stone_state_id = stone_state.0 as i32;
    // Sanity: the two must differ (otherwise the assertion is vacuous).
    assert_ne!(dirt_state_id, stone_state_id);

    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item id");

    let shutdown = mc_net::ShutdownHandle::default();
    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M6.g persistence + inventory".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy::default(),
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: shutdown.clone(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    let save = bound.save_handle();
    let server = tokio::spawn(async move { bound.serve().await });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M6gTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let event: GameEvent = client.read_typed().await.expect("GameEvent");
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    // Drain the spawn burst + collect the empty inventory seed.
    let expected_chunks = (2 * VIEW_DISTANCE + 1).pow(2) as usize;
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let mut saw_set_content = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while chunks_seen.len() < expected_chunks || !saw_set_content {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| panic!("login burst stalled: {e}"));
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode");
            chunks_seen.insert((pkt.chunk_x, pkt.chunk_z));
        } else if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
        } else if frame.id == ClientboundSetHeldSlot::ID {
            let mut body = frame.body;
            let pkt = ClientboundSetHeldSlot::decode(&mut body).expect("decode SetHeldSlot");
            assert_eq!(pkt.slot, 0, "login selects slot 0");
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode ContainerSetContent");
            assert_eq!(pkt.container_id, 0, "window 0 = player inventory");
            assert_eq!(pkt.items.len(), 46, "46-slot window-0 inventory");
            assert!(
                pkt.items
                    .iter()
                    .all(mc_protocol::packets::play::ItemStack::is_empty),
                "normal play starts with no implicit starter kit"
            );
            saw_set_content = true;
        }
    }

    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 64 1".into(),
        })
        .await
        .expect("seed dirt slot");
    wait_for_debug_give_slot(&mut client, dirt_item_id).await;

    // 1. Select dirt (hotbar slot 1).
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await
        .expect("send SetCarriedItem");

    // 2. Right-click the top block under spawn with direction Up.
    //    The old flat oracle used (-61 -> -60); Solaris-generated
    //    worlds choose spawn Y adaptively as `top + 2`.
    let target_y = sync.y.floor() as i32 - 2;
    let placed_y = target_y + 1;
    {
        let mut world = world_handle.lock().await;
        world
            .set_block_at(
                mc_world::BlockPos {
                    x: 0,
                    y: target_y,
                    z: 0,
                },
                stone_state,
            )
            .expect("seed deterministic placement support");
        world
            .set_block_at(
                mc_world::BlockPos {
                    x: 0,
                    y: placed_y,
                    z: 0,
                },
                air_state,
            )
            .expect("clear deterministic placement target");
    }
    let target_pos = pack_block_pos(0, target_y, 0);
    let sequence: i32 = 1;
    client
        .write_packet(&ServerboundUseItemOn {
            hand: mc_protocol::packets::play::InteractionHand::MainHand,
            position: target_pos,
            direction: Direction::Up,
            cursor_x: 0.5,
            cursor_y: 1.0,
            cursor_z: 0.5,
            inside: false,
            world_border_hit: false,
            sequence,
        })
        .await
        .expect("send UseItemOn");

    // Collect the resulting wire response. The server emits:
    //   BlockUpdate → LightUpdate × 5 → BlockChangedAck → ContainerSetSlot.
    let mut saw_block_update = false;
    let mut saw_ack = false;
    let mut saw_container_set_slot = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_update && saw_ack && saw_container_set_slot) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "place response stalled: block_update={saw_block_update}, ack={saw_ack}, \
                     container_set_slot={saw_container_set_slot}: {e}"
                )
            });
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            if (px, py, pz) != (0, placed_y, 0) || pkt.state_id != dirt_state_id {
                continue;
            }
            saw_block_update = true;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            assert_eq!(
                pkt.sequence, sequence,
                "ack must echo the action's sequence",
            );
            saw_ack = true;
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let pkt =
                ClientboundContainerSetSlot::decode(&mut body).expect("decode ContainerSetSlot");
            if pkt.container_id != 0
                || pkt.slot != 37
                || pkt.item_stack.count != 63
                || pkt.item_stack.item_id != dirt_item_id
            {
                continue;
            }
            saw_container_set_slot = true;
        }
        // Ignore stray frames (light updates, keepalive, …).
    }

    // 3. Use the production listener drain, then save through the coordinator
    //    after runtime owners have stopped mutating state.
    shutdown.request();
    drop(client);
    let serve_result = tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("server should stop after the shutdown request")
        .expect("server task should join");
    serve_result.expect("server should drain and stop cleanly");
    let save_report = save.save_all_after_drain().await;
    assert!(
        save_report.is_ok(),
        "post-drain save failed: {:?}",
        save_report.errors
    );
    assert_eq!(
        world_handle.lock().await.dirty_count(),
        0,
        "production shutdown must leave no dirty chunks"
    );

    let mut fresh =
        mc_world::WorldStorage::open(tmp_world.path(), Arc::clone(&blocks)).expect("reopen");
    let landed = fresh
        .get_block(mc_world::BlockPos {
            x: 0,
            y: placed_y,
            z: 0,
        })
        .unwrap()
        .expect("placed cell present");
    let resolved = blocks.by_id(landed).expect("state id resolves");
    assert_eq!(
        resolved.block.id.as_str(),
        "minecraft:dirt",
        "the placed block must persist as dirt on disk",
    );
}

#[test]
#[ignore = "requires local data/vanilla sidecars"]
fn item_despawn_deadline_survives_restart() {
    let test = std::thread::Builder::new()
        .name("item-despawn-restart".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build item despawn test runtime")
                .block_on(item_despawn_deadline_survives_restart_inner());
        })
        .expect("spawn item despawn test thread");
    if let Err(panic) = test.join() {
        std::panic::resume_unwind(panic);
    }
}

async fn item_despawn_deadline_survives_restart_inner() {
    const SERVER_VIEW_DISTANCE: i32 = 0;
    const REMAINING_TICKS_AFTER_REWRITE: u64 = 200;

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        panic!(
            "prerequisite failed: prerequisites missing under {}",
            vanilla_dir.display()
        );
    }

    let tmp_world = tempfile::tempdir().expect("item despawn temp world");
    std::fs::create_dir_all(tmp_world.path().join("region"))
        .expect("create item despawn region directory");
    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    let item_entity_type = entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:item").unwrap())
        .and_then(|id| i32::try_from(id).ok())
        .expect("item entity type id");
    let birch_log_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:birch_log").unwrap())
        .expect("birch log item id");

    let first_storage =
        mc_world::WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&blocks), 25)
            .expect("open first item despawn world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
                0,
                Arc::clone(&blocks),
            )));
    let first_shutdown = mc_net::ShutdownHandle::default();
    let first_cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "T04 item despawn save".into(),
        max_players: 8,
        view_distance: SERVER_VIEW_DISTANCE,
        data: Arc::clone(&data),
        blocks: Arc::clone(&blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(first_storage))),
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
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: first_shutdown.clone(),
    };
    let first_bound = mc_net::bind(first_cfg)
        .await
        .expect("bind first item despawn server");
    let first_addr = first_bound.local_addr().expect("first item despawn addr");
    let mut first_ticks = first_bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    let first_serve = tokio::spawn(async move { first_bound.serve_and_save().await });

    let (mut client, sync) = connect_persistence_play(first_addr, "DespawnClock").await;
    wait_for_persistence_chunk(&mut client).await;
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:birch_log 1 0".into(),
        })
        .await
        .expect("give item despawn fixture");
    wait_for_item_slot_count(&mut client, birch_log_id, 1).await;
    let entity_id =
        drop_selected_item_for_persistence(&mut client, item_entity_type, birch_log_id, 7_001)
            .await;

    let observed_tick = *first_ticks.borrow_and_update();
    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 6.5,
            y: sync.y,
            z: 0.5,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("move persisted observer away from dropped item");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            first_ticks
                .changed()
                .await
                .expect("first simulation tick publisher remains active");
            if *first_ticks.borrow_and_update() > observed_tick {
                return;
            }
        }
    })
    .await
    .expect("movement is followed by an owner tick");

    drop(client);
    first_shutdown.request();
    tokio::time::timeout(Duration::from_secs(30), first_serve)
        .await
        .expect("first item despawn server shutdown")
        .expect("first item despawn server join")
        .expect("first item despawn server serve");

    let (persisted_id, checkpoint_tick, despawn_tick) = rewrite_item_checkpoint_before_deadline(
        tmp_world.path(),
        entity_id,
        mc_net::ITEM_DESPAWN_AGE_TICKS,
        REMAINING_TICKS_AFTER_REWRITE,
    );
    assert_eq!(persisted_id, entity_id);

    let second_storage =
        mc_world::WorldStorage::open_with_capacity(tmp_world.path(), Arc::clone(&blocks), 25)
            .expect("open restarted item despawn world")
            .with_item_registry(Arc::clone(&items))
            .with_generator(Arc::new(mc_worldgen::TerrainGenerator::new(
                0,
                Arc::clone(&blocks),
            )));
    let second_shutdown = mc_net::ShutdownHandle::default();
    let second_cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "T04 item despawn restart".into(),
        max_players: 8,
        view_distance: SERVER_VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world: Some(Arc::new(tokio::sync::Mutex::new(second_storage))),
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 0,
            friendly_spawn_interval_ticks: 0,
            hostile_spawn_interval_ticks: 0,
            ..mc_net::RandomTickPolicy::default()
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: second_shutdown.clone(),
    };
    let second_bound = mc_net::bind(second_cfg)
        .await
        .expect("bind restarted item despawn server");
    let second_addr = second_bound
        .local_addr()
        .expect("restarted item despawn addr");
    let mut second_ticks = second_bound
        .runtime_telemetry_handle()
        .subscribe_simulation_ticks();
    assert_eq!(
        *second_ticks.borrow_and_update(),
        checkpoint_tick,
        "bind must restore the authoritative lifecycle clock before serving"
    );
    let second_serve = tokio::spawn(async move { second_bound.serve_and_save().await });

    let (mut client, rejoin_sync) = connect_persistence_play(second_addr, "DespawnClock").await;
    assert_eq!(rejoin_sync.x, 6.5, "rejoin must stay outside pickup radius");
    let mut saw_chunk = false;
    let mut saw_entity = false;
    let mut saw_stack = false;
    let visibility_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_chunk && saw_entity && saw_stack) {
        let frame = client
            .read_frame_with_timeout(
                visibility_deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("restored item visibility before despawn");
        if echo_persistence_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            saw_chunk = true;
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode restored item AddEntity");
            if packet.entity_id == entity_id {
                assert_eq!(packet.entity_type_id, item_entity_type);
                saw_entity = true;
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet =
                ClientboundSetEntityData::decode(&mut body).expect("decode restored item metadata");
            if packet.entity_id == entity_id {
                saw_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { stack, .. }
                            if stack.item_id == birch_log_id && stack.count == 1
                    )
                });
            }
        } else if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let packet = RemoveEntities::decode(&mut body).expect("decode early item removal");
            assert!(
                !packet.entity_ids.contains(&entity_id),
                "restored item despawned before it became visible"
            );
        }
    }
    assert!(
        *second_ticks.borrow() < despawn_tick,
        "restored item must be visible before its exact deadline"
    );

    let removal_wall_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let removal_tick = loop {
        let frame = client
            .read_frame_with_timeout(
                removal_wall_deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("restored item removal at deadline");
        if echo_persistence_keepalive(&mut client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != RemoveEntities::ID {
            continue;
        }
        let mut body = frame.body;
        let packet = RemoveEntities::decode(&mut body).expect("decode item deadline removal");
        if packet.entity_ids.contains(&entity_id) {
            break *second_ticks.borrow();
        }
    };
    assert!(
        removal_tick >= despawn_tick,
        "item despawned early: removal={removal_tick}, deadline={despawn_tick}"
    );
    assert!(
        removal_tick <= despawn_tick.saturating_add(2),
        "item lived past its bounded deadline: removal={removal_tick}, deadline={despawn_tick}"
    );

    drop(client);
    second_shutdown.request();
    tokio::time::timeout(Duration::from_secs(30), second_serve)
        .await
        .expect("second item despawn server shutdown")
        .expect("second item despawn server join")
        .expect("second item despawn server serve");
    assert_eq!(
        persisted_entity_count(tmp_world.path()),
        0,
        "post-despawn save must not resurrect the removed item"
    );
}

async fn connect_persistence_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _ = client.read_play_login().await.expect("play entry");
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: mc_protocol::packets::play::ClientboundInitializeBorder =
        client.read_typed().await.expect("InitializeBorder");
    let _: mc_protocol::packets::play::ClientboundSetTime =
        client.read_typed().await.expect("SetTime");
    let _: mc_protocol::packets::play::SetDefaultSpawnPosition =
        client.read_typed().await.expect("SetDefaultSpawnPosition");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");
    (client, sync)
}

async fn wait_for_persistence_chunk(client: &mut Client) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("persistence test chunk");
        if echo_persistence_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            return;
        }
    }
}

async fn wait_for_item_slot_count(client: &mut Client, item_id: u32, count: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("item fixture slot update");
        if echo_persistence_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id != ClientboundContainerSetSlot::ID {
            continue;
        }
        let mut body = frame.body;
        let packet =
            ClientboundContainerSetSlot::decode(&mut body).expect("decode item fixture slot");
        if packet.slot == 36
            && packet.item_stack.item_id == item_id
            && packet.item_stack.count == count
        {
            return;
        }
    }
}

async fn drop_selected_item_for_persistence(
    client: &mut Client,
    item_entity_type: i32,
    item_id: u32,
    sequence: i32,
) -> i32 {
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::DropItem,
            position: 0,
            direction: Direction::Down,
            sequence,
        })
        .await
        .expect("drop persisted item fixture");
    let mut entity_ids = HashSet::new();
    let mut dropped_id = None;
    let mut saw_stack = false;
    let mut saw_slot = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_stack && saw_slot && saw_ack) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("persisted item drop response");
        if echo_persistence_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let packet = AddEntity::decode(&mut body).expect("decode persisted item AddEntity");
            if packet.entity_type_id == item_entity_type {
                entity_ids.insert(packet.entity_id);
                dropped_id = Some(packet.entity_id);
            }
        } else if frame.id == ClientboundSetEntityData::ID {
            let mut body = frame.body;
            let packet = ClientboundSetEntityData::decode(&mut body)
                .expect("decode persisted item metadata");
            if entity_ids.contains(&packet.entity_id) {
                saw_stack |= packet.values.iter().any(|value| {
                    matches!(
                        value,
                        EntityDataValue::ItemStack { stack, .. }
                            if stack.item_id == item_id && stack.count == 1
                    )
                });
            }
        } else if frame.id == ClientboundContainerSetSlot::ID {
            let mut body = frame.body;
            let packet = ClientboundContainerSetSlot::decode(&mut body)
                .expect("decode persisted item slot debit");
            saw_slot |= packet.slot == 36 && packet.item_stack.is_empty();
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let packet = BlockChangedAck::decode(&mut body).expect("decode persisted item ack");
            saw_ack |= packet.sequence == sequence;
        }
    }
    dropped_id.expect("persisted item entity id")
}

async fn echo_persistence_keepalive(client: &mut Client, id: i32, body: &[u8]) -> bool {
    if id != ClientboundKeepAlive::ID {
        return false;
    }
    let mut body = body;
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}

fn rewrite_item_checkpoint_before_deadline(
    world_root: &Path,
    expected_entity_id: i32,
    item_despawn_age_ticks: u64,
    remaining_ticks: u64,
) -> (i32, u64, u64) {
    let path = world_root.join("solaris").join("entities.dat");
    let (root_name, mut root) = read_gzip_named_tag(&path);
    let Tag::Compound(root_fields) = &root else {
        panic!("entity checkpoint root must be a compound")
    };
    let saved_clock = match nbt_field(root_fields, "SolarisEntityLifecycleTick") {
        Some(Tag::Long(value)) if *value >= 0 => *value as u64,
        other => panic!("entity checkpoint lifecycle clock missing: {other:?}"),
    };
    let Tag::List(entities) = nbt_field(root_fields, "Entities").expect("Entities field") else {
        panic!("entity checkpoint Entities must be a list")
    };
    assert_eq!(
        entities.elements.len(),
        1,
        "checkpoint must contain one item"
    );
    let Tag::Compound(entity_fields) = &entities.elements[0] else {
        panic!("persisted item must be a compound")
    };
    let entity_id = match nbt_field(entity_fields, "SolarisEntityId") {
        Some(Tag::Int(value)) => *value,
        other => panic!("persisted entity id missing: {other:?}"),
    };
    assert_eq!(entity_id, expected_entity_id);
    let retained = match nbt_field(entity_fields, "SolarisRetainedState") {
        Some(Tag::String(value)) => serde_json::from_str::<serde_json::Value>(value)
            .expect("decode persisted retained state"),
        other => panic!("persisted retained state missing: {other:?}"),
    };
    let spawn_tick = retained
        .get("spawn_tick")
        .and_then(serde_json::Value::as_u64)
        .expect("persisted item spawn_tick");
    let ready_tick = retained
        .get("item_pickup_ready_tick")
        .and_then(serde_json::Value::as_u64);
    let despawn_tick = spawn_tick.saturating_add(item_despawn_age_ticks);
    let checkpoint_tick = despawn_tick
        .checked_sub(remaining_ticks)
        .expect("remaining ticks fit item lifetime");
    assert!(
        checkpoint_tick > saved_clock,
        "test rewrite must advance the lifecycle clock"
    );

    let Tag::Compound(root_fields) = &mut root else {
        unreachable!()
    };
    set_nbt_field(
        root_fields,
        "SolarisEntityLifecycleTick",
        Tag::Long(i64::try_from(checkpoint_tick).expect("checkpoint tick fits NBT long")),
    );
    let Tag::List(entities) = nbt_field_mut(root_fields, "Entities").expect("Entities field")
    else {
        unreachable!()
    };
    let Tag::Compound(entity_fields) = &mut entities.elements[0] else {
        unreachable!()
    };
    set_nbt_field(
        entity_fields,
        "SolarisLifetimeAge",
        Tag::Int(
            i32::try_from(checkpoint_tick.saturating_sub(spawn_tick))
                .expect("item age fits NBT int"),
        ),
    );
    set_nbt_field(
        entity_fields,
        "PickupDelay",
        Tag::Int(
            i32::try_from(
                ready_tick
                    .unwrap_or(checkpoint_tick)
                    .saturating_sub(checkpoint_tick),
            )
            .expect("pickup delay fits NBT int"),
        ),
    );
    write_gzip_named_tag(&path, &root_name, &root);
    (entity_id, checkpoint_tick, despawn_tick)
}

fn persisted_entity_count(world_root: &Path) -> usize {
    let path = world_root.join("solaris").join("entities.dat");
    let (_, root) = read_gzip_named_tag(&path);
    let Tag::Compound(fields) = root else {
        panic!("entity checkpoint root must be a compound")
    };
    let Tag::List(entities) = nbt_field(&fields, "Entities").expect("Entities field") else {
        panic!("entity checkpoint Entities must be a list")
    };
    entities.elements.len()
}

fn read_gzip_named_tag(path: &Path) -> (String, Tag) {
    let file = File::open(path).unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
    let mut decoder = GzDecoder::new(file);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .unwrap_or_else(|error| panic!("decompress {}: {error}", path.display()));
    let mut input = bytes.as_slice();
    mc_nbt::read_named(&mut input)
        .unwrap_or_else(|error| panic!("decode {}: {error}", path.display()))
}

fn write_gzip_named_tag(path: &Path, root_name: &str, root: &Tag) {
    let tmp_path = path.with_extension("dat.tmp");
    let file = File::create(&tmp_path)
        .unwrap_or_else(|error| panic!("create {}: {error}", tmp_path.display()));
    let mut encoder = GzEncoder::new(file, GzipCompression::default());
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, root_name, root)
        .unwrap_or_else(|error| panic!("encode {}: {error}", path.display()));
    encoder
        .write_all(&bytes)
        .unwrap_or_else(|error| panic!("compress {}: {error}", tmp_path.display()));
    let file = encoder
        .finish()
        .unwrap_or_else(|error| panic!("finish {}: {error}", tmp_path.display()));
    file.sync_all()
        .unwrap_or_else(|error| panic!("sync {}: {error}", tmp_path.display()));
    std::fs::rename(&tmp_path, path).unwrap_or_else(|error| {
        panic!(
            "replace {} with {}: {error}",
            path.display(),
            tmp_path.display()
        )
    });
}

fn nbt_field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
}

fn nbt_field_mut<'a>(fields: &'a mut [(String, Tag)], name: &str) -> Option<&'a mut Tag> {
    fields
        .iter_mut()
        .find_map(|(field, value)| (field == name).then_some(value))
}

fn set_nbt_field(fields: &mut Vec<(String, Tag)>, name: &str, value: Tag) {
    if let Some(existing) = nbt_field_mut(fields, name) {
        *existing = value;
    } else {
        fields.push((name.to_string(), value));
    }
}

async fn wait_for_debug_give_slot(client: &mut Client, dirt_item_id: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("debug give ContainerSetSlot");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id != ClientboundContainerSetSlot::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = ClientboundContainerSetSlot::decode(&mut body).expect("decode ContainerSetSlot");
        if pkt.slot == 37 {
            assert_eq!(pkt.item_stack.item_id, dirt_item_id);
            assert_eq!(pkt.item_stack.count, 64);
            return;
        }
    }
}
