//! M9.d — wire parity gate for the incremental relight engine.
//!
//! Boots `mc_net::run` on an ephemeral port against an in-memory
//! generated world, drives the spawn burst, places a
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
//! Skipped silently when required vanilla data sidecars are missing.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    BlockChangedAck, BlockUpdate, ClientboundKeepAlive, ConfirmTeleportation, Direction, GameEvent,
    LevelChunkWithLight, LightUpdate, ServerboundChatCommand, ServerboundKeepAlive,
    ServerboundSetCarriedItem, ServerboundUseItemOn, SetCenterChunk, SynchronizePlayerPosition,
    pack_block_pos, unpack_block_pos,
};
use mc_test_harness::client::Client;
use mc_world::light::{LightWorkspace, compute_chunk_light_in};
use mc_world::wire::encode_chunk_light;
use mc_world::{Chunk, ChunkPos};

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
#[ignore = "requires local data/vanilla sidecars"]
async fn incremental_relight_wire_matches_full_recompute() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    if !blocks_json.exists() || !registries_json.exists() || !block_light_path.exists() {
        panic!(
            "prerequisite failed: prerequisites missing under {}",
            vanilla_dir.display()
        );
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));
    let world = Some(Arc::clone(&world_handle));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light =
        Arc::new(mc_data::block_light::load(&block_light_path).expect("block_light loads"));

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M9.d incremental relight".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks: Arc::clone(&blocks),
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light: Some(Arc::clone(&block_light)),
        items: Arc::clone(&items),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
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

    let mut client = Client::connect(addr).await.expect("client connect");
    let _ = client
        .drive_login(addr, "M9dTester")
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
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".into(),
        })
        .await
        .expect("switch to creative");
    client
        .write_packet(&ServerboundChatCommand {
            command: "debug give minecraft:dirt 64 1".into(),
        })
        .await
        .expect("seed dirt slot");

    // Drain the configured spawn burst before editing so light-cache state is stable.
    let expected_chunks = (2 * VIEW_DISTANCE + 1).pow(2) as usize;
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

    // Select dirt (hotbar slot 1) and place one block above the known
    // skylit mid-chunk column used by this parity fixture.
    let target_y = {
        let guard = world_handle.lock().await;
        let mut chunk = guard
            .cached_chunk(ChunkPos { x: 0, z: 0 })
            .expect("origin chunk present");
        chunk.rebuild_highest_opaque(&block_light);
        chunk
            .highest_opaque_y(8, 8)
            .expect("origin local column has terrain")
    };
    client
        .write_packet(&ServerboundChatCommand {
            command: format!("tp 8.5 {} 8.5", target_y + 2),
        })
        .await
        .expect("teleport beside relight fixture");
    let teleported = loop {
        let frame = client
            .read_frame_with_timeout(Duration::from_secs(30))
            .await
            .expect("relight fixture teleport");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
        } else if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            break SynchronizePlayerPosition::decode(&mut body).expect("decode fixture teleport");
        }
    };
    client
        .write_packet(&ConfirmTeleportation {
            teleport_id: teleported.teleport_id,
        })
        .await
        .expect("confirm relight fixture teleport");
    let dx = teleported.x - 8.5;
    let dy = teleported.y + 1.62 - (f64::from(target_y) + 0.5);
    let dz = teleported.z - 8.5;
    assert!(
        dx * dx + dy * dy + dz * dz <= 36.0,
        "relight fixture must be within creative block reach"
    );
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
    let mut light_updates: Vec<LightUpdate> = Vec::new();
    let mut saw_block_update = false;
    let mut block_updates = Vec::new();
    let mut saw_ack = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !saw_ack {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .unwrap_or_else(|e| panic!("place response stalled: {e}"));
        if frame.id == BlockUpdate::ID {
            let mut body = frame.body;
            let packet = BlockUpdate::decode(&mut body).expect("decode BlockUpdate");
            let position = unpack_block_pos(packet.position);
            block_updates.push((position, packet.state_id));
            saw_block_update |= position == (8, target_y + 1, 8);
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
    assert!(
        saw_block_update,
        "placed-block update must arrive before ack; updates={block_updates:?}"
    );

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
