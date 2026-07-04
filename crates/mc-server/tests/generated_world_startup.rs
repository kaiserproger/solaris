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
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundKeepAlive, ConfirmTeleportation, LevelChunkWithLight, ServerboundChatCommand,
    ServerboundKeepAlive, SynchronizePlayerPosition,
};
use mc_test_harness::client::Client;

const VIEW_DISTANCE: i32 = 8;
const EXPECTED_CHUNKS: usize = ((VIEW_DISTANCE * 2 + 1) * (VIEW_DISTANCE * 2 + 1)) as usize;
const EXPECTED_SPAWN_WINDOW_CHUNKS: usize =
    (((VIEW_DISTANCE + 1) * 2 + 1) * ((VIEW_DISTANCE + 1) * 2 + 1)) as usize;
const STARTUP_TO_LISTENER_BUDGET: Duration = Duration::from_secs(10);

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
    assert_save_all_flushed_spawn_window(&first_log_text, "fresh startup stop");
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
"#,
        world_dir.display(),
        vanilla_dir.display()
    );
    std::fs::write(path, toml).expect("write server config");
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

fn assert_save_all_flushed_spawn_window(log: &str, label: &str) {
    let summary = log
        .lines()
        .find(|line| {
            line.contains("world storage save pressure")
                && line.contains(&format!("flushed={EXPECTED_SPAWN_WINDOW_CHUNKS}"))
        })
        .unwrap_or_else(|| panic!("{label}: missing save-all flush log:\n{log}"));
    for expected in [
        format!("planned={EXPECTED_SPAWN_WINDOW_CHUNKS}"),
        format!("dirty_before={EXPECTED_SPAWN_WINDOW_CHUNKS}"),
        "dirty_after=0".to_string(),
    ] {
        assert!(
            summary.contains(&expected),
            "{label}: save-all flush summary missing `{expected}`:\n{summary}"
        );
    }
}
