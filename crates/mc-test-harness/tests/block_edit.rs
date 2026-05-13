//! M5.f — raw-TCP integration test for the break-block flow.
//!
//! Boots `mc_net::run` on an ephemeral port against
//! `.analysis/test-world/`, walks the spawn burst, sends a
//! `ServerboundPlayerAction(START_DESTROY_BLOCK)` at the grass
//! cell directly under spawn `(0, -61, 0)`, and asserts that:
//!
//! - a `ClientboundBlockUpdate` for that position with the air
//!   state-id arrives,
//! - a `ClientboundBlockChangedAck` echoes our sequence,
//! - at least one `ClientboundLightUpdate` for chunk `(0, 0)`
//!   arrives with a well-shaped `LightData` payload (same M4.f
//!   mask invariants).
//!
//! Skipped silently when the test-world or required vanilla data
//! sidecars are missing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundKeepAlive, ConfirmTeleportation, Direction, GameEvent,
    LevelChunkWithLight, LightUpdate, LoginPlay, PlayerActionKind, ServerboundKeepAlive,
    ServerboundPlayerAction, SetCenterChunk, SynchronizePlayerPosition, pack_block_pos,
    unpack_block_pos,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn break_block_round_trips_update_ack_relight() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_dir = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !world_dir.exists() || !blocks_json.exists() {
        eprintln!(
            "skipping: {} or {} missing",
            world_dir.display(),
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::open_with_capacity(
        &world_dir,
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("world storage opens")
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));

    // The relight path needs the block-light table. Skip the test
    // if it isn't present — the same posture as the M4.f gate.
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!("skipping: {} ({err})", block_light_path.display(),);
            return;
        }
    };

    // Resolve the air state-id and the grass cell we expect to
    // break so the assertions don't hard-code 26.1.2 numerics.
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("air in registry");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M5.f block edit".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
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
        .drive_login(addr, "M5fTester")
        .await
        .expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");

    // Spawn burst.
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

    // Wait only for the spawn chunk, then send the edit while the rest
    // of the view-distance window is still streaming. This is the M12
    // responsiveness gate: inbound edits must not sit behind all 441
    // chunks.
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let burst_deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !chunks_seen.contains(&(0, 0)) {
        let remaining = burst_deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "spawn chunk stalled after {} chunks: {e}",
                    chunks_seen.len()
                )
            });
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
        // Stray packets between chunks (keepalive, etc.) ignored.
    }

    // Send the break action against the top block under spawn. For the
    // old flat oracle this is Y=-61; for Solaris-generated worlds the
    // Play position is adaptive (`top + 2`). Sequence
    // = 1 — fresh per-connection counter from the client side.
    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    let sequence: i32 = 1;
    client
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence,
        })
        .await
        .expect("send break action");

    // Collect the resulting wire response. The handler emits:
    //   BlockUpdate(pos, air) → LightUpdate × 5 → BlockChangedAck.
    // We allow stray frames (e.g. keepalive) interleaved.
    let mut saw_block_update = false;
    let mut saw_ack = false;
    let mut saw_light_for_origin = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(saw_block_update && saw_ack && saw_light_for_origin) {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "edit response stalled: block_update={saw_block_update}, ack={saw_ack}, \
                     light_for_origin={saw_light_for_origin}: {e}"
                )
            });
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!(
                (px, py, pz),
                (0, target_y, 0),
                "BlockUpdate position must match the broken cell",
            );
            assert_eq!(
                pkt.state_id, air_state_id,
                "BlockUpdate state must be air after break",
            );
            saw_block_update = true;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode BlockChangedAck");
            assert_eq!(
                pkt.sequence, sequence,
                "ack must echo the action's sequence"
            );
            saw_ack = true;
        } else if frame.id == LightUpdate::ID {
            let mut body = frame.body;
            let pkt = LightUpdate::decode(&mut body).expect("decode LightUpdate");
            // M4.f-style mask invariants — at least the origin chunk
            // must arrive lit-and-shaped.
            const ALL_26: u64 = (1 << 26) - 1;
            let sky_mask = mask_to_u64(&pkt.light.sky_y_mask);
            let empty_sky_mask = mask_to_u64(&pkt.light.empty_sky_y_mask);
            assert_eq!(
                sky_mask | empty_sky_mask,
                ALL_26,
                "LightUpdate sky present+empty must cover all 26 slots for chunk ({}, {})",
                pkt.chunk_x,
                pkt.chunk_z,
            );
            assert_eq!(
                pkt.light.sky_updates.len(),
                sky_mask.count_ones() as usize,
                "LightUpdate sky_updates count must match popcount",
            );
            if (pkt.chunk_x, pkt.chunk_z) == (0, 0) {
                saw_light_for_origin = true;
            }
        }
        // Ignore stray frames (keepalive, etc.) that might land in
        // between the edit response packets.
    }
}

#[tokio::test]
async fn break_block_broadcasts_update_to_second_subscriber() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let world_dir = manifest.join("../../.analysis/test-world");
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !world_dir.exists() || !blocks_json.exists() {
        eprintln!(
            "skipping: {} or {} missing",
            world_dir.display(),
            blocks_json.display()
        );
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::open_with_capacity(
        &world_dir,
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .expect("world storage opens")
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => Some(Arc::new(table)),
        Err(err) => {
            eprintln!("skipping: {} ({err})", block_light_path.display(),);
            return;
        }
    };
    let air_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .map(|b| b.default.0 as i32)
        .expect("air in registry");

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M15 two-client block edit".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
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

    let (mut actor, sync) = connect_to_play(addr, "M15Actor").await;
    let (mut observer, _) = connect_to_play(addr, "M15Observer").await;
    drain_until_chunk(&mut actor, (0, 0)).await;
    drain_until_chunk(&mut observer, (0, 0)).await;

    let target_y = sync.y.floor() as i32 - 2;
    let target_pos = pack_block_pos(0, target_y, 0);
    let sequence: i32 = 15;
    actor
        .write_packet(&ServerboundPlayerAction {
            action: PlayerActionKind::StartDestroyBlock,
            position: target_pos,
            direction: Direction::Up,
            sequence,
        })
        .await
        .expect("send break action");

    let mut actor_saw_ack = false;
    let mut actor_saw_update = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !(actor_saw_ack && actor_saw_update) {
        let frame = actor
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("actor edit response");
        if handle_keepalive(&mut actor, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode actor BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!((px, py, pz), (0, target_y, 0));
            assert_eq!(pkt.state_id, air_state_id);
            actor_saw_update = true;
        } else if frame.id == BlockChangedAck::ID {
            let mut body = frame.body;
            let pkt = BlockChangedAck::decode(&mut body).expect("decode actor ack");
            assert_eq!(pkt.sequence, sequence);
            actor_saw_ack = true;
        }
    }

    let mut observer_saw_update = false;
    let mut observer_saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !observer_saw_update {
        let frame = observer
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("observer edit response");
        if handle_keepalive(&mut observer, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let pkt = BlockUpdate::decode(&mut body).expect("decode observer BlockUpdate");
            let (px, py, pz) = unpack_block_pos(pkt.position);
            assert_eq!((px, py, pz), (0, target_y, 0));
            assert_eq!(pkt.state_id, air_state_id);
            observer_saw_update = true;
        } else if frame.id == BlockChangedAck::ID {
            observer_saw_ack = true;
        }
    }
    assert!(!observer_saw_ack, "observer must not receive actor ack");
}

async fn connect_to_play(
    addr: std::net::SocketAddr,
    name: &str,
) -> (Client, SynchronizePlayerPosition) {
    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client.drive_login(addr, name).await.expect("drive login");
    client
        .drive_configuration()
        .await
        .expect("drive configuration");
    let _: LoginPlay = client.read_typed().await.expect("LoginPlay");
    let sync: SynchronizePlayerPosition = client.read_typed().await.expect("SyncPlayerPos");
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

async fn drain_until_chunk(client: &mut Client, target: (i32, i32)) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("drain chunks");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode chunk");
            if (pkt.chunk_x, pkt.chunk_z) == target {
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
    let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
    client
        .write_packet(&ServerboundKeepAlive { id: keepalive.id })
        .await
        .expect("echo KeepAlive");
    true
}

fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
