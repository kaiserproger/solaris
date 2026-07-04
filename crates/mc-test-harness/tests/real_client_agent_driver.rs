use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[test]
fn agent_driver_writes_passed_observation_from_loopback_bridge() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start(6);

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        commands,
        [
            "ping",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert!(
        requests
            .iter()
            .all(|request| request["secret"] == "test-secret"),
        "every bridge request must include the shared secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");

    assert_eq!(
        observations["schema"],
        "solaris.real_client_observations.v1"
    );
    assert_eq!(observations["client_gate"], "agent-run-real-client");
    assert_eq!(observations["quality_label"], "stabilization");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-02b-rejected-block-resync"
    );
    assert_eq!(observations["scenarios"][0]["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["agent_report"]["result"],
        "passed"
    );
    assert_eq!(
        observations["scenarios"][0]["final_state"]["dimension"],
        "minecraft:overworld"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| !screenshots.is_empty()),
        "passed observations must include a screenshot path"
    );
}

#[test]
fn agent_driver_connects_when_client_is_not_already_in_play() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_wait_play(8, vec![false, true]);

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        commands,
        [
            "ping",
            "wait_play",
            "connect",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
}

#[test]
fn agent_driver_waits_when_client_is_already_connecting() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_wait_play_payloads(
        7,
        vec![
            json!({
                "in_play": false,
                "dimension": "",
                "current_screen": "net.minecraft.client.gui.screens.ConnectScreen",
                "disconnect_reason": "",
            }),
            wait_play_payload(true),
        ],
    );

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        commands,
        [
            "ping",
            "wait_play",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ],
        "driver must not send a second connect while the client is already connecting"
    );
}

#[test]
fn agent_driver_waits_for_interactive_play_screen_before_scenario() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_wait_play_payloads(
        7,
        vec![
            json!({
                "in_play": true,
                "dimension": "minecraft:overworld",
                "current_screen": "net.minecraft.client.gui.screens.LevelLoadingScreen",
                "disconnect_reason": "",
            }),
            json!({
                "in_play": true,
                "dimension": "minecraft:overworld",
                "current_screen": "none",
                "disconnect_reason": "",
            }),
        ],
    );

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        commands,
        [
            "ping",
            "wait_play",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ],
        "driver must wait for the vanilla client to leave LevelLoadingScreen before Java-agent scenarios"
    );
}

#[test]
fn agent_driver_waits_for_async_screenshot_file() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_delayed_screenshot(6, Duration::from_millis(150));

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert!(
        run_dir
            .path()
            .join("screenshots/m94-02b-rejected-block-resync.png")
            .is_file(),
        "delayed fake screenshot must exist before the driver reports pass"
    );

    bridge.join();
}

#[test]
fn agent_driver_runs_join_rejoin_movement_scenario_without_run_scenario_rpc() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    std::fs::write(
        run_dir.path().join("server.log"),
        "2026-06-22T14:10:54.741091Z  INFO mc_net::play: saved player state player=SolarisAgent state=pos=(-2.31,80.00,-6.58)\n",
    )
    .expect("write server release marker");
    let bridge = FakeBridge::start(13);

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-01-join-rejoin-chunks-movement")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        commands,
        [
            "ping",
            "wait_play",
            "state",
            "move_forward",
            "state",
            "screenshot",
            "disconnect",
            "state",
            "connect",
            "wait_play",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert_eq!(requests[3]["payload"]["duration_millis"], 750);
    assert_eq!(
        requests[7]["command"], "state",
        "driver must poll client state after disconnect"
    );
    assert_eq!(
        requests[8]["command"], "connect",
        "driver must wait for the client to leave Play before reconnecting"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-01-join-rejoin-chunks-movement"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry
                .as_str()
                .is_some_and(|text| text.contains("movement probe: passed")))),
        "scenario report must record the movement probe outcome"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "join/rejoin scenario must attach before and after reconnect screenshots"
    );
}

#[test]
fn agent_driver_waits_for_server_session_release_before_rejoin() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let server_log_path = run_dir.path().join("server.log");
    std::fs::write(&server_log_path, "").expect("create server log");
    let bridge =
        FakeBridge::start_requiring_server_release_before_connect(13, server_log_path.clone());

    let release_writer = thread::spawn({
        let server_log_path = server_log_path.clone();
        move || {
            thread::sleep(Duration::from_millis(150));
            std::fs::write(
                &server_log_path,
                "2026-06-22T14:10:54.741091Z  INFO mc_net::play: saved player state player=SolarisAgent state=pos=(-2.31,80.00,-6.58)\n",
            )
            .expect("write server release marker");
        }
    });

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-01-join-rejoin-chunks-movement")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    release_writer.join().expect("release writer joins");
    assert!(
        output.status.success(),
        "driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    let commands: Vec<_> = requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(commands[8], "connect");

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry
                .as_str()
                .is_some_and(|text| text.contains("server session release: observed")))),
        "scenario report must record the server-side session release wait"
    );
}

#[test]
fn agent_driver_appends_phase_observations_and_recomputes_blocked_result() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let observations_path = run_dir.path().join("observations.json");
    std::fs::write(
        &observations_path,
        serde_json::to_vec_pretty(&json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "server_addr": "127.0.0.1:25565",
            "generated_at": "2026-06-22T00:00:00Z",
            "scenarios": [{
                "id": "m94-06-save-restart-before",
                "result": "passed",
                "commands": [],
                "final_state": {},
                "screenshots": ["screenshots/m94-06-save-restart-before.png"]
            }]
        }))
        .expect("serialize existing observations"),
    )
    .expect("write existing observations");
    let bridge = FakeBridge::start_with_scenario_result(6, "blocked");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-06-save-restart-after")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .arg("--append-observations")
        .output()
        .expect("run real-client agent driver");

    assert!(
        !output.status.success(),
        "blocked appended scenario must keep the driver exit fail-closed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "blocked");
    assert_eq!(
        observations["scenarios"]
            .as_array()
            .expect("scenarios is an array")
            .len(),
        2
    );
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-06-save-restart-before"
    );
    assert_eq!(
        observations["scenarios"][1]["id"],
        "m94-06-save-restart-after"
    );
    assert_eq!(observations["scenarios"][1]["result"], "blocked");

    bridge.join();
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_m94_06_visibility() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(6, "passed");
    let secondary = FakeBridge::start_with_scenario_result(6, "passed");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", primary.port))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", secondary.port))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-06-two-client-live-visibility")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    let primary_commands: Vec<_> = primary_requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        primary_commands,
        [
            "ping",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert_eq!(
        primary_requests[2]["payload"]["id"],
        "m94-06-two-client-place"
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    let secondary_commands: Vec<_> = secondary_requests
        .iter()
        .map(|request| request["command"].as_str().expect("command is present"))
        .collect();
    assert_eq!(
        secondary_commands,
        [
            "ping",
            "wait_play",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert_eq!(
        secondary_requests[2]["payload"]["id"],
        "m94-06-two-client-observe"
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-06-two-client-live-visibility"
    );
    assert_eq!(observations["scenarios"][0]["result"], "passed");
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry
                .as_str()
                .is_some_and(|text| text.contains(
                    "secondary bridge scenario=m94-06-two-client-observe result=passed"
                )))),
        "combined report must record secondary real-client observation"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "two-client visibility run must attach both client screenshots"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_m94_06_shared_drop() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(6, "passed");
    let secondary = FakeBridge::start_with_scenario_result(6, "passed");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", primary.port))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", secondary.port))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-06-two-client-shared-drop")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client shared-drop driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    assert_eq!(
        primary_requests[2]["payload"]["id"],
        "m94-06-two-client-drop-break"
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary shared-drop bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    assert_eq!(
        secondary_requests[2]["payload"]["id"],
        "m94-06-two-client-drop-observe"
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary shared-drop bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-06-two-client-shared-drop"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "secondary bridge scenario=m94-06-two-client-drop-observe result=passed"
                    )))
            ),
        "combined report must record secondary shared-drop observation"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_m94_03b_shared_chest() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(6, "passed");
    let secondary = FakeBridge::start_with_scenario_result(6, "passed");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", primary.port))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", secondary.port))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-03b-two-client-shared-chest")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client shared-chest driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    assert_eq!(
        primary_requests[2]["payload"]["id"],
        "m94-03b-two-client-shared-chest-deposit"
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary shared-chest bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    assert_eq!(
        secondary_requests[2]["payload"]["id"],
        "m94-03b-two-client-shared-chest-observe"
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary shared-chest bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-03b-two-client-shared-chest"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "secondary bridge scenario=m94-03b-two-client-shared-chest-observe result=passed"
                    )))
            ),
        "combined report must record secondary shared-chest observation"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_m94_03c_shared_chest_live_update() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(7, "passed");
    let secondary = FakeBridge::start_with_scenario_result(6, "passed");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", primary.port))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", secondary.port))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-03c-two-client-shared-chest-live-update")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client shared-chest live-update driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    let primary_scenarios: Vec<_> = primary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| request["payload"]["id"].as_str().expect("scenario id"))
        .collect();
    assert_eq!(
        primary_scenarios,
        [
            "m94-03c-two-client-shared-chest-open-with-dirt",
            "m94-03c-two-client-shared-chest-observe-empty"
        ]
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary live-update bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    let secondary_scenarios: Vec<_> = secondary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| request["payload"]["id"].as_str().expect("scenario id"))
        .collect();
    assert_eq!(
        secondary_scenarios,
        ["m94-03c-two-client-shared-chest-withdraw"]
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary live-update bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-03c-two-client-shared-chest-live-update"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "primary bridge scenario=m94-03c-two-client-shared-chest-observe-empty result=passed"
                    )))
            ),
        "combined report must record primary observation of the secondary chest mutation"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_m94_06_shared_pickup() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(7, "passed");
    let secondary = FakeBridge::start_with_scenario_result(7, "passed");

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", primary.port))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", secondary.port))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-06-two-client-shared-pickup")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client shared-pickup driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    let primary_scenarios: Vec<_> = primary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| request["payload"]["id"].as_str().expect("scenario id"))
        .collect();
    assert_eq!(
        primary_scenarios,
        [
            "m94-06-two-client-drop-break",
            "m94-06-two-client-pickup-collect"
        ]
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary shared-pickup bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    let secondary_scenarios: Vec<_> = secondary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| request["payload"]["id"].as_str().expect("scenario id"))
        .collect();
    assert_eq!(
        secondary_scenarios,
        [
            "m94-06-two-client-drop-observe",
            "m94-06-two-client-pickup-gone-observe"
        ]
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary shared-pickup bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "m94-06-two-client-shared-pickup"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "secondary bridge scenario=m94-06-two-client-pickup-gone-observe result=passed"
                    )))
            ),
        "combined report must record secondary shared-pickup removal observation"
    );
}

#[test]
fn agent_driver_fails_closed_without_bridge() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let unused_port = reserve_then_release_port();

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{unused_port}/rpc"))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("0.2")
        .output()
        .expect("run real-client agent driver");

    assert!(
        !output.status.success(),
        "driver unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("failed observations.json exists"),
    )
    .expect("failed observations.json is valid JSON");

    assert_eq!(observations["client_gate"], "agent-run-real-client");
    assert_eq!(observations["result"], "failed");
    assert_ne!(observations["result"], "passed");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "failed observations must include a diagnostic error"
    );
}

#[test]
fn agent_driver_reports_structured_bridge_error_body() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FailingWaitPlayBridge::start();

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("m94-02b-rejected-block-resync")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        !output.status.success(),
        "driver unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    bridge.join();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("command-failed: snapshot boom"),
        "driver stderr must include the structured bridge error, got:\n{stderr}"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("failed observations.json exists"),
    )
    .expect("failed observations.json is valid JSON");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("command-failed: snapshot boom")),
        "failed observations must preserve the structured bridge error"
    );
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn reserve_then_release_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused port");
    listener.local_addr().expect("local addr").port()
}

struct FailingWaitPlayBridge {
    port: u16,
    handle: Option<thread::JoinHandle<()>>,
}

impl FailingWaitPlayBridge {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing bridge");
        listener
            .set_nonblocking(true)
            .expect("failing bridge nonblocking");
        let port = listener.local_addr().expect("failing bridge addr").port();

        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = 0;
            while Instant::now() < deadline && requests < 2 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_json_request(&mut stream);
                        requests += 1;
                        let request_id = request["id"].as_u64().expect("request id");
                        if request["command"].as_str() == Some("wait_play") {
                            write_json_response_with_status(
                                &mut stream,
                                &json!({
                                    "id": request_id,
                                    "ok": false,
                                    "payload": null,
                                    "error": {
                                        "code": "command-failed",
                                        "message": "snapshot boom",
                                    },
                                }),
                                500,
                            );
                        } else {
                            write_json_response(
                                &mut stream,
                                &json!({
                                    "id": request_id,
                                    "ok": true,
                                    "payload": {"agent": "fake-real-client"},
                                    "error": null,
                                }),
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("failing bridge accept failed: {error}"),
                }
            }
        });

        Self {
            port,
            handle: Some(handle),
        }
    }

    fn join(mut self) {
        self.handle
            .take()
            .expect("failing bridge thread exists")
            .join()
            .expect("failing bridge thread joins");
    }
}

struct FakeBridge {
    port: u16,
    requests: Arc<Mutex<Vec<Value>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FakeBridge {
    fn start(expected_requests: usize) -> Self {
        Self::start_with_wait_play(expected_requests, vec![true])
    }

    fn start_with_wait_play(expected_requests: usize, wait_play_results: Vec<bool>) -> Self {
        Self::start_with_wait_play_payloads(
            expected_requests,
            wait_play_results
                .into_iter()
                .map(wait_play_payload)
                .collect(),
        )
    }

    fn start_with_wait_play_payloads(
        expected_requests: usize,
        wait_play_results: Vec<Value>,
    ) -> Self {
        Self::start_with_options(
            expected_requests,
            wait_play_results,
            Duration::ZERO,
            None,
            "passed",
        )
    }

    fn start_with_delayed_screenshot(expected_requests: usize, screenshot_delay: Duration) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            screenshot_delay,
            None,
            "passed",
        )
    }

    fn start_requiring_server_release_before_connect(
        expected_requests: usize,
        server_release_log_path: std::path::PathBuf,
    ) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            Duration::ZERO,
            Some(server_release_log_path),
            "passed",
        )
    }

    fn start_with_scenario_result(expected_requests: usize, scenario_result: &'static str) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            Duration::ZERO,
            None,
            scenario_result,
        )
    }

    fn start_with_options(
        expected_requests: usize,
        wait_play_results: Vec<Value>,
        screenshot_delay: Duration,
        server_release_log_path: Option<std::path::PathBuf>,
        scenario_result: &'static str,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake bridge");
        listener
            .set_nonblocking(true)
            .expect("fake bridge nonblocking");
        let port = listener.local_addr().expect("fake bridge addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let wait_play_results = Arc::new(Mutex::new(VecDeque::from(wait_play_results)));
        let thread_wait_play_results = Arc::clone(&wait_play_results);
        let movement_count = Arc::new(Mutex::new(0_u32));
        let thread_movement_count = Arc::clone(&movement_count);
        let play_state = Arc::new(Mutex::new(true));
        let thread_play_state = Arc::clone(&play_state);

        let handle = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if thread_requests.lock().expect("requests lock").len() >= expected_requests {
                    break;
                }

                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_json_request(&mut stream);
                        let response = bridge_response(
                            &request,
                            &thread_wait_play_results,
                            &thread_movement_count,
                            &thread_play_state,
                            screenshot_delay,
                            server_release_log_path.as_ref(),
                            scenario_result,
                        );
                        thread_requests.lock().expect("requests lock").push(request);
                        write_json_response(&mut stream, &response);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fake bridge accept failed: {error}"),
                }
            }
        });

        Self {
            port,
            requests,
            handle: Some(handle),
        }
    }

    fn join(mut self) -> Vec<Value> {
        self.handle
            .take()
            .expect("fake bridge thread exists")
            .join()
            .expect("fake bridge thread joins");
        self.requests.lock().expect("requests lock").clone()
    }
}

fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk).expect("read request");
        assert!(read > 0, "request closed before headers");
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
    };

    let header = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().expect("content length"))
        })
        .expect("content length header");

    let body_start = header_end + 4;
    let mut body = buffer[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk).expect("read request body");
        assert!(read > 0, "request closed before body");
        body.extend_from_slice(&chunk[..read]);
    }

    serde_json::from_slice(&body[..content_length]).expect("request body json")
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn bridge_response(
    request: &Value,
    wait_play_results: &Mutex<VecDeque<Value>>,
    movement_count: &Mutex<u32>,
    play_state: &Mutex<bool>,
    screenshot_delay: Duration,
    server_release_log_path: Option<&std::path::PathBuf>,
    scenario_result: &'static str,
) -> Value {
    let request_id = request["id"].as_u64().expect("request id");
    if request["command"].as_str() == Some("connect")
        && server_release_log_path.is_some_and(|path| !server_session_release_logged(path))
    {
        return json!({
            "id": request_id,
            "ok": false,
            "payload": null,
            "error": {
                "code": "server-session-still-active",
                "message": "server release marker was not observed before reconnect",
            },
        });
    }

    let payload = match request["command"].as_str().expect("command") {
        "ping" => json!({"client": "fake-real-client", "agent_version": "test"}),
        "connect" => {
            *play_state.lock().expect("play lock") = true;
            json!({
                "connected": true,
                "server_addr": request["payload"]["server_addr"],
            })
        }
        "wait_play" => wait_play_results
            .lock()
            .expect("wait_play lock")
            .pop_front()
            .unwrap_or_else(|| wait_play_payload(true)),
        "move_forward" => {
            *movement_count.lock().expect("movement lock") += 1;
            json!({"status": "ok"})
        }
        "run_scenario" => json!({
            "result": scenario_result,
            "id": request["payload"]["id"],
            "observations": [
                "fake bridge executed the scenario action path"
            ],
        }),
        "state" => {
            let moved = *movement_count.lock().expect("movement lock") > 0;
            let in_play = *play_state.lock().expect("play lock");
            json!({
                "in_play": in_play,
                "dimension": if in_play { "minecraft:overworld" } else { "" },
                "x": 0.5,
                "y": 64.0,
                "z": if moved { 1.25 } else { 0.5 },
                "selected_hotbar_slot": 0,
                "screen": null,
            })
        }
        "screenshot" => {
            let path = request["payload"]["path"]
                .as_str()
                .expect("screenshot path");
            if screenshot_delay.is_zero() {
                std::fs::write(path, b"fake png").expect("write fake screenshot");
            } else {
                let path = path.to_owned();
                thread::spawn(move || {
                    thread::sleep(screenshot_delay);
                    std::fs::write(path, b"fake png").expect("write delayed fake screenshot");
                });
            }
            json!({"path": path})
        }
        "disconnect" => {
            *play_state.lock().expect("play lock") = false;
            json!({"disconnected": true})
        }
        other => json!({"unknown_command": other}),
    };

    json!({
        "id": request_id,
        "ok": true,
        "payload": payload,
        "error": null,
    })
}

fn wait_play_payload(in_play: bool) -> Value {
    if in_play {
        json!({
            "in_play": true,
            "dimension": "minecraft:overworld",
            "player": {"x": 0.5, "y": 64.0, "z": 0.5},
        })
    } else {
        json!({
            "in_play": false,
            "dimension": "",
            "player": null,
        })
    }
}

fn server_session_release_logged(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|log| log.contains("saved player state player=SolarisAgent"))
}

fn write_json_response(stream: &mut TcpStream, response: &Value) {
    write_json_response_with_status(stream, response, 200);
}

fn write_json_response_with_status(stream: &mut TcpStream, response: &Value, status: u16) {
    let body = serde_json::to_vec(response).expect("encode response");
    let reason = if status == 200 {
        "OK"
    } else {
        "Internal Server Error"
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .and_then(|()| stream.write_all(&body))
        .expect("write response");
}
