//! Ignored disk-backed generated-world startup/stream gate for M100.
//!
//! This exercises the `mc-server` binary startup path rather than the in-process
//! `mc_net::bind` path: fresh world pre-generation, baked spawn light, listener
//! readiness, 289-chunk view-distance-8 stream, process stop, restart, and the
//! warmed stream. It is ignored because it depends on local vanilla sidecars and
//! is intended to remain a performance blocker gate until the startup budget is
//! fixed.

use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
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
const STARTUP_TO_LISTENER_BUDGET: Duration = Duration::from_secs(10);
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
    let first_startup = first_started.elapsed();
    drive_to_play(&mut first_client, first_addr, "M100DiskA").await;
    let first_stream = drain_view_distance_window(&mut first_client).await;
    stop_server(&mut first_client, &mut first, &first_log).await;
    let first_log_text = std::fs::read_to_string(&first_log).expect("read first log");
    assert_deferred_startup_flush(&first_log_text, "fresh startup");
    assert_save_all_flushed_chunks(
        &first_log_text,
        "fresh startup stop",
        EXPECTED_SPAWN_WINDOW_CHUNKS,
    );
    assert_chunk_stream_summary(&first_log_text, "fresh startup");

    let second_addr = loopback_addr_with_reserved_port();
    let second_config = temp.path().join("second.toml");
    write_server_config(&second_config, &world_dir, &vanilla_dir, second_addr.port());
    let second_started = Instant::now();
    let mut second = spawn_server(&second_config, &second_log);
    let mut second_client = connect_when_ready(second_addr, &mut second, &second_log).await;
    let second_startup = second_started.elapsed();
    drive_to_play(&mut second_client, second_addr, "M100DiskB").await;
    let second_stream = drain_view_distance_window(&mut second_client).await;
    stop_server(&mut second_client, &mut second, &second_log).await;
    let second_log_text = std::fs::read_to_string(&second_log).expect("read second log");
    assert_chunk_stream_summary(&second_log_text, "warmed restart");

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
        first_startup <= STARTUP_TO_LISTENER_BUDGET,
        "fresh generated-world startup-to-listener exceeded budget: startup={first_startup:?} \
         budget={STARTUP_TO_LISTENER_BUDGET:?}; first_stream={first_stream:?} \
         second_startup={second_startup:?} second_stream={second_stream:?}"
    );
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
    let startup = started.elapsed();
    drive_to_play(&mut client, addr, "M100Unbaked").await;
    let stream = drain_view_distance_window(&mut client).await;
    stop_server(&mut client, &mut server, &log).await;
    let log_text = std::fs::read_to_string(&log).expect("read server log");

    assert_existing_missing_light_deferred_flush(&log_text, "existing missing-light startup");
    assert_save_all_flushed_chunks(&log_text, "existing missing-light stop", EXPECTED_CHUNKS);
    assert_chunk_stream_summary(&log_text, "existing missing-light startup");

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
        startup <= STARTUP_TO_LISTENER_BUDGET,
        "existing generated-world missing-light startup-to-listener exceeded budget: \
         startup={startup:?} budget={STARTUP_TO_LISTENER_BUDGET:?}; stream={stream:?}"
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
    assert!(
        !streamed_chunks.is_empty(),
        "console-stop gate should stream generated chunks before stop"
    );

    write_console_stop(&mut server);
    drop(clients);
    wait_for_server_exit(&mut server, &log, Duration::from_secs(30)).await;
    let log_text = std::fs::read_to_string(&log).expect("read console-stop log");
    assert_console_stop_requested_shutdown_before_save(&log_text, "console stop");
    assert_final_shutdown_save_quiescent(&log_text, "console stop");
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
random_tick_chunk_budget = 32
save_interval_ticks = 1200

[chunk_pipeline]
chunk_send_rate = 8
chunk_load_rate = 16
chunk_generate_rate = 16
chunk_prepare_budget_ms = 0
chunk_prepare_batch_size = 8
chunk_io_threads_percent = 25
chunk_worker_threads_percent = 25
entity_worker_threads_percent = 25
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

fn spawn_server(config: &Path, log: &Path) -> Child {
    let log_file = File::create(log).expect("create server log");
    let stderr = log_file.try_clone().expect("clone server log");
    Command::new(assert_cmd::cargo::cargo_bin("mc-server"))
        .arg("--config")
        .arg(config)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn mc-server")
}

fn spawn_server_with_stdin(config: &Path, log: &Path) -> Child {
    let log_file = File::create(log).expect("create server log");
    let stderr = log_file.try_clone().expect("clone server log");
    Command::new(assert_cmd::cargo::cargo_bin("mc-server"))
        .arg("--config")
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn mc-server")
}

async fn connect_when_ready(addr: SocketAddr, child: &mut Child, log: &Path) -> Client {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if let Some(status) = child.try_wait().expect("poll server process") {
            panic!(
                "server exited before listener became reachable: status={status}; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        match Client::connect(addr).await {
            Ok(client) => return client,
            Err(err) => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {addr}: {err}; log:\n{}",
                    std::fs::read_to_string(log).unwrap_or_default()
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
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

async fn stop_server(client: &mut Client, child: &mut Child, log: &Path) {
    client
        .write_packet(&ServerboundChatCommand {
            command: "stop".to_string(),
        })
        .await
        .expect("send stop command");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().expect("poll server process") {
            assert!(
                status.success(),
                "server exited non-zero: {status}; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!(
                "server did not stop after command; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn write_console_stop(child: &mut Child) {
    let stdin = child.stdin.as_mut().expect("console stdin is piped");
    stdin.write_all(b"stop\n").expect("write console stop");
    stdin.flush().expect("flush console stop");
}

async fn wait_for_server_exit(child: &mut Child, log: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("poll server process") {
            assert!(
                status.success(),
                "server exited non-zero: {status}; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!(
                "server did not stop after console command; log:\n{}",
                std::fs::read_to_string(log).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn assert_chunk_stream_summary(log: &str, label: &str) {
    let summary = log
        .lines()
        .rev()
        .find(|line| line.contains("view-distance window flushed"))
        .unwrap_or_else(|| panic!("{label}: missing view-distance summary log:\n{log}"));
    for expected in [
        "emitted=289",
        "absent=0",
        "pressure_flush_runs=0",
        "degraded_delivery=false",
        "light_compute_ms=0",
        "slow_fetch_chunks=0",
        "slow_light_compute_chunks=0",
    ] {
        assert!(
            summary.contains(expected),
            "{label}: summary missing `{expected}`:\n{summary}"
        );
    }
}

fn assert_console_stop_requested_shutdown_before_save(log: &str, label: &str) {
    let request_idx = log
        .lines()
        .position(|line| line.contains("console stop requested shutdown before save-all"))
        .unwrap_or_else(|| {
            panic!("{label}: missing console stop shutdown-before-save log:\n{log}")
        });
    let save_idx = log
        .lines()
        .position(|line| line.contains("console stop") && line.contains("save-all complete"))
        .unwrap_or_else(|| panic!("{label}: missing console stop save-all log:\n{log}"));
    assert!(
        request_idx < save_idx,
        "{label}: console save-all started before shutdown request log"
    );
}

fn assert_final_shutdown_save_quiescent(log: &str, label: &str) {
    let lines: Vec<_> = log.lines().collect();
    let final_save_idx = lines
        .iter()
        .position(|line| {
            line.contains("listener shutdown final save") && line.contains("save-all complete")
        })
        .unwrap_or_else(|| panic!("{label}: missing listener final save log:\n{log}"));
    let pressure = lines[..final_save_idx]
        .iter()
        .rev()
        .find(|line| line.contains("world storage save pressure"))
        .unwrap_or_else(|| panic!("{label}: missing final save pressure log before final save"));
    for expected in ["flushed=0", "planned=0", "dirty_before=0", "dirty_after=0"] {
        assert!(
            pressure.contains(expected),
            "{label}: final shutdown save was not quiescent; missing `{expected}`:\n{pressure}"
        );
    }
}

fn assert_deferred_startup_flush(log: &str, label: &str) {
    let summary = log
        .lines()
        .find(|line| line.contains("disk flush deferred to save-all"))
        .unwrap_or_else(|| panic!("{label}: missing deferred startup flush log:\n{log}"));
    for expected in [
        format!("chunks={EXPECTED_SPAWN_WINDOW_CHUNKS}"),
        format!("dirty={EXPECTED_SPAWN_WINDOW_CHUNKS}"),
        "region_files=0".to_string(),
    ] {
        assert!(
            summary.contains(&expected),
            "{label}: deferred flush summary missing `{expected}`:\n{summary}"
        );
    }
}

fn assert_existing_missing_light_deferred_flush(log: &str, label: &str) {
    let summary = log
        .lines()
        .find(|line| line.contains("existing world spawn window warmed"))
        .unwrap_or_else(|| panic!("{label}: missing existing-world warm-cache log:\n{log}"));
    for expected in [
        format!("chunks={EXPECTED_SPAWN_WINDOW_CHUNKS}"),
        format!("baked={EXPECTED_CHUNKS}"),
        "flushed=0".to_string(),
        format!("dirty={EXPECTED_CHUNKS}"),
    ] {
        assert!(
            summary.contains(&expected),
            "{label}: existing-world warm-cache summary missing `{expected}`:\n{summary}"
        );
    }
}

fn assert_save_all_flushed_chunks(log: &str, label: &str, expected_chunks: usize) {
    let summary = log
        .lines()
        .find(|line| {
            line.contains("world storage save pressure")
                && line.contains(&format!("flushed={expected_chunks}"))
        })
        .unwrap_or_else(|| panic!("{label}: missing save-all flush log:\n{log}"));
    for expected in [
        format!("planned={expected_chunks}"),
        format!("dirty_before={expected_chunks}"),
        "dirty_after=0".to_string(),
    ] {
        assert!(
            summary.contains(&expected),
            "{label}: save-all flush summary missing `{expected}`:\n{summary}"
        );
    }
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
