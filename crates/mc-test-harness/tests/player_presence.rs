//! M16 - two-client player presence and movement visibility.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundKeepAlive, ConfirmTeleportation, EntityPositionSync, GameEvent,
    LevelChunkWithLight, LoginPlay, MovePlayerFlags, PlayerInfoRemove, PlayerInfoUpdate,
    RemoveEntities, ServerboundKeepAlive, ServerboundMovePlayerPosRot, SetCenterChunk,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;
use uuid::Uuid;

const VIEW_DISTANCE: i32 = 2;

#[tokio::test]
async fn two_clients_spawn_move_and_despawn_visible_players() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks =
        Arc::new(mc_world::BlockRegistry::from_report(&report).expect("block registry builds"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M16 player presence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        entity_types: Arc::new(mc_data::entity_types::EntityTypeRegistry::default()),
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut alice, _) = connect_to_play(addr, "M16Alice").await;
    drain_until_chunk(&mut alice, (0, 0)).await;

    let (mut bob, bob_sync) = connect_to_play(addr, "M16Bob").await;
    let bob_entity = wait_for_player_spawn(&mut alice, "M16Bob").await;
    let alice_entity = wait_for_player_spawn(&mut bob, "M16Alice").await;
    assert_ne!(alice_entity, bob_entity, "remote entity ids must be unique");

    bob.write_packet(&ServerboundMovePlayerPosRot {
        x: 2.5,
        y: bob_sync.y,
        z: 0.5,
        yaw: 90.0,
        pitch: 0.0,
        flags: MovePlayerFlags::new(true, false),
    })
    .await
    .expect("send Bob movement");
    wait_for_position(&mut alice, bob_entity, 2.5, bob_sync.y, 0.5).await;

    drop(bob);
    wait_for_player_remove(&mut alice, bob_entity).await;
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

async fn wait_for_player_spawn(client: &mut Client, expected_name: &str) -> i32 {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut expected_uuid: Option<Uuid> = None;
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for player spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == PlayerInfoUpdate::ID {
            let mut body = frame.body;
            let pkt = PlayerInfoUpdate::decode(&mut body).expect("decode PlayerInfoUpdate");
            for entry in pkt.entries {
                if entry.name == expected_name {
                    expected_uuid = Some(entry.profile_id);
                }
            }
        } else if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            if expected_uuid == Some(pkt.uuid) {
                return pkt.entity_id;
            }
        }
    }
}

async fn wait_for_position(client: &mut Client, entity_id: i32, x: f64, y: f64, z: f64) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for player movement");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == EntityPositionSync::ID {
            let mut body = frame.body;
            let pkt = EntityPositionSync::decode(&mut body).expect("decode EntityPositionSync");
            if pkt.entity_id == entity_id
                && (pkt.values.position.x - x).abs() < f64::EPSILON
                && (pkt.values.position.y - y).abs() < f64::EPSILON
                && (pkt.values.position.z - z).abs() < f64::EPSILON
            {
                return;
            }
        }
    }
}

async fn wait_for_player_remove(client: &mut Client, entity_id: i32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut saw_entity_remove = false;
    let mut saw_info_remove = false;
    while !(saw_entity_remove && saw_info_remove) {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for player remove");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == RemoveEntities::ID {
            let mut body = frame.body;
            let pkt = RemoveEntities::decode(&mut body).expect("decode RemoveEntities");
            saw_entity_remove |= pkt.entity_ids.contains(&entity_id);
        } else if frame.id == PlayerInfoRemove::ID {
            let mut body = frame.body;
            let pkt = PlayerInfoRemove::decode(&mut body).expect("decode PlayerInfoRemove");
            saw_info_remove |= !pkt.profile_ids.is_empty();
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
