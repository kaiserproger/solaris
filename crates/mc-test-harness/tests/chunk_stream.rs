//! M3.g / M4.f — raw-TCP integration test for the chunk-streaming
//! spawn burst.
//!
//! Boots `mc_net::run` on an ephemeral port with a real
//! `WorldStorage` opened on `.analysis/test-world/`, walks
//! Handshake → Login → Configuration → Play exactly as a vanilla
//! client does, and asserts that the M3.e view-distance ring around
//! spawn arrives as `LevelChunkWithLight` packets — each with the
//! expected coordinates, non-empty client-usage heightmaps, and a
//! non-empty chunk-data blob.
//!
//! M4.f extends the assertions to the bundled `LightData` payload:
//! when `block_light.json` is present, every chunk's sky / block
//! masks must cover all 26 wire slots between the present and empty
//! channels, the layer counts must match the mask popcount, and the
//! spawn chunk's above-world slot must ship 0xFF nibbles (open sky).
//!
//! Skipped silently when the test-world or `blocks.json` is missing,
//! matching the M2 round-trip oracle's pattern.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LevelChunkWithLight, LoginPlay,
    MovePlayerFlags, ServerboundKeepAlive, ServerboundMovePlayerPos, SetCenterChunk,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

/// Matches `SPAWN_VIEW_DISTANCE` in `mc_net::play`. Hard-coded here on
/// purpose: a regression that quietly raises the constant should fail
/// this test by overshooting the bound, not silently pass.
const VIEW_DISTANCE: i32 = 10;

#[tokio::test]
async fn vanilla_client_receives_spawn_view_distance_window() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_dir = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !world_dir.exists() || !blocks_json.exists() {
        eprintln!(
            "skipping: {} or {} missing — run tools/generate-test-world.sh \
             and tools/extract-vanilla-data.sh --reports",
            world_dir.display(),
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let storage =
        mc_world::WorldStorage::open(&world_dir, Arc::clone(&blocks)).expect("world storage opens");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    // block_light.json is optional — when missing, the M4.f assertions
    // are skipped but the M3.g shape assertions still run. This keeps
    // CI green on a fresh checkout that hasn't run extract-block-light.sh.
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!(
                "M4.f light assertions skipped: {} ({err})",
                block_light_path.display()
            );
            None
        }
    };
    let assert_light = block_light.is_some();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M3.g chunk stream".into(),
        max_players: 8,
        data,
        blocks,
        world,
        tags,
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M3gTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    // Spawn burst, in the order `mc_net::play::handle` emits it.
    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let event: GameEvent = client.read_typed().await.expect("GameEvent");
    assert_eq!(event.event, GameEvent::EVENT_START_WAITING_FOR_CHUNKS);
    let center: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    assert_eq!(
        (center.chunk_x, center.chunk_z),
        (0, 0),
        "spawn (0.5, 0.5) anchors at chunk (0, 0)"
    );

    // Ack the teleport so the server's keepalive scheduler doesn't see
    // a desync warning. The chunk stream is independent of this ack —
    // the M3.d/M3.e path emits chunks before the keepalive loop — but
    // doing it keeps the test honest about the M1.g handshake.
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    // 24 sections × minimum 8 bytes (i16 non_air + i16 fluid + u8 bpe +
    // VarInt(0) block id + u8 bpe + VarInt biome) = 192. Any chunk
    // emitted by `encode_chunk_data` must clear this floor regardless of
    // whether vanilla's generator filled the chunk in or wrote it as a
    // Status: empty placeholder.
    const MIN_CHUNK_DATA_BYTES: usize = 24 * 8;

    let expected_count = (2 * VIEW_DISTANCE + 1).pow(2) as usize; // 21×21 = 441
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    // Cap per-chunk client-usage heightmaps so a partially-generated
    // test-world chunk (Status: empty, no Heightmaps NBT) doesn't fail
    // the test. The spawn chunk (0,0) is always fully-generated; we
    // assert its heightmaps explicitly after the loop.
    let mut chunks_with_heightmaps = 0usize;
    let mut spawn_heightmaps_seen = 0usize;
    // 180 s gives the M4.f light-bearing burst comfortable margin in
    // debug builds; the M3.g shape-only burst finishes in ~5 s, M4.f's
    // BFS-per-chunk run brings that up to ~30-40 s in isolation and
    // ~50-70 s when other workspace tests run in parallel.
    let timeout = Duration::from_secs(180);
    let deadline = tokio::time::Instant::now() + timeout;
    while seen.len() < expected_count {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match client.read_frame_with_timeout(remaining).await {
            Ok(f) => f,
            Err(err) => panic!(
                "expected {} chunks within {:?}, only saw {}: {err}",
                expected_count,
                timeout,
                seen.len(),
            ),
        };
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        assert!(
            (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_x)
                && (-VIEW_DISTANCE..=VIEW_DISTANCE).contains(&pkt.chunk_z),
            "chunk ({}, {}) outside the expected ring ±{VIEW_DISTANCE}",
            pkt.chunk_x,
            pkt.chunk_z
        );
        assert!(
            pkt.data.len() >= MIN_CHUNK_DATA_BYTES,
            "chunk ({}, {}) payload {} bytes < {MIN_CHUNK_DATA_BYTES} byte floor",
            pkt.chunk_x,
            pkt.chunk_z,
            pkt.data.len()
        );
        for hm in &pkt.heightmaps {
            assert!(
                !hm.data.is_empty(),
                "heightmap type_id={} long[] must be populated for chunk ({}, {})",
                hm.type_id,
                pkt.chunk_x,
                pkt.chunk_z
            );
        }
        if !pkt.heightmaps.is_empty() {
            chunks_with_heightmaps += 1;
        }
        if (pkt.chunk_x, pkt.chunk_z) == (0, 0) {
            spawn_heightmaps_seen = pkt.heightmaps.len();
        }
        if assert_light {
            assert_light_invariants(&pkt);
        }
        let fresh = seen.insert((pkt.chunk_x, pkt.chunk_z));
        assert!(
            fresh,
            "duplicate chunk ({}, {}) on the wire",
            pkt.chunk_x, pkt.chunk_z
        );
    }

    assert_eq!(
        seen.len(),
        expected_count,
        "every chunk in the spawn view-distance ring must be on the wire"
    );

    // Lattice spot-check: the (cx, cz) set is exactly [-VD..=VD]^2.
    for cz in -VIEW_DISTANCE..=VIEW_DISTANCE {
        for cx in -VIEW_DISTANCE..=VIEW_DISTANCE {
            assert!(
                seen.contains(&(cx, cz)),
                "missing chunk ({cx}, {cz}) from the view-distance ring"
            );
        }
    }

    // The test-world generator (tools/generate-test-world.sh) boots the
    // bundled server at view-distance=2, so vanilla fully populates a
    // small core around spawn and leaves the rest of the .mca slots as
    // Status: empty placeholders without Heightmaps NBT — chunks we
    // serve faithfully but with the heightmap list empty (vanilla
    // accepts the subset). The spawn chunk is always fully generated;
    // the rest of the ring must have at least one populated chunk to
    // prove the encoder is actually emitting heightmaps end-to-end.
    assert!(
        spawn_heightmaps_seen > 0,
        "spawn chunk (0, 0) must carry client-usage heightmaps"
    );
    assert!(
        chunks_with_heightmaps >= 1,
        "no chunk in the ring carried client-usage heightmaps — \
         encode_chunk_data is probably dropping them"
    );
}

#[tokio::test]
async fn movement_across_chunk_boundary_replans_view_subscription() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_dir = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !world_dir.exists() || !blocks_json.exists() {
        eprintln!(
            "skipping: {} or {} missing — run tools/generate-test-world.sh \
             and tools/extract-vanilla-data.sh --reports",
            world_dir.display(),
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let storage =
        mc_world::WorldStorage::open(&world_dir, Arc::clone(&blocks)).expect("world storage opens");
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);
    let policy = mc_net::ChunkPipelinePolicy {
        chunk_prepare_batch_size: 1,
        chunk_result_queue_size: 1,
        ..mc_net::ChunkPipelinePolicy::default()
    };

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M14 movement chunk stream".into(),
        max_players: 8,
        data,
        blocks,
        world,
        tags,
        block_light,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        chunk_pipeline: policy,
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M14MoveTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
    let _: GameEvent = client.read_typed().await.expect("GameEvent");
    let center: SetCenterChunk = client.read_typed().await.expect("SetCenterChunk");
    assert_eq!((center.chunk_x, center.chunk_z), (0, 0));
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: sync.teleport_id,
        })
        .await
        .expect("ack teleport");

    client
        .write_packet(&ServerboundMovePlayerPos {
            x: 16.5,
            y: sync.y,
            z: 0.5,
            flags: MovePlayerFlags::new(true, false),
        })
        .await
        .expect("send movement");

    let timeout = Duration::from_secs(180);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut saw_new_center = false;
    let mut saw_new_strip_chunk = false;
    while !saw_new_strip_chunk {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("movement replan frames");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == SetCenterChunk::ID {
            let mut body = frame.body;
            let center = SetCenterChunk::decode(&mut body).expect("decode SetCenterChunk");
            if (center.chunk_x, center.chunk_z) == (1, 0) {
                saw_new_center = true;
            }
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        if (pkt.chunk_x, pkt.chunk_z) == (VIEW_DISTANCE + 1, 0) {
            saw_new_strip_chunk = true;
        }
    }

    assert!(saw_new_center, "movement must send SetCenterChunk(1, 0)");
}

/// M4.f: every streamed chunk must carry a wire LightData whose
/// present + empty masks cover all 26 sections, with the present
/// layer count matching the mask popcount. The above-world slot
/// (bit 25) must always be present in the sky mask and carry an
/// all-0xFF nibble layer (open sky).
fn assert_light_invariants(pkt: &LevelChunkWithLight) {
    let light = &pkt.light;
    let sky_mask = mask_to_u64(&light.sky_y_mask);
    let empty_sky_mask = mask_to_u64(&light.empty_sky_y_mask);
    let block_mask = mask_to_u64(&light.block_y_mask);
    let empty_block_mask = mask_to_u64(&light.empty_block_y_mask);

    const ALL_26: u64 = (1 << 26) - 1;
    assert_eq!(
        sky_mask | empty_sky_mask,
        ALL_26,
        "sky present+empty must cover slots 0..=25 for chunk ({}, {}); \
         got present={sky_mask:#X} empty={empty_sky_mask:#X}",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        sky_mask & empty_sky_mask,
        0,
        "sky present/empty masks must be disjoint for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        light.sky_updates.len(),
        sky_mask.count_ones() as usize,
        "sky_updates count must match popcount of sky mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    assert_eq!(
        block_mask | empty_block_mask,
        ALL_26,
        "block present+empty must cover slots 0..=25 for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        block_mask & empty_block_mask,
        0,
        "block present/empty masks must be disjoint for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    assert_eq!(
        light.block_updates.len(),
        block_mask.count_ones() as usize,
        "block_updates count must match popcount of block mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    // Slot 25 (Y=20, the above-world slab) is open sky — always
    // present in the sky channel as all-0xFF.
    assert!(
        sky_mask & (1 << 25) != 0,
        "slot 25 (above-world) must be present in sky mask for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );
    // The above-world layer is the *last* one we emit (M4.d emits in
    // ascending slot order, slot 25 last).
    let last_sky = light
        .sky_updates
        .last()
        .expect("sky_updates non-empty when slot 25 is present");
    assert!(
        last_sky.iter().all(|&b| b == 0xFF),
        "above-world sky slab must be all-0xFF nibbles for chunk ({}, {})",
        pkt.chunk_x,
        pkt.chunk_z,
    );

    // Every present layer must be exactly 2048 bytes (one section of
    // 4-bit nibbles).
    for (slot_idx, layer) in light.sky_updates.iter().enumerate() {
        assert_eq!(
            layer.len(),
            2048,
            "sky layer {slot_idx} has {} bytes (expected 2048) on chunk ({}, {})",
            layer.len(),
            pkt.chunk_x,
            pkt.chunk_z,
        );
    }
    for (slot_idx, layer) in light.block_updates.iter().enumerate() {
        assert_eq!(
            layer.len(),
            2048,
            "block layer {slot_idx} has {} bytes (expected 2048) on chunk ({}, {})",
            layer.len(),
            pkt.chunk_x,
            pkt.chunk_z,
        );
    }
}

fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
