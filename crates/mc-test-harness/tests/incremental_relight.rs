//! M9.d — wire parity gate for the incremental relight engine.
//!
//! Boots `mc_net::run` on an ephemeral port against a writable copy
//! of `.analysis/test-world/`, drives the spawn burst, places a
//! dirt block mid-chunk via `ServerboundUseItemOn`, and asserts:
//!
//! - Exactly one `ClientboundLightUpdate` arrives for the centre
//!   chunk (`(0, 0)`). The mid-chunk edit is far enough from any
//!   chunk seam that no neighbouring chunk's light is touched —
//!   M9.b's bounded BFS produces a touched-list of length 1.
//! - The wire payload of that update is **byte-identical** to the
//!   wire encoding of a fresh full-recompute over the post-edit
//!   3×3 chunks. Pins the incremental BFS against the M4 reference
//!   engine end-to-end.
//!
//! Skipped silently when the test-world or required vanilla data
//! sidecars are missing.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundKeepAlive, ConfirmTeleportation, Direction, GameEvent,
    LevelChunkWithLight, LightUpdate, LoginPlay, ServerboundKeepAlive, ServerboundSetCarriedItem,
    ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition, pack_block_pos,
};
use mc_test_harness::client::Client;
use mc_world::light::{LightWorkspace, compute_chunk_light_in};
use mc_world::wire::encode_chunk_light;
use mc_world::{Chunk, ChunkPos};

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
async fn incremental_relight_wire_matches_full_recompute() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_src = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    if !world_src.exists()
        || !blocks_json.exists()
        || !registries_json.exists()
        || !block_light_path.exists()
    {
        eprintln!(
            "skipping: prerequisites missing under {}",
            vanilla_dir.display()
        );
        return;
    }

    let tmp_world = tempfile::tempdir().unwrap();
    copy_dir_recursive(&world_src, tmp_world.path());

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let storage = mc_world::WorldStorage::open(tmp_world.path(), Arc::clone(&blocks))
        .expect("world storage opens");
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light =
        Arc::new(mc_data::block_light::load(&block_light_path).expect("block_light loads"));

    let dirt_item_id = items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .expect("dirt item id");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M9.d incremental relight".into(),
        max_players: 8,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        block_light: Some(Arc::clone(&block_light)),
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
        .drive_login(addr, "M9dTester")
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

    // Drain the spawn burst (441 chunks at view distance 10).
    let expected_chunks = (2 * 10 + 1u32).pow(2) as usize;
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while chunks_seen.len() < expected_chunks {
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
        }
    }

    // Select dirt (hotbar slot 1) and place one block above the
    // top block at local (8, 8). That column is far enough from every
    // chunk seam (lx/lz in 1..=14) that light propagation stays inside
    // chunk (0, 0).
    let target_y = {
        let mut guard = world_handle.lock().await;
        let chunk = guard
            .get_chunk_mut(ChunkPos { x: 0, z: 0 })
            .expect("chunk read")
            .expect("origin chunk present");
        chunk.rebuild_highest_opaque(&block_light);
        chunk
            .highest_opaque_y(8, 8)
            .expect("origin local column has terrain")
    };
    client
        .write_packet(&ServerboundSetCarriedItem { slot: 1 })
        .await
        .expect("send SetCarriedItem");
    let target_pos = pack_block_pos(8, target_y, 8);
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

    // Collect LightUpdate frames. We expect exactly one for chunk
    // (0, 0). Stop after the BlockChangedAck arrives, since the
    // server emits updates in order BlockUpdate → LightUpdate(s) →
    // BlockChangedAck.
    let _ = dirt_item_id;
    let mut light_updates: Vec<LightUpdate> = Vec::new();
    let mut saw_block_update = false;
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !saw_ack {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| panic!("place response stalled: {e}"));
        if frame.id == BlockUpdate::ID {
            saw_block_update = true;
        } else if frame.id == LightUpdate::ID {
            let mut body = frame.body;
            let pkt = LightUpdate::decode(&mut body).expect("decode LightUpdate");
            light_updates.push(pkt);
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            assert_eq!(pkt.sequence, sequence);
            saw_ack = true;
        }
    }
    assert!(saw_block_update, "BlockUpdate must arrive before ack");

    // M9.c claim: mid-chunk edit -> exactly one LightUpdate, for
    // the centre chunk. (Edge-of-chunk edits can touch up to 9.)
    assert_eq!(
        light_updates.len(),
        1,
        "expected exactly 1 LightUpdate for a mid-chunk edit, got {} ({:?})",
        light_updates.len(),
        light_updates
            .iter()
            .map(|u| (u.chunk_x, u.chunk_z))
            .collect::<Vec<_>>(),
    );
    let pkt = &light_updates[0];
    assert_eq!((pkt.chunk_x, pkt.chunk_z), (0, 0));

    // Wire parity: rebuild the expected light arrays from a fresh
    // full-recompute on the post-edit 3×3 chunks pulled out of the
    // same world storage. The byte-for-byte payload must match.
    let chunks_3x3 = {
        let mut guard = world_handle.lock().await;
        let mut out: Vec<(ChunkPos, Option<Chunk>)> = Vec::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let p = ChunkPos { x: dx, z: dz };
                let chunk = guard.get_chunk(p).expect("chunk read").cloned();
                out.push((p, chunk));
            }
        }
        out
    };
    let mut refs: [[Option<&Chunk>; 3]; 3] = [[None; 3]; 3];
    for (p, chunk) in &chunks_3x3 {
        let dx = (p.x + 1) as usize;
        let dz = (p.z + 1) as usize;
        refs[dz][dx] = chunk.as_ref();
    }
    assert!(
        refs[1][1].is_some(),
        "centre chunk must be present after edit"
    );

    let mut ws = LightWorkspace::new();
    let expected_light = compute_chunk_light_in(&mut ws, refs, &block_light);
    let expected_wire = encode_chunk_light(&expected_light);

    assert_eq!(
        pkt.light.sky_y_mask, expected_wire.sky_y_mask,
        "sky_y_mask mismatch",
    );
    assert_eq!(
        pkt.light.block_y_mask, expected_wire.block_y_mask,
        "block_y_mask mismatch",
    );
    assert_eq!(
        pkt.light.empty_sky_y_mask, expected_wire.empty_sky_y_mask,
        "empty_sky_y_mask mismatch",
    );
    assert_eq!(
        pkt.light.empty_block_y_mask, expected_wire.empty_block_y_mask,
        "empty_block_y_mask mismatch",
    );
    assert_eq!(
        pkt.light.sky_updates, expected_wire.sky_updates,
        "sky_updates payload mismatch",
    );
    assert_eq!(
        pkt.light.block_updates, expected_wire.block_updates,
        "block_updates payload mismatch",
    );
}
