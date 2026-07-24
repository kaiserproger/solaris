use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use mc_test_harness::replay::{
    ReplayDriver, ReplayOutcome, ReplayRunResult, ReplayScenarioManifest,
};
use serde_json::{Value, json};

#[test]
fn real_client_gate_rejects_runs_without_gradle_runtime_classes_provenance() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");

    std::fs::copy(
        repo_root.join("docs/real-client-regression/manifests/m94-regression-pack.json"),
        run_dir.path().join("manifest.json"),
    )
    .expect("copy real-client manifest");
    std::fs::create_dir(run_dir.path().join("screenshots")).expect("create screenshots dir");
    for artifact in ["client.log", "server.log", "git.txt", "toolchain.txt"] {
        std::fs::write(run_dir.path().join(artifact), "").expect("write required artifact");
    }
    std::fs::write(
        run_dir.path().join("observations.json"),
        r#"{
  "schema": "solaris.real_client_observations.v1",
  "client_gate": "agent-run-real-client",
  "quality_label": "stabilization",
  "result": "passed",
  "scenarios": [{
    "id": "m94-02b-rejected-block-resync",
    "result": "passed",
    "screenshots": ["screenshots/m94-02b-rejected-block-resync.png"]
  }]
}"#,
    )
    .expect("write completed observations");
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        "client_kind=gradle-runclient\n\
client_adapter_source=auto-gradle-runclient\n\
client_adapter_task=:fabric-agent:runClientAgent\n\
client_agent_driver_exit_status=0\n\
client_agent_phase_exit_status_m94-02b-rejected-block-resync=0\n\
client_agent_bridge_wait_status_primary=ready\n",
    )
    .expect("write pre-provenance automation driver");

    let output = Command::new("bash")
        .arg(repo_root.join("tools/run-real-client-regression.sh"))
        .arg("--validate-run")
        .arg(run_dir.path())
        .output()
        .expect("run real-client validator");

    assert!(
        !output.status.success(),
        "validator accepted a run without loaded runtime provenance\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("runtime classes provenance"),
        "validator must reject missing Gradle runtime provenance before unrelated artifacts\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agent_driver_executes_checked_core_replay_manifest_and_emits_valid_result() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    std::fs::write(
        run_dir.path().join("server.toml"),
        "[network]\nview_distance = 8\n",
    )
    .expect("write replay config fingerprint input");
    std::fs::write(
        run_dir.path().join("client.log"),
        "fake Gradle runClient log\n",
    )
    .expect("write replay client evidence");
    let manifest_path = repo_root.join("tools/core-replay-scenarios/core-actions-seed-81.json");
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
        .arg("core-actions-seed-81")
        .arg("--replay-manifest")
        .arg(&manifest_path)
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client core replay driver");

    assert!(
        output.status.success(),
        "core replay driver failed\nstdout:\n{}\nstderr:\n{}",
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
            "wait_ticks",
            "state",
            "move_by",
            "state",
            "look",
            "state",
            "wait_ticks",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert_eq!(requests[3]["payload"], json!({"ticks": 2}));
    assert_eq!(requests[5]["payload"], json!({"dx_cm": 100, "dz_cm": -50}));
    assert_eq!(
        requests[7]["payload"],
        json!({"yaw_deg": 90, "pitch_deg": 0})
    );
    assert_eq!(requests[9]["payload"], json!({"ticks": 4}));

    let scenario = ReplayScenarioManifest::from_json(
        &std::fs::read_to_string(&manifest_path).expect("read checked replay manifest"),
    )
    .expect("checked replay manifest parses");
    let result_path = run_dir.path().join("core-replay-result.json");
    let result = ReplayRunResult::from_json(
        &std::fs::read_to_string(&result_path).expect("core replay result exists"),
    )
    .expect("core replay result parses");
    result
        .validate_against(&scenario)
        .expect("real-client result cross-validates against checked manifest");
    assert_eq!(result.driver, ReplayDriver::RealClient);
    assert_eq!(result.outcome, ReplayOutcome::Degraded);
    assert_eq!(result.actions, scenario.actions);
    assert_eq!(result.observations.len(), 1);
    assert!(result.observations[0].facts().iter().any(|fact| matches!(
        fact,
        mc_test_harness::parity::ObservationFact::Note { key, value }
            if key == "post_action_liveness" && value == "client_play_state"
    )));
    let validator = Command::new(env!("CARGO_BIN_EXE_core-replay-validate"))
        .arg(&manifest_path)
        .arg(&result_path)
        .output()
        .expect("run core replay result validator");
    assert!(
        validator.status.success(),
        "core replay validator failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&validator.stdout),
        String::from_utf8_lossy(&validator.stderr)
    );
    assert!(
        String::from_utf8_lossy(&validator.stdout)
            .contains("CORE_REPLAY_RESULT_VALID scenario=core-actions-seed-81")
    );

    let observations: Value = serde_json::from_slice(
        &std::fs::read(run_dir.path().join("observations.json"))
            .expect("real-client observations exist"),
    )
    .expect("real-client observations parse");
    assert_eq!(observations["result"], "passed");
    assert_eq!(observations["scenarios"][0]["id"], "core-actions-seed-81");
}

#[test]
fn agent_driver_rejects_malformed_core_replay_before_bridge_rpcs() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let mut manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(
            repo_root.join("tools/core-replay-scenarios/core-actions-seed-81.json"),
        )
        .expect("read checked replay manifest"),
    )
    .expect("checked replay manifest parses");
    manifest["actions"][0]["silent_default"] = json!(true);
    let manifest_path = run_dir.path().join("malformed-replay.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode malformed manifest"),
    )
    .expect("write malformed manifest");
    let bridge = FakeBridge::start(1);

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
        .arg("--secret")
        .arg("test-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("core-actions-seed-81")
        .arg("--replay-manifest")
        .arg(&manifest_path)
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run malformed real-client core replay driver");

    assert!(
        !output.status.success(),
        "driver accepted malformed replay manifest\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        bridge.join().is_empty(),
        "malformed replay manifest must fail before bridge RPCs"
    );
    let observations: Value = serde_json::from_slice(
        &std::fs::read(run_dir.path().join("observations.json"))
            .expect("failed observations exist"),
    )
    .expect("failed observations parse");
    assert_eq!(observations["result"], "failed");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(
                |message| message.contains("unknown") && message.contains("silent_default")
            )
    );
}

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
fn agent_driver_runs_generated_ruin_cache_phases_without_screenshots() {
    let repo_root = repo_root();
    for scenario_id in [
        "playable-46-generated-ruin-cache-before",
        "playable-46-generated-ruin-cache-after",
    ] {
        let run_dir = tempfile::tempdir().expect("create run dir");
        let bridge = FakeBridge::start(5);

        let output = Command::new("python3")
            .arg(repo_root.join("tools/real-client-agent-driver.py"))
            .arg("--bridge-url")
            .arg(format!("http://127.0.0.1:{}/rpc", bridge.port))
            .arg("--secret")
            .arg("test-secret")
            .arg("--run-dir")
            .arg(run_dir.path())
            .arg("--scenario")
            .arg(scenario_id)
            .arg("--server-addr")
            .arg("127.0.0.1:25565")
            .arg("--timeout-seconds")
            .arg("3")
            .output()
            .expect("run playable-46 real-client agent driver");

        assert!(
            output.status.success(),
            "driver failed for {scenario_id}\\nstdout:\\n{}\\nstderr:\\n{}",
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
            ["ping", "wait_play", "run_scenario", "state", "disconnect"]
        );
        assert_eq!(requests[2]["payload"]["id"], scenario_id);

        let observations: Value = serde_json::from_slice(
            &std::fs::read(run_dir.path().join("observations.json"))
                .expect("observations.json exists"),
        )
        .expect("observations.json is valid JSON");
        assert_eq!(observations["result"], "passed");
        assert_eq!(observations["scenarios"][0]["id"], scenario_id);
        assert_eq!(observations["scenarios"][0]["screenshots"], json!([]));
    }
}

#[test]
fn agent_driver_rejects_non_loopback_server_addr() {
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
        .arg("example.com:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        !output.status.success(),
        "driver accepted a non-loopback server_addr\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("failed observations.json exists"),
    )
    .expect("failed observations.json is valid JSON");
    assert_eq!(observations["result"], "failed");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("server_addr") && message.contains("loopback")),
        "failed observations must report loopback server_addr policy\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = bridge.join();
    assert!(
        requests.is_empty(),
        "driver must reject non-loopback server_addr before bridge RPCs"
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
                "state_version": 1,
                "in_play": true,
                "dimension": "minecraft:overworld",
                "current_screen": "net.minecraft.client.gui.screens.LevelLoadingScreen",
                "disconnect_reason": "",
            }),
            json!({
                "state_version": 2,
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
            "wait_state_change",
            "run_scenario",
            "state",
            "screenshot",
            "disconnect"
        ],
        "driver must wait for the vanilla client to leave LevelLoadingScreen before Java-agent scenarios"
    );
}

#[test]
fn interactive_pause_close_waits_for_the_client_thread_response() {
    let repo_root = repo_root();
    let driver = repo_root.join("tools/real-client-agent-driver.py");
    let probe = r#"
import importlib.util
import pathlib
import sys

driver_path = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("solaris_real_client_driver", driver_path)
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)

driver.time.monotonic = lambda: 0.0
calls = []

def call_and_record(client, transcript, command, payload, timeout_seconds, actor=None):
    calls.append((command, timeout_seconds))
    if command == "close_screen":
        if timeout_seconds < 10.0:
            raise RuntimeError("simulated busy Minecraft client thread")
        return {"status": "ok"}
    if command == "state":
        return {
            "state_version": 2,
            "in_play": True,
            "current_screen": "none",
        }
    raise AssertionError(f"unexpected command {command}")

driver.call_and_record = call_and_record
result = driver.wait_for_interactive_play(
    object(),
    [],
    {
        "state_version": 1,
        "in_play": True,
        "current_screen": "net.minecraft.client.gui.screens.PauseScreen",
    },
    30.0,
)

assert result["current_screen"] == "none", result
assert calls[0][0] == "close_screen", calls
assert calls[0][1] >= 10.0, calls
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(probe)
        .arg(driver)
        .output()
        .expect("run interactive pause-close probe");

    assert!(
        output.status.success(),
        "pause-close probe failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agent_driver_waits_for_async_screenshot_file() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_deferred_screenshot(6);

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
fn agent_driver_rejects_invalid_screenshot_artifact() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let bridge = FakeBridge::start_with_screenshot_bytes(5, b"fake png");

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
        "driver accepted an invalid screenshot artifact\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("failed observations.json exists"),
    )
    .expect("failed observations.json is valid JSON");
    assert_eq!(observations["result"], "failed");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("invalid PNG")),
        "failed observations must report invalid PNG screenshot bytes\nstdout:\n{}\nstderr:\n{}",
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
        ["ping", "wait_play", "run_scenario", "state", "screenshot"]
    );
}

#[test]
fn agent_driver_rejects_passed_broad_blocked_only_scenario() {
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
        .arg("m94-02-blocks-fluids-farming-drops")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        !output.status.success(),
        "driver accepted a passed broad blocked-only scenario\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("failed observations.json exists"),
    )
    .expect("failed observations.json is valid JSON");
    assert_eq!(observations["result"], "failed");
    assert_ne!(observations["result"], "passed");
    assert!(
        observations["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("blocked-only")),
        "failed observations must report blocked-only broad scenario policy\nstdout:\n{}\nstderr:\n{}",
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
    assert_eq!(requests[3]["payload"]["ticks"], 15);
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
fn agent_driver_coordinates_two_real_client_bridges_for_playable_38_inventory_drop_handoff() {
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
        .arg("playable-38-two-client-inventory-drop-handoff")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client inventory-drop driver failed\nstdout:\n{}\nstderr:\n{}",
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
            "playable-38-two-client-inventory-drop-primary",
            "playable-38-two-client-inventory-drop-gone-observe"
        ]
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary inventory-drop bridge requests must use only the primary secret"
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
            "playable-38-two-client-inventory-drop-observe",
            "playable-38-two-client-inventory-drop-secondary-pickup"
        ]
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary inventory-drop bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "playable-38-two-client-inventory-drop-handoff"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "secondary bridge scenario=playable-38-two-client-inventory-drop-secondary-pickup result=passed"
                    )))
            ),
        "combined report must record secondary inventory-drop pickup observation"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "primary bridge scenario=playable-38-two-client-inventory-drop-gone-observe result=passed"
                    )))
            ),
        "combined report must record primary inventory-drop removal observation"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "inventory-drop handoff run must attach both client screenshots"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_playable_39_short_soak() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(17, "passed");
    let secondary = FakeBridge::start_with_scenario_result(17, "passed");

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
        .arg("playable-39-two-client-short-soak")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client short-soak driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    let primary_commands: Vec<_> = primary_requests
        .iter()
        .map(|request| request["command"].as_str().expect("command"))
        .collect();
    assert_eq!(
        primary_commands,
        [
            "ping",
            "wait_play",
            "state",
            "move_forward",
            "state",
            "move_forward",
            "state",
            "move_forward",
            "state",
            "move_forward",
            "state",
            "move_forward",
            "state",
            "move_forward",
            "state",
            "screenshot",
            "disconnect"
        ]
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary short-soak bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    let secondary_commands: Vec<_> = secondary_requests
        .iter()
        .map(|request| request["command"].as_str().expect("command"))
        .collect();
    assert_eq!(secondary_commands, primary_commands);
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary short-soak bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "playable-39-two-client-short-soak"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(|entries| entries.iter().any(|entry| entry
                .as_str()
                .is_some_and(|text| text.contains("two-client short soak: passed pulses=6")))),
        "combined report must record short-soak liveness pulse coverage"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "short-soak run must attach both client screenshots"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_playable_40_chunk_crossing() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(29, "passed");
    let secondary = FakeBridge::start_with_scenario_result(29, "passed");

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
        .arg("playable-40-two-client-chunk-stream-crossing")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client chunk-crossing driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    assert_eq!(
        primary_requests
            .iter()
            .filter(|request| request["command"] == "move_forward")
            .count(),
        12,
        "primary must perform enough movement pulses to cross chunk boundaries"
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary chunk-crossing bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    assert_eq!(
        secondary_requests
            .iter()
            .filter(|request| request["command"] == "move_forward")
            .count(),
        12,
        "secondary must perform enough movement pulses to cross chunk boundaries"
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary chunk-crossing bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "playable-40-two-client-chunk-stream-crossing"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(|entries| entries
                .iter()
                .any(|entry| entry.as_str().is_some_and(|text| text
                    .contains("two-client chunk crossing: passed pulses=12 min_chunk_delta=1")))),
        "combined report must record chunk-crossing pulse and chunk coverage"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "chunk-crossing run must attach both client screenshots"
    );
}

#[test]
fn agent_driver_coordinates_two_real_client_bridges_for_playable_41_chunk_prewarm_crossing() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let primary = FakeBridge::start_with_scenario_result(29, "passed");
    let secondary = FakeBridge::start_with_scenario_result(29, "passed");

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
        .arg("playable-41-two-client-chunk-prewarm-crossing")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run real-client agent driver");

    assert!(
        output.status.success(),
        "two-client chunk-prewarm driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let primary_requests = primary.join();
    assert_eq!(
        primary_requests
            .iter()
            .filter(|request| request["command"] == "move_forward")
            .count(),
        12,
        "primary must perform enough movement pulses to hit prewarmed chunk boundaries"
    );
    assert!(
        primary_requests
            .iter()
            .all(|request| request["secret"] == "primary-secret"),
        "primary chunk-prewarm bridge requests must use only the primary secret"
    );

    let secondary_requests = secondary.join();
    assert_eq!(
        secondary_requests
            .iter()
            .filter(|request| request["command"] == "move_forward")
            .count(),
        12,
        "secondary must perform enough movement pulses to hit prewarmed chunk boundaries"
    );
    assert!(
        secondary_requests
            .iter()
            .all(|request| request["secret"] == "secondary-secret"),
        "secondary chunk-prewarm bridge requests must use only the secondary secret"
    );

    let observations_path = run_dir.path().join("observations.json");
    let observations: Value = serde_json::from_slice(
        &std::fs::read(&observations_path).expect("observations.json exists"),
    )
    .expect("observations.json is valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"][0]["id"],
        "playable-41-two-client-chunk-prewarm-crossing"
    );
    assert!(
        observations["scenarios"][0]["agent_report"]["observations"]
            .as_array()
            .is_some_and(
                |entries| entries.iter().any(|entry| entry
                    .as_str()
                    .is_some_and(|text| text.contains(
                        "two-client chunk prewarm crossing: passed pulses=12 min_chunk_delta=1"
                    )))
            ),
        "combined report must record chunk-prewarm pulse and chunk coverage"
    );
    assert!(
        observations["scenarios"][0]["screenshots"]
            .as_array()
            .is_some_and(|screenshots| screenshots.len() == 2),
        "chunk-prewarm run must attach both client screenshots"
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

#[test]
fn agent_driver_coordinates_two_client_shared_chest_across_restart_phases() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    std::fs::write(
        run_dir
            .path()
            .join("playable-31-shared-chest-marker.properties"),
        "x=1\ny=65\nz=1\nface=up\nitem=minecraft:oak_planks\ncount=1\n",
    )
    .expect("write P45 shared chest marker");
    let primary = FakeBridge::start_with_options(
        14,
        vec![wait_play_payload(true), wait_play_payload(false)],
        false,
        None,
        "passed",
        VALID_PNG_1X1,
    );
    let secondary = FakeBridge::start_with_options(
        13,
        vec![wait_play_payload(true), wait_play_payload(false)],
        false,
        None,
        "passed",
        VALID_PNG_1X1,
    );

    let run_phase = |scenario: &str, append: bool| {
        let mut command = Command::new("python3");
        command
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
            .arg(scenario)
            .arg("--server-addr")
            .arg("127.0.0.1:25565")
            .arg("--timeout-seconds")
            .arg("3");
        if append {
            command.arg("--append-observations");
        }
        command.output().expect("run P45 driver phase")
    };

    let before = run_phase(
        "playable-45-two-client-shared-chest-save-restart-before",
        false,
    );
    assert!(
        before.status.success(),
        "P45 before phase failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&before.stdout),
        String::from_utf8_lossy(&before.stderr)
    );
    let after = run_phase(
        "playable-45-two-client-shared-chest-save-restart-after",
        true,
    );
    assert!(
        after.status.success(),
        "P45 after phase failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&after.stdout),
        String::from_utf8_lossy(&after.stderr)
    );

    let primary_requests = primary.join();
    let primary_scenarios: Vec<_> = primary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| {
            request["payload"]["id"]
                .as_str()
                .expect("primary scenario id")
        })
        .collect();
    assert_eq!(
        primary_scenarios,
        [
            "playable-31-two-client-earned-shared-chest-deposit",
            "playable-31-two-client-earned-shared-chest-observe-empty",
        ]
    );
    assert_eq!(
        primary_requests
            .iter()
            .filter(|request| request["command"] == "connect")
            .count(),
        1,
        "primary must reconnect after the restart boundary"
    );

    let secondary_requests = secondary.join();
    let secondary_scenarios: Vec<_> = secondary_requests
        .iter()
        .filter(|request| request["command"] == "run_scenario")
        .map(|request| {
            request["payload"]["id"]
                .as_str()
                .expect("secondary scenario id")
        })
        .collect();
    assert_eq!(
        secondary_scenarios,
        ["playable-31-two-client-earned-shared-chest-withdraw"]
    );
    assert_eq!(
        secondary_requests
            .iter()
            .filter(|request| request["command"] == "connect")
            .count(),
        1,
        "secondary must reconnect after the restart boundary"
    );

    let observations: Value = serde_json::from_slice(
        &std::fs::read(run_dir.path().join("observations.json")).expect("P45 observations exist"),
    )
    .expect("P45 observations are valid JSON");
    assert_eq!(observations["result"], "passed");
    assert_eq!(
        observations["scenarios"]
            .as_array()
            .expect("P45 scenarios array")
            .len(),
        2
    );
    assert_eq!(
        observations["scenarios"][0]["id"],
        "playable-45-two-client-shared-chest-save-restart-before"
    );
    assert_eq!(
        observations["scenarios"][1]["id"],
        "playable-45-two-client-shared-chest-save-restart-after"
    );
    assert_eq!(
        observations["scenarios"][1]["agent_report"]["restart_invariant_validation"]["status"],
        "passed"
    );
    assert_eq!(
        observations["scenarios"][1]["agent_report"]["restart_invariant_validation"]["checks"]
            .as_array()
            .expect("restart invariant checks array")
            .len(),
        6
    );

    let snapshot: Value = serde_json::from_slice(
        &std::fs::read(run_dir.path().join("restart-invariants.json"))
            .expect("restart invariant snapshot exists"),
    )
    .expect("restart invariant snapshot is valid JSON");
    assert_eq!(snapshot["schema"], "solaris.restart_invariants.v1");
    assert_eq!(
        snapshot["producer_scenario"],
        "playable-45-two-client-shared-chest-save-restart-before"
    );
    let invariants = snapshot["invariants"]
        .as_array()
        .expect("restart invariants array");
    assert_eq!(invariants.len(), 6);
    let categories: std::collections::BTreeSet<_> = invariants
        .iter()
        .map(|invariant| invariant["category"].as_str().expect("invariant category"))
        .collect();
    assert_eq!(
        categories,
        std::collections::BTreeSet::from(["container", "entity", "inventory", "player", "world",])
    );
}

#[test]
fn agent_driver_rejects_restart_snapshot_missing_typed_field_before_bridge_calls() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    std::fs::write(
        run_dir
            .path()
            .join("playable-31-shared-chest-marker.properties"),
        "x=1\ny=65\nz=1\nface=up\nitem=minecraft:oak_planks\ncount=1\n",
    )
    .expect("write P45 marker");
    std::fs::write(
        run_dir.path().join("restart-invariants.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "solaris.restart_invariants.v1",
            "producer_scenario": "playable-45-two-client-shared-chest-save-restart-before",
            "created_at": "2026-07-24T00:00:00Z",
            "invariants": [{
                "id": "player.primary.dimension",
                "type": "string",
                "mode": "stable",
                "before": "minecraft:overworld",
                "expected_after": "minecraft:overworld"
            }]
        }))
        .expect("serialize malformed restart snapshot"),
    )
    .expect("write malformed restart snapshot");
    let primary_port = reserve_then_release_port();
    let secondary_port = reserve_then_release_port();

    let output = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{primary_port}/rpc"))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{secondary_port}/rpc"))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("playable-45-two-client-shared-chest-save-restart-after")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("1")
        .arg("--append-observations")
        .output()
        .expect("run malformed restart snapshot phase");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid restart invariant snapshot") && stderr.contains("category"),
        "missing typed field must fail before bridge calls: {stderr}"
    );
    let observations: Value = serde_json::from_slice(
        &std::fs::read(run_dir.path().join("observations.json"))
            .expect("failed observations exist"),
    )
    .expect("failed observations parse");
    assert_eq!(observations["result"], "failed");
}

#[test]
fn agent_driver_rejects_restart_marker_drift_across_phases() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let marker_path = run_dir
        .path()
        .join("playable-31-shared-chest-marker.properties");
    std::fs::write(
        &marker_path,
        "x=1\ny=65\nz=1\nface=up\nitem=minecraft:oak_planks\ncount=1\n",
    )
    .expect("write original P45 marker");
    let primary = FakeBridge::start_with_options(
        6,
        vec![wait_play_payload(true)],
        false,
        None,
        "passed",
        VALID_PNG_1X1,
    );
    let secondary = FakeBridge::start_with_options(
        5,
        vec![wait_play_payload(true)],
        false,
        None,
        "passed",
        VALID_PNG_1X1,
    );

    let before = Command::new("python3")
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
        .arg("playable-45-two-client-shared-chest-save-restart-before")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("3")
        .output()
        .expect("run P45 before phase");
    assert!(before.status.success(), "before phase must pass");
    primary.join();
    secondary.join();

    std::fs::write(
        &marker_path,
        "x=2\ny=65\nz=1\nface=up\nitem=minecraft:oak_planks\ncount=1\n",
    )
    .expect("drift P45 marker");
    let primary_port = reserve_then_release_port();
    let secondary_port = reserve_then_release_port();
    let after = Command::new("python3")
        .arg(repo_root.join("tools/real-client-agent-driver.py"))
        .arg("--bridge-url")
        .arg(format!("http://127.0.0.1:{primary_port}/rpc"))
        .arg("--secret")
        .arg("primary-secret")
        .arg("--secondary-bridge-url")
        .arg(format!("http://127.0.0.1:{secondary_port}/rpc"))
        .arg("--secondary-secret")
        .arg("secondary-secret")
        .arg("--run-dir")
        .arg(run_dir.path())
        .arg("--scenario")
        .arg("playable-45-two-client-shared-chest-save-restart-after")
        .arg("--server-addr")
        .arg("127.0.0.1:25565")
        .arg("--timeout-seconds")
        .arg("1")
        .arg("--append-observations")
        .output()
        .expect("run P45 drifted after phase");

    assert!(!after.status.success());
    assert!(
        String::from_utf8_lossy(&after.stderr).contains("world.shared_chest_marker"),
        "marker drift must be reported as a typed invariant mismatch"
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
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FailingWaitPlayBridge {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing bridge");
        let port = listener.local_addr().expect("failing bridge addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            let mut requests = 0;
            while requests < 2 {
                let (mut stream, _) = listener.accept().expect("failing bridge accepts request");
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }

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
        });

        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }

    fn join(mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
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
    stop: Arc<AtomicBool>,
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
            false,
            None,
            "passed",
            VALID_PNG_1X1,
        )
    }

    fn start_with_deferred_screenshot(expected_requests: usize) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            true,
            None,
            "passed",
            VALID_PNG_1X1,
        )
    }

    fn start_with_screenshot_bytes(
        expected_requests: usize,
        screenshot_bytes: &'static [u8],
    ) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            false,
            None,
            "passed",
            screenshot_bytes,
        )
    }

    fn start_requiring_server_release_before_connect(
        expected_requests: usize,
        server_release_log_path: std::path::PathBuf,
    ) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            false,
            Some(server_release_log_path),
            "passed",
            VALID_PNG_1X1,
        )
    }

    fn start_with_scenario_result(expected_requests: usize, scenario_result: &'static str) -> Self {
        Self::start_with_options(
            expected_requests,
            vec![wait_play_payload(true)],
            false,
            None,
            scenario_result,
            VALID_PNG_1X1,
        )
    }

    fn start_with_options(
        expected_requests: usize,
        wait_play_results: Vec<Value>,
        defer_screenshot_until_response: bool,
        server_release_log_path: Option<std::path::PathBuf>,
        scenario_result: &'static str,
        screenshot_bytes: &'static [u8],
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake bridge");
        let port = listener.local_addr().expect("fake bridge addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let wait_play_results = Arc::new(Mutex::new(VecDeque::from(wait_play_results)));
        let thread_wait_play_results = Arc::clone(&wait_play_results);
        let movement_count = Arc::new(Mutex::new(0_u32));
        let thread_movement_count = Arc::clone(&movement_count);
        let replay_movement_cm = Arc::new(Mutex::new((0_i32, 0_i32)));
        let thread_replay_movement_cm = Arc::clone(&replay_movement_cm);
        let play_state = Arc::new(Mutex::new(true));
        let thread_play_state = Arc::clone(&play_state);

        let handle = thread::spawn(move || {
            loop {
                if thread_requests.lock().expect("requests lock").len() >= expected_requests {
                    break;
                }

                let (mut stream, _) = listener.accept().expect("fake bridge accepts request");
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }

                let request = read_json_request(&mut stream);
                let deferred_screenshot_path = defer_screenshot_until_response
                    .then(|| request["command"].as_str())
                    .flatten()
                    .filter(|command| *command == "screenshot")
                    .map(|_| {
                        request["payload"]["path"]
                            .as_str()
                            .expect("screenshot path")
                            .to_owned()
                    });
                let response = bridge_response(
                    &request,
                    &thread_wait_play_results,
                    &thread_movement_count,
                    &thread_replay_movement_cm,
                    &thread_play_state,
                    BridgeResponseOptions {
                        defer_screenshot_until_response,
                        server_release_log_path: server_release_log_path.as_deref(),
                        scenario_result,
                        screenshot_bytes,
                    },
                );
                thread_requests.lock().expect("requests lock").push(request);
                write_json_response(&mut stream, &response);
                if let Some(path) = deferred_screenshot_path {
                    std::fs::write(path, screenshot_bytes).expect("write deferred fake screenshot");
                }
            }
        });

        Self {
            port,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn join(mut self) -> Vec<Value> {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
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
    replay_movement_cm: &Mutex<(i32, i32)>,
    play_state: &Mutex<bool>,
    options: BridgeResponseOptions<'_>,
) -> Value {
    let request_id = request["id"].as_u64().expect("request id");
    if request["command"].as_str() == Some("connect")
        && options
            .server_release_log_path
            .is_some_and(|path| !server_session_release_logged(path))
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
        "wait_play" | "wait_state_change" => wait_play_results
            .lock()
            .expect("wait_play lock")
            .pop_front()
            .unwrap_or_else(|| wait_play_payload(true)),
        "move_forward" => {
            *movement_count.lock().expect("movement lock") += 1;
            json!({"status": "ok"})
        }
        "move_by" => {
            let mut movement = replay_movement_cm.lock().expect("replay movement lock");
            movement.0 += request["payload"]["dx_cm"].as_i64().expect("move_by dx_cm") as i32;
            movement.1 += request["payload"]["dz_cm"].as_i64().expect("move_by dz_cm") as i32;
            json!({"status": "ok"})
        }
        "wait_ticks" | "look" => json!({"status": "ok"}),
        "run_scenario" => json!({
            "result": options.scenario_result,
            "id": request["payload"]["id"],
            "observations": [
                "fake bridge executed the scenario action path"
            ],
        }),
        "state" => {
            let movement_count = *movement_count.lock().expect("movement lock");
            let replay_movement = *replay_movement_cm.lock().expect("replay movement lock");
            let in_play = *play_state.lock().expect("play lock");
            json!({
                "state_version": 1,
                "in_play": in_play,
                "dimension": if in_play { "minecraft:overworld" } else { "" },
                "x": 0.5 + f64::from(replay_movement.0) / 100.0,
                "y": 64.0,
                "z": 0.5 + f64::from(movement_count) * 3.0
                    + f64::from(replay_movement.1) / 100.0,
                "selected_hotbar_slot": 0,
                "screen": null,
            })
        }
        "screenshot" => {
            let path = request["payload"]["path"]
                .as_str()
                .expect("screenshot path");
            if !options.defer_screenshot_until_response {
                std::fs::write(path, options.screenshot_bytes).expect("write fake screenshot");
            }
            json!({"path": path})
        }
        "disconnect" => {
            *play_state.lock().expect("play lock") = false;
            if let Some(path) = options.server_release_log_path {
                std::fs::write(
                    path,
                    "2026-06-22T14:10:54.741091Z  INFO mc_net::play: saved player state player=SolarisAgent state=pos=(-2.31,80.00,-6.58)\n",
                )
                .expect("write server release marker after disconnect");
            }
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

struct BridgeResponseOptions<'a> {
    defer_screenshot_until_response: bool,
    server_release_log_path: Option<&'a std::path::Path>,
    scenario_result: &'static str,
    screenshot_bytes: &'static [u8],
}

const VALID_PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

fn wait_play_payload(in_play: bool) -> Value {
    if in_play {
        json!({
            "state_version": 1,
            "in_play": true,
            "dimension": "minecraft:overworld",
            "player": {"x": 0.5, "y": 64.0, "z": 0.5},
        })
    } else {
        json!({
            "state_version": 1,
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
