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
    BlockChangedAck, BlockUpdate, ConfirmTeleportation, Direction, GameEvent, LevelChunkWithLight,
    LightUpdate, LoginPlay, PlayerActionKind, ServerboundPlayerAction, SetCenterChunk,
    SynchronizePlayerPosition, pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;

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
    let storage =
        mc_world::WorldStorage::open(&world_dir, Arc::clone(&blocks)).expect("world storage opens");
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
        data,
        blocks,
        world,
        tags,
        block_light,
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

    // Drain the 441 chunk packets that follow. We don't validate
    // them here — M4.f already does. We just need to consume them
    // before the break flow's packets start arriving.
    let expected_chunks = (2 * 10 + 1u32).pow(2) as usize;
    let mut chunks_seen: HashSet<(i32, i32)> = HashSet::new();
    let burst_deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while chunks_seen.len() < expected_chunks {
        let remaining = burst_deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| panic!("burst stalled after {} chunks: {e}", chunks_seen.len()));
        if frame.id == LevelChunkWithLight::ID {
            let mut body = frame.body;
            let pkt = LevelChunkWithLight::decode(&mut body).expect("decode");
            chunks_seen.insert((pkt.chunk_x, pkt.chunk_z));
        }
        // Stray packets between chunks (keepalive, etc.) ignored.
    }

    // Send the break action: grass at world (0, -61, 0). Sequence
    // = 1 — fresh per-connection counter from the client side.
    let target_pos = pack_block_pos(0, -61, 0);
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
                (0, -61, 0),
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

fn mask_to_u64(longs: &[i64]) -> u64 {
    longs.first().copied().unwrap_or(0) as u64
}
