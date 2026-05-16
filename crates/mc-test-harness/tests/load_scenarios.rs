//! M37 load-oriented scenarios.
//!
//! These tests are ignored by default because they are timing/report harnesses,
//! not deterministic correctness gates. Run with:
//!
//! ```text
//! cargo test -p mc-test-harness --test load_scenarios -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    AddEntity, BlockChangedAck, ClientboundKeepAlive, ConfirmTeleportation, Direction, GameEvent,
    InteractionHand, LevelChunkWithLight, LoginPlay, MovePlayerFlags, ServerboundChatCommand,
    ServerboundKeepAlive, ServerboundMovePlayerPos, ServerboundUseItemOn, SetCenterChunk,
    SynchronizePlayerPosition, pack_block_pos,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 1;

#[tokio::test]
#[ignore = "M37 load report; run explicitly with --ignored --nocapture"]
async fn reports_spawn_exploration_block_entity_and_multi_client_load() {
    let Some(server) = start_load_server().await else {
        return;
    };
    let addr = server.addr;

    let started = Instant::now();
    let mut clients = Vec::new();
    for idx in 0..4 {
        let (mut client, sync) = connect_to_play(addr, &format!("M37Load{idx}")).await;
        drain_until_chunk(&mut client, (0, 0)).await;
        clients.push((client, sync));
    }
    eprintln!(
        "M37 load spawn_login_multi_client clients={} elapsed_ms={}",
        clients.len(),
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    let (client, sync) = clients.get_mut(0).expect("first client");
    for step in 1..=8 {
        client
            .write_packet(&ServerboundMovePlayerPos {
                x: 16.5 * f64::from(step),
                y: sync.y,
                z: 0.5,
                flags: MovePlayerFlags::new(true, false),
            })
            .await
            .expect("send exploration move");
    }
    drain_until_chunk(client, (8, 0)).await;
    eprintln!(
        "M37 load exploration moves=8 elapsed_ms={}",
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    client
        .write_packet(&ServerboundChatCommand {
            command: "gamemode creative".to_string(),
        })
        .await
        .expect("creative command");
    client
        .write_packet(&ServerboundChatCommand {
            command: "give minecraft:dirt 64".to_string(),
        })
        .await
        .expect("give dirt command");
    let base_y = sync.y.floor() as i32 - 1;
    let storm_x = 16 * 8;
    for sequence in 1..=16 {
        client
            .write_packet(&ServerboundMovePlayerPos {
                x: f64::from(storm_x + sequence) + 0.5,
                y: sync.y,
                z: 0.5,
                flags: MovePlayerFlags::new(true, false),
            })
            .await
            .expect("move for block storm");
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(storm_x + sequence, base_y, 0),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence,
            })
            .await
            .expect("place dirt");
    }
    let acks = drain_counting(client, Duration::from_secs(3), BlockChangedAck::ID).await;
    eprintln!(
        "M37 load block_edit_storm attempts=16 acks={} elapsed_ms={}",
        acks,
        started.elapsed().as_millis()
    );

    let started = Instant::now();
    for idx in 0..8 {
        client
            .write_packet(&ServerboundChatCommand {
                command: format!(
                    "summon minecraft:zombie {} {} {}",
                    storm_x + idx,
                    base_y + 1,
                    3
                ),
            })
            .await
            .expect("summon zombie");
    }
    let spawns = drain_counting(client, Duration::from_secs(5), AddEntity::ID).await;
    eprintln!(
        "M37 load entity_crowd summons=8 add_entity_frames={} elapsed_ms={}",
        spawns,
        started.elapsed().as_millis()
    );
}

struct LoadServer {
    addr: std::net::SocketAddr,
}

async fn start_load_server() -> Option<LoadServer> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    let registries_json = vanilla_dir.join("reports/registries.json");
    if !blocks_json.exists() || !registries_json.exists() {
        eprintln!(
            "skipping M37 load scenarios: missing {} or {}",
            blocks_json.display(),
            registries_json.display()
        );
        return None;
    }

    let data = Arc::new(mc_data::load(&vanilla_dir).expect("vanilla data loads"));
    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report loads");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry"));
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(0, Arc::clone(&blocks)));
    let storage = mc_world::WorldStorage::in_memory_with_capacity(
        Arc::clone(&blocks),
        ((2 * VIEW_DISTANCE + 5) as usize).pow(2),
    )
    .with_generator(generator);
    let world = Some(Arc::new(tokio::sync::Mutex::new(storage)));
    let tags = Arc::new(mc_data::tags::load(&vanilla_dir, &data).expect("tags load"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .ok()
        .map(Arc::new);
    let items_report = mc_data::items::load_items_report(&registries_json).expect("items report");
    let items = Arc::new(mc_data::items::ItemRegistry::from_report(&items_report));
    let entity_report =
        mc_data::entity_types::load_entity_types_report(&registries_json).expect("entity report");
    let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(
        &entity_report,
    ));
    let biome_spawns =
        mc_data::biomes::load_biome_spawn_rules(vanilla_dir.join("data/minecraft/worldgen/biome"))
            .map(Arc::new)
            .unwrap_or_default();

    let cfg = mc_net::ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "M37 load scenarios".into(),
        max_players: 8,
        view_distance: VIEW_DISTANCE,
        data,
        blocks,
        world,
        tags,
        recipes: Arc::new(Vec::new()),
        loot: Arc::new(mc_data::loot::LootTables::default()),
        block_light,
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns,
        chunk_pipeline: mc_net::ChunkPipelinePolicy {
            chunk_prepare_batch_size: 2,
            chunk_result_queue_size: 8,
            ..mc_net::ChunkPipelinePolicy::default()
        },
        random_tick: mc_net::RandomTickPolicy {
            random_tick_speed: 3,
            chunk_budget: 8,
            fluid_tick_budget: 64,
            save_interval_ticks: 20,
            seed: 0,
        },
        command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), true),
        shutdown: mc_net::ShutdownHandle::default(),
    };
    let bound = mc_net::bind(cfg).await.expect("bind");
    let addr = bound.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = bound.serve().await;
    });
    Some(LoadServer { addr })
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
    let _: mc_protocol::packets::play::ClientboundCommands =
        client.read_typed().await.expect("Commands");
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
            .expect("drain chunk");
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

async fn drain_counting(client: &mut Client, duration: Duration, packet_id: i32) -> usize {
    let deadline = tokio::time::Instant::now() + duration;
    let mut count = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return count;
        }
        let Ok(frame) = client.read_frame_with_timeout(remaining).await else {
            return count;
        };
        if handle_keepalive(client, frame.id, &frame.body).await {
            continue;
        }
        if frame.id == packet_id {
            count += 1;
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
