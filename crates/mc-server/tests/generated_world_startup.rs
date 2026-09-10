//! Ignored disk-backed generated-world startup/stream gate for M100.
//!
//! This exercises the `mc-server` binary startup path rather than the in-process
//! `mc_net::bind` path: fresh world pre-generation, baked spawn light,
//! protocol readiness, 289-chunk view-distance-8 stream, process stop, restart, and the
//! warmed stream. It is ignored because it depends on local vanilla sidecars and
//! is intended to remain a performance blocker gate until the startup budget is
//! fixed.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, LevelChunkWithLight, ServerboundChatCommand,
    ServerboundKeepAlive, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;
use mc_world::ChunkGenerator;

const VIEW_DISTANCE: i32 = 8;
const EXPECTED_CHUNKS: usize = ((VIEW_DISTANCE * 2 + 1) * (VIEW_DISTANCE * 2 + 1)) as usize;
const EXPECTED_SPAWN_WINDOW_CHUNKS: usize =
    (((VIEW_DISTANCE + 1) * 2 + 1) * ((VIEW_DISTANCE + 1) * 2 + 1)) as usize;
const STARTUP_TO_PLAY_BUDGET: Duration = Duration::from_secs(10);
const CONSOLE_STOP_CLIENTS: usize = 4;
const CONSOLE_STOP_STREAM_CHUNKS: usize = 9;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "M100 generated-world startup budget gate; requires local data/vanilla sidecars"]
async fn disk_backed_generated_world_startup_stream_budget() {
    let vanilla_dir = vanilla_data_dir();
    assert_required_sidecars(&vanilla_dir);

    let temp = tempfile::tempdir().expect("tempdir");
    let world_dir = temp.path().join("world");
    let first_log = temp.path().join("first-server.log");
    let second_log = temp.path().join("second-server.log");

    let first_addr = loopback_addr_with_reserved_port();
    let first_config = temp.path().join("first.toml");
    write_server_config(&first_config, &world_dir, &vanilla_dir, first_addr.port());
    let first_started = Instant::now();
    let mut first = spawn_server(&first_config, &first_log);
    let mut first_client = connect_when_ready(first_addr, &mut first, &first_log).await;
    drive_to_play(&mut first_client, first_addr, "M100DiskA").await;
    let first_startup = first_started.elapsed();
    let first_stream = drain_view_distance_window(&mut first_client).await;
    wait_for_startup_dirty_checkpoint(&mut first, &first_log, &world_dir, &vanilla_dir).await;
    stop_server(&mut first_client, &mut first, &first_log).await;

    let second_addr = loopback_addr_with_reserved_port();
    let second_config = temp.path().join("second.toml");
    write_server_config(&second_config, &world_dir, &vanilla_dir, second_addr.port());
    let second_started = Instant::now();
    let mut second = spawn_server(&second_config, &second_log);
    let mut second_client = connect_when_ready(second_addr, &mut second, &second_log).await;
    drive_to_play(&mut second_client, second_addr, "M100DiskB").await;
    let second_startup = second_started.elapsed();
    let second_stream = drain_view_distance_window(&mut second_client).await;
    stop_server(&mut second_client, &mut second, &second_log).await;

    eprintln!(
        "M100 generated-world disk-backed startup: first_startup_ms={} first_full_ms={} \
         first_first_chunk_ms={} first_ring1_ms={:?} first_ring2_ms={:?} \
         second_startup_ms={} second_full_ms={} second_first_chunk_ms={} \
         second_ring1_ms={:?} second_ring2_ms={:?}",
        first_startup.as_millis(),
        first_stream.full_window_ms,
        first_stream.first_chunk_ms,
        first_stream.ring1_complete_ms,
        first_stream.ring2_complete_ms,
        second_startup.as_millis(),
        second_stream.full_window_ms,
        second_stream.first_chunk_ms,
        second_stream.ring1_complete_ms,
        second_stream.ring2_complete_ms,
    );

    assert!(
        first_startup <= STARTUP_TO_PLAY_BUDGET,
        "fresh generated-world startup-to-play exceeded budget: startup={first_startup:?} \
         budget={STARTUP_TO_PLAY_BUDGET:?}; first_stream={first_stream:?} \
         second_startup={second_startup:?} second_stream={second_stream:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "M100 startup dirty checkpoint regression gate; requires local data/vanilla sidecars"]
async fn disk_backed_generated_world_startup_checkpoint_survives_kill() {
    let vanilla_dir = vanilla_data_dir();
    assert_required_sidecars(&vanilla_dir);

    let temp = tempfile::tempdir().expect("tempdir");
    let world_dir = temp.path().join("world");
    let log = temp.path().join("server.log");
    let addr = loopback_addr_with_reserved_port();
    let config = temp.path().join("server.toml");
    write_server_config(&config, &world_dir, &vanilla_dir, addr.port());

    let mut server = spawn_server(&config, &log);
    let _client = connect_when_ready(addr, &mut server, &log).await;
    wait_for_startup_dirty_checkpoint(&mut server, &log, &world_dir, &vanilla_dir).await;
    kill_server_without_stop_and_assert_exit(&mut server, &log).await;
    assert_spawn_window_chunks_on_disk(&world_dir, &vanilla_dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "M100 existing-world missing-light startup budget gate; requires local data/vanilla sidecars"]
async fn disk_backed_existing_world_missing_light_startup_stream_budget() {
    let vanilla_dir = vanilla_data_dir();
    assert_required_sidecars(&vanilla_dir);

    let temp = tempfile::tempdir().expect("tempdir");
    let world_dir = temp.path().join("world");
    materialize_unbaked_spawn_window(&world_dir, &vanilla_dir);

    let log = temp.path().join("server.log");
    let addr = loopback_addr_with_reserved_port();
    let config = temp.path().join("server.toml");
    write_server_config(&config, &world_dir, &vanilla_dir, addr.port());

    let started = Instant::now();
    let mut server = spawn_server(&config, &log);
    let mut client = connect_when_ready(addr, &mut server, &log).await;
    drive_to_play(&mut client, addr, "M100Unbaked").await;
    let startup = started.elapsed();
    let stream = drain_view_distance_window(&mut client).await;
    wait_for_startup_dirty_checkpoint(&mut server, &log, &world_dir, &vanilla_dir).await;
    stop_server(&mut client, &mut server, &log).await;

    eprintln!(
        "M100 existing-world missing-light startup: startup_ms={} full_ms={} \
         first_chunk_ms={} ring1_ms={:?} ring2_ms={:?}",
        startup.as_millis(),
        stream.full_window_ms,
        stream.first_chunk_ms,
        stream.ring1_complete_ms,
        stream.ring2_complete_ms,
    );

    assert!(
        startup <= STARTUP_TO_PLAY_BUDGET,
        "existing generated-world missing-light startup-to-play exceeded budget: \
         startup={startup:?} budget={STARTUP_TO_PLAY_BUDGET:?}; stream={stream:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "M100 console-stop shutdown drain gate; requires local data/vanilla sidecars"]
async fn disk_backed_generated_world_console_stop_drains_stream_load() {
    let vanilla_dir = vanilla_data_dir();
    assert_required_sidecars(&vanilla_dir);

    let temp = tempfile::tempdir().expect("tempdir");
    let world_dir = temp.path().join("world");
    let log = temp.path().join("console-stop-server.log");
    let addr = loopback_addr_with_reserved_port();
    let config = temp.path().join("console-stop.toml");
    write_server_config_with_autoscale(&config, &world_dir, &vanilla_dir, addr.port(), true);

    let mut server = spawn_server_with_stdin(&config, &log);
    let first_client = connect_when_ready(addr, &mut server, &log).await;
    let mut client_tasks = Vec::new();
    client_tasks.push(tokio::spawn(async move {
        drive_to_play_and_drain_unique(first_client, addr, "M100ConsoleStop0").await
    }));
    for idx in 1..CONSOLE_STOP_CLIENTS {
        client_tasks.push(tokio::spawn(async move {
            let client = Client::connect(addr)
                .await
                .expect("connect console-stop client");
            drive_to_play_and_drain_unique(client, addr, &format!("M100ConsoleStop{idx}")).await
        }));
    }

    let mut clients = Vec::new();
    let mut streamed_chunks = HashSet::new();
    for task in client_tasks {
        let (client, chunks) = task.await.expect("console-stop client task joins");
        streamed_chunks.extend(chunks);
        clients.push(client);
    }

    write_console_stop(&mut server);
    drop(clients);
    wait_for_server_exit(&mut server, &log, Duration::from_secs(30)).await;
    assert_streamed_chunks_on_disk(&world_dir, &vanilla_dir, &streamed_chunks);

    eprintln!(
        "M100 console-stop disk-backed stream load: clients={} streamed_chunks={}",
        CONSOLE_STOP_CLIENTS,
        streamed_chunks.len(),
    );
}

fn vanilla_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

fn assert_required_sidecars(vanilla_dir: &Path) {
    for required in [
        "version.json",
        "reports/blocks.json",
        "reports/block_light.json",
        "reports/registries.json",
    ] {
        let path = vanilla_dir.join(required);
        assert!(
            path.exists(),
            "M100 generated-world startup gate requires {}; rerun vanilla extraction tools",
            path.display()
        );
    }
}

fn loopback_addr_with_reserved_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    listener.local_addr().expect("reserved local addr")
}

fn write_server_config(path: &Path, world_dir: &Path, vanilla_dir: &Path, port: u16) {
    write_server_config_with_autoscale(path, world_dir, vanilla_dir, port, false);
}

fn write_server_config_with_autoscale(
    path: &Path,
    world_dir: &Path,
    vanilla_dir: &Path,
    port: u16,
    autoscale_enabled: bool,
) {
    let toml = format!(
        r#"
[server]
name = "M100GeneratedWorld"
motd = "M100 generated-world startup gate"
view_distance = {VIEW_DISTANCE}

[network]
bind_address = "127.0.0.1"
port = {port}

[auth]
online_mode = false
whitelist_enabled = false
whitelist = []
banned_players = []

[admin]
operators = []
allow_local_dev_operators = true

[data]
world_dir = "{}"
vanilla_data_dir = "{}"
seed = 0

[simulation]
random_tick_speed = 0
save_interval_ticks = 1200

[chunk_pipeline]
chunk_send_rate = 8
chunk_load_rate = 16
chunk_generate_rate = 16
chunk_prepare_budget_ms = 0
chunk_prepare_batch_size = 8
chunk_result_queue_size = 64
region_cache_size = 9

[autoscale]
enabled = {autoscale_enabled}
profile = "balanced"
"#,
        world_dir.display(),
        vanilla_dir.display()
    );
    std::fs::write(path, toml).expect("write server config");
}

fn materialize_unbaked_spawn_window(world_dir: &Path, vanilla_dir: &Path) {
    std::fs::create_dir_all(world_dir.join("region")).expect("create region dir");
    let blocks_report =
        mc_data::blocks::load_blocks_report(vanilla_dir.join("reports/blocks.json"))
            .expect("load blocks report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report).expect("registry"));
    let generator = mc_worldgen::TerrainGenerator::try_with_biome_rules(
        0,
        Arc::clone(&blocks),
        mc_worldgen::BiomeRules::vanilla_overworld(),
    )
    .expect("terrain generator")
    .with_structures(mc_worldgen::StructureRules::none());
    let mut storage = mc_world::WorldStorage::open_with_capacities(
        world_dir,
        blocks,
        EXPECTED_SPAWN_WINDOW_CHUNKS,
        9,
    )
    .expect("open fixture storage");
    for pos in spawn_window_positions(VIEW_DISTANCE) {
        storage
            .insert_generated_chunk(pos, generator.generate(pos))
            .expect("insert generated unbaked chunk");
    }
    assert_eq!(
        storage.flush_dirty().expect("flush fixture chunks"),
        EXPECTED_SPAWN_WINDOW_CHUNKS
    );
}

fn spawn_window_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0) + 1;
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    positions
}

fn spawn_view_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0);
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    positions
}

enum RawServerOutput {
    Line(String),
    Closed,
}

enum ServerEvent {
    LogChanged,
    OutputClosed,
}

struct ServerProcess {
    child: Child,
    events: tokio::sync::mpsc::UnboundedReceiver<ServerEvent>,
}

fn spawn_server(config: &Path, log: &Path) -> ServerProcess {
    spawn_server_process(config, log, false)
}

fn spawn_server_with_stdin(config: &Path, log: &Path) -> ServerProcess {
    spawn_server_process(config, log, true)
}

fn spawn_server_process(config: &Path, log: &Path, pipe_stdin: bool) -> ServerProcess {
    File::create(log).expect("create server log");
    let mut command = Command::new(assert_cmd::cargo::cargo_bin("mc-server"));
    command
        .arg("--config")
        .arg(config)
        // Output wakes the disk probe; diagnostic wording is not acceptance evidence.
        .env("RUST_LOG", "debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if pipe_stdin {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn().expect("spawn mc-server");

    let (raw_tx, raw_rx) = mpsc::channel();
    forward_server_output(
        child.stdout.take().expect("server stdout is piped"),
        raw_tx.clone(),
    );
    forward_server_output(child.stderr.take().expect("server stderr is piped"), raw_tx);

    let (event_tx, events) = tokio::sync::mpsc::unbounded_channel();
    let log = log.to_owned();
    thread::spawn(move || {
        let mut log_file = File::create(log).expect("open forwarded server log");
        let mut closed_streams = 0;
        while closed_streams < 2 {
            match raw_rx
                .recv()
                .expect("server output forwarders stay connected")
            {
                RawServerOutput::Line(line) => {
                    log_file
                        .write_all(line.as_bytes())
                        .and_then(|()| log_file.flush())
                        .expect("write forwarded server log");
                    let _ = event_tx.send(ServerEvent::LogChanged);
                }
                RawServerOutput::Closed => closed_streams += 1,
            }
        }
        let _ = event_tx.send(ServerEvent::OutputClosed);
    });

    ServerProcess { child, events }
}

fn forward_server_output<R>(output: R, tx: mpsc::Sender<RawServerOutput>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).expect("read server output");
            if read == 0 {
                break;
            }
            if tx.send(RawServerOutput::Line(line.clone())).is_err() {
                return;
            }
        }
        let _ = tx.send(RawServerOutput::Closed);
    });
}

async fn connect_when_ready(addr: SocketAddr, server: &mut ServerProcess, log: &Path) -> Client {
    let connection = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            if let Ok(client) = Client::connect(addr).await {
                return Some(client);
            }
            match server.events.recv().await {
                Some(ServerEvent::LogChanged) => {}
                Some(ServerEvent::OutputClosed) | None => return None,
            }
        }
    })
    .await;
    match connection {
        Ok(Some(client)) => client,
        Ok(None) => {
            let status = server.child.wait().expect("wait for exited server");
            panic!(
                "server exited before accepting {addr}: status={status}; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        Err(_) => {
            let _ = kill_server_without_stop_and_wait(server, log, "listener readiness").await;
            panic!(
                "server did not accept {addr} before the deadline; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
    }
}

async fn drive_to_play(client: &mut Client, addr: SocketAddr, name: &str) {
    client.drive_login(addr, name).await.expect("login");
    client.drive_configuration().await.expect("configuration");
}

async fn drive_to_play_and_drain_unique(
    mut client: Client,
    addr: SocketAddr,
    name: &str,
) -> (Client, HashSet<(i32, i32)>) {
    drive_to_play(&mut client, addr, name).await;
    let chunks = drain_unique_chunks(&mut client, CONSOLE_STOP_STREAM_CHUNKS).await;
    (client, chunks)
}

#[derive(Debug)]
struct StreamDrain {
    first_chunk_ms: u128,
    ring1_complete_ms: Option<u128>,
    ring2_complete_ms: Option<u128>,
    full_window_ms: u128,
}

async fn drain_view_distance_window(client: &mut Client) -> StreamDrain {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(180);
    let mut seen = HashSet::new();
    let mut first_chunk_ms = None;
    let mut ring_counts = vec![0usize; (VIEW_DISTANCE + 1) as usize];
    let mut ring1_complete_ms = None;
    let mut ring2_complete_ms = None;
    while seen.len() < EXPECTED_CHUNKS {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("read startup stream frame");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let sync = SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await
                .expect("ack teleport");
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
            "chunk ({}, {}) outside view-distance window",
            pkt.chunk_x,
            pkt.chunk_z
        );
        let fresh = seen.insert((pkt.chunk_x, pkt.chunk_z));
        assert!(
            fresh,
            "duplicate chunk ({}, {}) on startup stream",
            pkt.chunk_x, pkt.chunk_z
        );
        first_chunk_ms.get_or_insert_with(|| started.elapsed().as_millis());
        let ring = pkt.chunk_x.abs().max(pkt.chunk_z.abs()) as usize;
        ring_counts[ring] += 1;
        if ring == 1 && ring_counts[1] == 8 {
            ring1_complete_ms.get_or_insert_with(|| started.elapsed().as_millis());
        }
        if ring == 2 && ring_counts[2] == 16 {
            ring2_complete_ms.get_or_insert_with(|| started.elapsed().as_millis());
        }
    }
    for cz in -VIEW_DISTANCE..=VIEW_DISTANCE {
        for cx in -VIEW_DISTANCE..=VIEW_DISTANCE {
            assert!(
                seen.contains(&(cx, cz)),
                "missing chunk ({cx}, {cz}) from startup stream"
            );
        }
    }
    StreamDrain {
        first_chunk_ms: first_chunk_ms.expect("at least one chunk"),
        ring1_complete_ms,
        ring2_complete_ms,
        full_window_ms: started.elapsed().as_millis(),
    }
}

async fn drain_unique_chunks(client: &mut Client, expected: usize) -> HashSet<(i32, i32)> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = HashSet::new();
    while seen.len() < expected {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {expected} unique chunks; seen={seen:?}"
        );
        let frame = client
            .read_frame_with_timeout(remaining)
            .await
            .expect("read console-stop stream frame");
        if frame.id == ClientboundKeepAlive::ID {
            let mut body = frame.body;
            let keepalive = ClientboundKeepAlive::decode(&mut body).expect("decode KeepAlive");
            client
                .write_packet(&ServerboundKeepAlive { id: keepalive.id })
                .await
                .expect("echo KeepAlive");
            continue;
        }
        if frame.id == SynchronizePlayerPosition::ID {
            let mut body = frame.body;
            let sync = SynchronizePlayerPosition::decode(&mut body).expect("decode SyncPlayerPos");
            client
                .write_packet(&ConfirmTeleportation {
                    teleport_id: sync.teleport_id,
                })
                .await
                .expect("ack teleport");
            continue;
        }
        if frame.id != LevelChunkWithLight::ID {
            continue;
        }
        let mut body = frame.body;
        let pkt = LevelChunkWithLight::decode(&mut body).expect("decode LevelChunkWithLight");
        seen.insert((pkt.chunk_x, pkt.chunk_z));
    }
    seen
}

async fn stop_server(client: &mut Client, server: &mut ServerProcess, log: &Path) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    wait_for_server_exit(server, log, Duration::from_secs(30)).await;
}

fn write_console_stop(server: &mut ServerProcess) {
    let stdin = server.child.stdin.as_mut().expect("console stdin is piped");
    stdin.write_all(b"stop\n").expect("write console stop");
    stdin.flush().expect("flush console stop");
}

async fn wait_for_server_exit(server: &mut ServerProcess, log: &Path, timeout: Duration) {
    if tokio::time::timeout(timeout, wait_for_output_close(server))
        .await
        .is_err()
    {
        let _ = server.child.kill();
        panic!(
            "server did not stop after command; log:\n{}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
    }
    let status = server.child.wait().expect("wait for stopped server");
    assert!(
        status.success(),
        "server exited non-zero: {status}; log:\n{}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
}

async fn wait_for_output_close(server: &mut ServerProcess) {
    loop {
        match server.events.recv().await {
            Some(ServerEvent::LogChanged) => {}
            Some(ServerEvent::OutputClosed) | None => return,
        }
    }
}

async fn wait_for_startup_dirty_checkpoint(
    server: &mut ServerProcess,
    log: &Path,
    world_dir: &Path,
    vanilla_dir: &Path,
) {
    let blocks_report =
        mc_data::blocks::load_blocks_report(vanilla_dir.join("reports/blocks.json"))
            .expect("load blocks report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report).expect("registry"));
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .expect("load block light report");
    let positions = spawn_window_positions(VIEW_DISTANCE);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let persisted = (|| {
            // Reopen for each observation: a cached region header cannot see another
            // process publishing new chunks. Never acquire the writer's world lease.
            let mut storage =
                mc_world::WorldStorage::open_read_only(world_dir, Arc::clone(&blocks)).ok()?;
            for &pos in &positions {
                let chunk = storage.get_chunk(pos).ok()??;
                if pos.x.abs() <= VIEW_DISTANCE && pos.z.abs() <= VIEW_DISTANCE {
                    if !chunk
                        .section_lights
                        .iter()
                        .any(|section| section.sky.is_some())
                    {
                        return None;
                    }
                    assert_spawn_view_chunk_light_sections_survived(chunk, &block_light, pos);
                }
            }
            Some(())
        })()
        .is_some();
        if persisted {
            return;
        }
        match tokio::time::timeout_at(deadline, server.events.recv()).await {
            Ok(Some(ServerEvent::LogChanged)) => {}
            Ok(Some(ServerEvent::OutputClosed)) | Ok(None) => {
                let status = server.child.wait().expect("wait for exited server");
                let log_text = std::fs::read_to_string(log).unwrap_or_default();
                panic!(
                    "server exited before startup data persisted: status={status}; log:\n{log_text}"
                );
            }
            Err(_) => {
                let _ = kill_server_without_stop_and_wait(server, log, "startup persistence").await;
                let log_text = std::fs::read_to_string(log).unwrap_or_default();
                panic!(
                    "startup chunks and baked light did not persist before the deadline; log:\n{log_text}"
                );
            }
        }
    }
}

async fn kill_server_without_stop_and_assert_exit(server: &mut ServerProcess, log: &Path) {
    let status = kill_server_without_stop_and_wait(server, log, "kill-without-stop").await;
    assert!(
        !status.success(),
        "kill-without-stop should not look like a graceful successful stop; status={status}"
    );
}

async fn kill_server_without_stop_and_wait(
    server: &mut ServerProcess,
    log: &Path,
    label: &str,
) -> std::process::ExitStatus {
    assert!(
        server
            .child
            .try_wait()
            .expect("check server before kill")
            .is_none(),
        "{label}: server exited before kill; log:\n{}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
    server
        .child
        .kill()
        .unwrap_or_else(|err| panic!("{label}: kill server without stop: {err}"));
    if tokio::time::timeout(Duration::from_secs(10), wait_for_output_close(server))
        .await
        .is_err()
    {
        panic!(
            "{label}: server did not exit after kill; log:\n{}",
            std::fs::read_to_string(log).unwrap_or_default()
        );
    }
    server.child.wait().expect("wait for killed server")
}

fn assert_streamed_chunks_on_disk(
    world_dir: &Path,
    vanilla_dir: &Path,
    streamed_chunks: &HashSet<(i32, i32)>,
) {
    let blocks_report =
        mc_data::blocks::load_blocks_report(vanilla_dir.join("reports/blocks.json"))
            .expect("load blocks report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report).expect("registry"));
    let mut storage =
        mc_world::WorldStorage::open_with_capacity(world_dir, blocks, streamed_chunks.len().max(4))
            .expect("reopen console-stop world");
    for (cx, cz) in streamed_chunks {
        let chunk = storage
            .get_chunk(mc_world::ChunkPos { x: *cx, z: *cz })
            .expect("read streamed chunk after console stop");
        assert!(
            chunk.is_some(),
            "streamed chunk ({cx},{cz}) should exist on disk after console stop"
        );
    }
}

fn assert_spawn_window_chunks_on_disk(world_dir: &Path, vanilla_dir: &Path) {
    let blocks_report =
        mc_data::blocks::load_blocks_report(vanilla_dir.join("reports/blocks.json"))
            .expect("load blocks report");
    let block_light = mc_data::block_light::load(vanilla_dir.join("reports/block_light.json"))
        .expect("load block light report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&blocks_report).expect("registry"));
    let mut storage = mc_world::WorldStorage::open_with_capacities(
        world_dir,
        blocks,
        EXPECTED_SPAWN_WINDOW_CHUNKS,
        9,
    )
    .expect("reopen generated startup checkpoint world");
    for pos in spawn_window_positions(VIEW_DISTANCE) {
        let chunk = storage
            .get_chunk(pos)
            .expect("read spawn window chunk after kill-without-stop");
        assert!(
            chunk.is_some(),
            "spawn window chunk ({}, {}) should exist on disk after startup dirty checkpoint",
            pos.x,
            pos.z
        );
    }
    for pos in spawn_view_positions(VIEW_DISTANCE) {
        let chunk = storage
            .get_chunk(pos)
            .expect("read baked spawn view chunk after kill-without-stop")
            .unwrap_or_else(|| {
                panic!(
                    "spawn view chunk ({}, {}) should exist on disk after startup dirty checkpoint",
                    pos.x, pos.z
                )
            });
        assert_spawn_view_chunk_light_sections_survived(chunk, &block_light, pos);
    }
}

fn assert_spawn_view_chunk_light_sections_survived(
    chunk: &mc_world::Chunk,
    block_light: &mc_data::block_light::BlockLightTable,
    pos: mc_world::ChunkPos,
) {
    assert_eq!(
        chunk.section_lights.len(),
        mc_world::SECTION_COUNT,
        "spawn view chunk ({}, {}) should retain one light slot per section",
        pos.x,
        pos.z
    );

    let expected_sky_sections = expected_direct_sky_sections(chunk, block_light);
    let stored_sky_sections = chunk
        .section_lights
        .iter()
        .enumerate()
        .filter_map(|(idx, section)| section.sky.as_ref().map(|_| idx))
        .collect::<HashSet<_>>();
    for section_idx in &expected_sky_sections {
        assert!(
            stored_sky_sections.contains(section_idx),
            "spawn view chunk ({}, {}) missing persisted sky light section {section_idx}; expected={expected_sky_sections:?} stored={stored_sky_sections:?}",
            pos.x,
            pos.z
        );
    }
    assert_full_direct_sky_sections_survived(chunk, block_light, pos);
    let baked = mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights)
        .expect("persisted spawn view must retain baked light arrays");
    assert_eq!(
        baked.sky_at(8, mc_world::MAX_Y - 1, 8),
        15,
        "spawn view chunk ({}, {}) must retain top skylight",
        pos.x,
        pos.z
    );
}

fn expected_direct_sky_sections(
    chunk: &mc_world::Chunk,
    block_light: &mc_data::block_light::BlockLightTable,
) -> HashSet<usize> {
    let mut sections = HashSet::new();
    for z in 0..16 {
        for x in 0..16 {
            for y in (mc_world::MIN_Y..mc_world::MAX_Y).rev() {
                let Some(state) = chunk.get_block(x, y, z) else {
                    break;
                };
                if !block_light.propagates_sky(state.0).unwrap_or(true) {
                    break;
                }
                sections.insert((y - mc_world::MIN_Y) as usize / mc_world::SECTION_DIM);
            }
        }
    }
    sections
}

fn assert_full_direct_sky_sections_survived(
    chunk: &mc_world::Chunk,
    block_light: &mc_data::block_light::BlockLightTable,
    pos: mc_world::ChunkPos,
) {
    let first_full_sky_section = first_full_direct_sky_section(chunk, block_light);
    assert!(
        first_full_sky_section < mc_world::SECTION_COUNT,
        "spawn view chunk ({}, {}) should have at least one full direct-sky section",
        pos.x,
        pos.z
    );
    for section_idx in first_full_sky_section..mc_world::SECTION_COUNT {
        let sky = chunk.section_lights[section_idx]
            .sky
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "spawn view chunk ({}, {}) should retain full SkyLight section {section_idx} above sky blockers",
                    pos.x, pos.z
                )
            });
        assert!(
            sky.bytes().all(|byte| byte == 0xFF),
            "spawn view chunk ({}, {}) full SkyLight section {section_idx} should persist as skylight 15",
            pos.x,
            pos.z
        );
    }
}

fn first_full_direct_sky_section(
    chunk: &mc_world::Chunk,
    block_light: &mc_data::block_light::BlockLightTable,
) -> usize {
    let first_sky_y = (0..16)
        .flat_map(|z| {
            (0..16).filter_map(move |x| {
                (mc_world::MIN_Y..mc_world::MAX_Y)
                    .rev()
                    .find(|y| {
                        let Some(state) = chunk.get_block(x, *y, z) else {
                            return false;
                        };
                        !block_light.propagates_sky(state.0).unwrap_or(true)
                    })
                    .map(|y| y + 1)
            })
        })
        .max()
        .unwrap_or(mc_world::MIN_Y);
    let offset = (first_sky_y - mc_world::MIN_Y).max(0) as usize;
    offset / mc_world::SECTION_DIM + usize::from(!offset.is_multiple_of(mc_world::SECTION_DIM))
}
