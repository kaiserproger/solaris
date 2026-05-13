//! M17 - server-owned vanilla entity visibility baseline.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, ClientboundKeepAlive, ConfirmTeleportation, GameEvent, LevelChunkWithLight,
    LoginPlay, MoveEntityPosRot, ServerboundKeepAlive, SetCenterChunk, SetEntityMotion,
    SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 2;
const COW_ENTITY_TYPE_ID: i32 = 30;

#[tokio::test]
async fn vanilla_client_receives_server_owned_cow_and_motion() {
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
    let storage = match mc_world::WorldStorage::open_with_capacity(
        &world_dir,
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 3) as usize).pow(2),
    ) {
        Ok(storage) => storage.with_generator(generator),
        Err(err) => {
            eprintln!("skipping: {} ({err})", world_dir.display());
            return;
        }
    };
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light_path = vanilla_dir.join("reports/block_light.json");
    let block_light = mc_data::block_light::load(&block_light_path)
        .ok()
        .map(Arc::new);

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M17 mob presence".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        block_light,
        items: Arc::new(mc_data::items::ItemRegistry::default()),
        chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });

    let (mut client, _) = connect_to_play(addr, "M17MobProbe").await;
    drain_until_chunk(&mut client, (0, 0)).await;
    let cow = wait_for_cow_spawn(&mut client).await;
    wait_for_cow_motion_after_spawn(&mut client, cow.entity_id, cow.x, cow.z).await;
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

async fn wait_for_cow_spawn(client: &mut Client) -> AddEntity {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for cow spawn");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == AddEntity::ID {
            let mut body = frame.body;
            let pkt = AddEntity::decode(&mut body).expect("decode AddEntity");
            if pkt.entity_type_id == COW_ENTITY_TYPE_ID {
                return pkt;
            }
        }
    }
}

async fn wait_for_cow_motion_after_spawn(
    client: &mut Client,
    entity_id: i32,
    _spawn_x: f64,
    _spawn_z: f64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let frame = client
            .read_frame_with_timeout(
                deadline.saturating_duration_since(tokio::time::Instant::now()),
            )
            .await
            .expect("wait for cow motion");
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == MoveEntityPosRot::ID {
            let mut body = frame.body;
            let pkt = MoveEntityPosRot::decode(&mut body).expect("decode MoveEntityPosRot");
            if pkt.entity_id == entity_id
                && (pkt.delta_x != 0 || pkt.delta_y != 0 || pkt.delta_z != 0)
            {
                return;
            }
        } else if frame.id == SetEntityMotion::ID {
            let mut body = frame.body;
            let pkt = SetEntityMotion::decode(&mut body).expect("decode SetEntityMotion");
            assert_eq!(pkt.entity_id, entity_id);
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
