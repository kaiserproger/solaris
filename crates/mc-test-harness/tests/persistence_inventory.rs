//! M6.g — raw-TCP integration test for the persistence + inventory
//! round-trip.
//!
//! Two scenarios in one test:
//!
//! 1. After login the server emits `ClientboundSetHeldSlot` and a
//!    `ClientboundContainerSetContent` packet with the M6 starter kit.
//!    The hotbar slots 36..=39 must contain stone, dirt, oak planks,
//!    torch.
//! 2. Send `ServerboundSetCarriedItem{slot=1}` to select dirt, then
//!    a `ServerboundUseItemOn` on the grass cell at world (0, -61, 0)
//!    with `direction = Up` so the placed block lands at (0, -60, 0).
//!    Assert the resulting `BlockUpdate` carries the dirt block-state
//!    id (not stone, the M5 fallback) and a `ContainerSetSlot` ships
//!    with the slot 36-not-actually-touched-but-37-decremented update.
//! 3. Lock the shared `WorldStorage` handle and call `flush_dirty()`.
//!    Drop the world, re-open it from the same path, and assert the
//!    placed dirt block is visible at (0, -60, 0) — proves the edit
//!    landed on disk, not just in the LRU.
//!
//! Skipped silently when the test-world or required vanilla data
//! sidecars are missing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundContainerSetContent, ClientboundContainerSetSlot,
    ClientboundKeepAlive, ClientboundSetHeldSlot, ConfirmTeleportation, Direction, GameEvent,
    LevelChunkWithLight, LoginPlay, ServerboundKeepAlive, ServerboundSetCarriedItem,
    ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition, pack_block_pos,
    unpack_block_pos,
};
use mc_test_harness::client::Client;
use std::collections::HashSet;

const VIEW_DISTANCE: i32 = 2;

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

#[tokio::test]
async fn place_dirt_persists_through_flush_to_disk() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_src = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !world_src.exists() || !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping: prerequisites missing under {}",
            vanilla_dir.display()
        );
        return;
    }

    // Stage a writable copy of the world under tempfile.
    let tmp_world = tempfile::tempdir().unwrap();
    copy_dir_recursive(&world_src, tmp_world.path());

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
    .expect("world storage opens")
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!("skipping: {} ({err})", block_light_path.display());
            return;
        }
    };

    // Resolve the dirt + stone state ids so the test isn't pinned to
    // 26.1.2 numerics. Dirt is what the server *should* place from
    // the held item; stone is the M5 fallback we must NOT see.
    let dirt_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("dirt in registry");
    let stone_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("stone in registry");
    // Sanity: the two must differ (otherwise the assertion is vacuous).
    assert_ne!(dirt_state_id, stone_state_id);

    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item id");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M6.g persistence + inventory".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        block_light,
        items: Arc::clone(&items),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M6gTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let event: GameEvent = client.read_typed().await.expect("GameEvent");
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);
    let _: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    // Drain the spawn burst + collect the SetHeldSlot + ContainerSetContent
    // emitted by the M6 seed (they ship after the chunk burst).
    let expected_chunks = (2 * VIEW_DISTANCE + 1).pow(2) as usize;
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let mut saw_set_held_slot = false;
    let mut saw_set_content = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while chunks_seen.len() < expected_chunks || !saw_set_held_slot || !saw_set_content {
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
            assert_eq!(pkt.slot, 0, "M6 seeds slot 0 on login");
            saw_set_held_slot = true;
        } else if frame.id == ClientboundContainerSetContent::ID {
            let mut body = frame.body;
            let pkt = ClientboundContainerSetContent::decode(&mut body)
                .expect("decode ContainerSetContent");
            assert_eq!(pkt.container_id, 0, "window 0 = player inventory");
            assert_eq!(pkt.items.len(), 46, "46-slot window-0 inventory");
            assert!(
                pkt.items[36].item_id
                    == items
                        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
                        .unwrap(),
                "starter kit slot 36 = stone"
            );
            assert!(
                pkt.items[37].item_id == dirt_item_id,
                "starter kit slot 37 = dirt",
            );
            saw_set_content = true;
        }
    }

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
            assert_eq!(
                (px, py, pz),
                (0, placed_y, 0),
                "BlockUpdate position must match the placement target",
            );
            assert_eq!(
                pkt.state_id, dirt_state_id,
                "M6.f must place dirt (held item), not stone (M5 fallback)",
            );
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
            assert_eq!(pkt.container_id, 0);
            assert_eq!(
                pkt.slot, 37,
                "decrement target is hotbar slot 1 (= wire slot 37)",
            );
            assert_eq!(
                pkt.item_stack.count, 63,
                "stack starts at 64, one placement leaves 63",
            );
            assert_eq!(
                pkt.item_stack.item_id, dirt_item_id,
                "decremented slot still references dirt",
            );
            saw_container_set_slot = true;
        }
        // Ignore stray frames (light updates, keepalive, …).
    }

    // 3. Flush the world to disk and re-open from scratch. The placed
    //    dirt block must survive the round-trip.
    {
        let mut guard = world_handle.lock().await;
        let n = guard.flush_dirty().expect("flush_dirty");
        assert!(n >= 1, "at least one dirty chunk should have been flushed");
    }
    drop(client);

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
