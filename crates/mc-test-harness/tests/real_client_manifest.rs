use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use mc_test_harness::replay::ReplayScenarioManifest;
use serde_json::{Value, json};

const M94_MANIFEST: &str =
    include_str!("../../../docs/real-client-regression/manifests/m94-regression-pack.json");

#[test]
fn playable_real_client_manifest_tracks_no_debug_twenty_minute_loop() {
    let manifest_path = repo_root().join("docs/playable/real-client-playable-loop.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    let manifest: Value = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|err| panic!("parse {}: {err}", manifest_path.display()));

    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["pack_id"], "playable-real-client-loop");
    assert_eq!(manifest["quality_label"], "playable-spike");
    assert_eq!(
        manifest["server_command"],
        "cargo run --bin mc-server -- --config playable.toml"
    );
    assert!(
        manifest["client_requirement"]
            .as_str()
            .is_some_and(|requirement| {
                requirement.contains("vanilla 26.1.2")
                    && requirement.contains("real client")
                    && requirement.contains("protocol harnesses do not satisfy")
            }),
        "playable client gate must require a real vanilla client"
    );
    assert_eq!(
        manifest["automation_runner"]["script"].as_str(),
        Some("tools/run-playable-client-gate.sh")
    );
    assert_eq!(
        manifest["automation_runner"]["delegates_to"].as_str(),
        Some("tools/run-real-client-regression.sh")
    );
    assert_eq!(
        manifest["automation_runner"]["server_config_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_SERVER_CONFIG")
    );
    assert_eq!(
        manifest["automation_runner"]["server_config_value"].as_str(),
        Some("playable.toml")
    );

    let forbidden = manifest["forbidden_client_evidence"]
        .as_array()
        .expect("forbidden client evidence list is present");
    for forbidden_client in [
        "wire-probe",
        "mc_test_harness::client::Client",
        "protocol-only bot",
        "debug commands",
    ] {
        assert!(
            forbidden.iter().any(|entry| entry == forbidden_client),
            "playable manifest must exclude {forbidden_client}"
        );
    }

    let scenarios = manifest["scenarios"]
        .as_array()
        .expect("scenarios are present");
    let mut scenario_ids = BTreeSet::new();
    for scenario in scenarios {
        let id = scenario["id"].as_str().expect("scenario id is present");
        assert!(scenario_ids.insert(id), "duplicate scenario id {id}");
        assert_eq!(scenario["status"], "manual-pending");
        assert_eq!(scenario["screenshots_required"], true);
        assert!(
            scenario["steps"]
                .as_array()
                .is_some_and(|steps| !steps.is_empty()),
            "scenario {id} needs runnable steps"
        );
        assert!(
            scenario["expected_observations"]
                .as_array()
                .is_some_and(|observations| !observations.is_empty()),
            "scenario {id} needs expected observations"
        );
    }

    assert!(scenario_ids.contains("playable-01-join-generated-spawn"));
    assert!(scenario_ids.contains("playable-02-natural-wood-to-tool"));
    assert!(scenario_ids.contains("playable-02a-natural-log-to-planks"));
    assert!(scenario_ids.contains("playable-02b-natural-crafting-table-open"));
    assert!(scenario_ids.contains("playable-03-save-restart-rejoin"));
    assert!(scenario_ids.contains("playable-04-twenty-minute-survival-loop"));
    assert!(scenario_ids.contains("playable-05-stone-tool-progression"));
    assert!(scenario_ids.contains("playable-06-stone-tool-save-restart"));
    assert!(scenario_ids.contains("playable-06-stone-tool-save-restart-before"));
    assert!(scenario_ids.contains("playable-06-stone-tool-save-restart-after"));
    assert!(scenario_ids.contains("playable-07-furnace-placement-open"));
    assert!(scenario_ids.contains("playable-08-furnace-charcoal-smelt"));
    assert!(scenario_ids.contains("playable-09-torch-craft-place"));
    assert!(scenario_ids.contains("playable-10-passive-food-drop"));
    assert!(scenario_ids.contains("playable-11-eat-passive-food"));
    assert!(scenario_ids.contains("playable-12-earned-chest-storage"));
    assert!(scenario_ids.contains("playable-13-chest-storage-save-restart"));
    assert!(scenario_ids.contains("playable-13-chest-storage-save-restart-before"));
    assert!(scenario_ids.contains("playable-13-chest-storage-save-restart-after"));
    assert!(scenario_ids.contains("playable-14-earned-bed-sleep"));
    assert!(scenario_ids.contains("playable-15-cooked-passive-food"));
    assert!(scenario_ids.contains("playable-16-earned-door-place-toggle"));
    assert!(scenario_ids.contains("playable-17-earned-sign-place-edit"));
    assert!(scenario_ids.contains("playable-18-earned-campfire-cooking"));
    assert!(scenario_ids.contains("playable-19-earned-campfire-death-respawn"));
    assert!(scenario_ids.contains("playable-20-campfire-death-drop-recovery"));
    assert!(scenario_ids.contains("playable-21-earned-tool-zombie-combat"));
    assert!(scenario_ids.contains("playable-22-stone-sword-zombie-combat"));
    assert!(scenario_ids.contains("playable-23-iron-ingot-progression"));
    assert!(scenario_ids.contains("playable-24-iron-sword-zombie-combat"));
    assert!(scenario_ids.contains("playable-25-iron-sword-save-restart"));
    assert!(scenario_ids.contains("playable-25-iron-sword-save-restart-before"));
    assert!(scenario_ids.contains("playable-25-iron-sword-save-restart-after"));
    assert!(scenario_ids.contains("playable-26-earned-shield-zombie-block"));
    assert!(scenario_ids.contains("playable-27-earned-iron-chestplate-equip"));
    assert!(scenario_ids.contains("playable-28-earned-iron-chestplate-zombie-mitigation"));
    assert!(scenario_ids.contains("playable-29-iron-chestplate-save-restart-mitigation"));
    assert!(scenario_ids.contains("playable-29-iron-chestplate-save-restart-mitigation-before"));
    assert!(scenario_ids.contains("playable-29-iron-chestplate-save-restart-mitigation-after"));
    assert!(scenario_ids.contains("playable-30-two-client-shared-log-drop-pickup"));
    assert!(scenario_ids.contains("playable-30-two-client-shared-log-drop-break"));
    assert!(scenario_ids.contains("playable-30-two-client-shared-log-drop-observe"));
    assert!(scenario_ids.contains("playable-30-two-client-shared-log-pickup-collect"));
    assert!(scenario_ids.contains("playable-30-two-client-shared-log-pickup-gone-observe"));
    assert!(scenario_ids.contains("playable-31-two-client-earned-shared-chest"));
    assert!(scenario_ids.contains("playable-31-two-client-earned-shared-chest-deposit"));
    assert!(scenario_ids.contains("playable-31-two-client-earned-shared-chest-withdraw"));
    assert!(scenario_ids.contains("playable-31-two-client-earned-shared-chest-observe-empty"));
    assert!(scenario_ids.contains("playable-32-two-client-earned-torch-block-edit"));
    assert!(scenario_ids.contains("playable-32-two-client-earned-torch-place"));
    assert!(scenario_ids.contains("playable-32-two-client-earned-torch-observe"));
    assert!(scenario_ids.contains("playable-32-two-client-earned-torch-break"));
    assert!(scenario_ids.contains("playable-32-two-client-earned-torch-gone-observe"));
    assert!(scenario_ids.contains("playable-33-two-client-player-visibility-movement"));
    assert!(scenario_ids.contains("playable-33-two-client-player-observe"));
    assert!(scenario_ids.contains("playable-33-two-client-player-moved-observe"));
    assert!(scenario_ids.contains("playable-34-two-client-chat-message"));
    assert!(scenario_ids.contains("playable-34-two-client-chat-send"));
    assert!(scenario_ids.contains("playable-34-two-client-chat-observe"));
    assert!(scenario_ids.contains("playable-35-two-client-player-disconnect-removal"));
    assert!(scenario_ids.contains("playable-35-two-client-player-disconnect-visible"));
    assert!(scenario_ids.contains("playable-35-two-client-player-gone-observe"));
    assert!(scenario_ids.contains("playable-36-two-client-player-reconnect-cleanup"));
    assert!(scenario_ids.contains("playable-36-two-client-player-reconnect-visible"));
    assert!(scenario_ids.contains("playable-36-two-client-player-reconnect-gone-observe"));
    assert!(scenario_ids.contains("playable-36-two-client-player-reconnected-observe"));
    assert!(scenario_ids.contains("playable-37-two-client-player-death-respawn-visibility"));
    assert!(scenario_ids.contains("playable-37-two-client-player-death-baseline"));
    assert!(scenario_ids.contains("playable-37-two-client-campfire-death-respawn"));
    assert!(scenario_ids.contains("playable-37-two-client-player-post-respawn-moved-observe"));
    assert!(scenario_ids.contains("playable-38-two-client-inventory-drop-handoff"));
    assert!(scenario_ids.contains("playable-38-two-client-inventory-drop-primary"));
    assert!(scenario_ids.contains("playable-38-two-client-inventory-drop-observe"));
    assert!(scenario_ids.contains("playable-38-two-client-inventory-drop-secondary-pickup"));
    assert!(scenario_ids.contains("playable-38-two-client-inventory-drop-gone-observe"));
    assert!(scenario_ids.contains("playable-39-two-client-short-soak"));
    assert!(scenario_ids.contains("playable-40-two-client-chunk-stream-crossing"));
    assert!(scenario_ids.contains("playable-41-two-client-chunk-prewarm-crossing"));
    assert!(scenario_ids.contains("playable-42-two-client-opposite-chunk-crossing"));
    assert!(scenario_ids.contains("playable-43-renewable-wheat-bread"));
    assert!(scenario_ids.contains("playable-44-passive-livestock-motion"));
    assert!(scenario_ids.contains("playable-45-two-client-shared-chest-save-restart"));
    assert!(scenario_ids.contains("playable-45-two-client-shared-chest-save-restart-before"));
    assert!(scenario_ids.contains("playable-45-two-client-shared-chest-save-restart-after"));
    assert!(scenario_ids.contains("playable-48-wall-torch-stairs-slabs"));
}

#[test]
fn playable_real_client_runner_check_selects_gradle_adapter() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--check")
        .output()
        .expect("run playable real-client check");

    assert!(
        output.status.success(),
        "playable runner did not auto-select the repo-native runClient adapter\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("gradle-runclient"),
        "playable runner check should name the auto-selected adapter\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("scenario policy: no-debug, no operator privileges"),
        "playable runner must deny operator privileges for its no-debug scenario\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn real_client_runner_rejects_a_scenario_missing_from_the_manifest() {
    let repo_root = repo_root();
    let manifest_dir = tempfile::tempdir().expect("create manifest directory");
    let manifest_path = manifest_dir.path().join("single-scenario.json");
    std::fs::write(
        &manifest_path,
        r#"{
  "schema_version": 1,
  "pack_id": "runner-scenario-membership-test",
  "quality_label": "test-only",
  "scenarios": [{"id": "declared-scenario"}]
}"#,
    )
    .expect("write manifest fixture");

    let output = Command::new("bash")
        .arg(repo_root.join("tools/run-real-client-regression.sh"))
        .arg("--check")
        .env("SOLARIS_REAL_CLIENT_MANIFEST", manifest_path)
        .env("SOLARIS_REAL_CLIENT_AGENT_SCENARIO", "undeclared-scenario")
        .output()
        .expect("check undeclared scenario");

    assert!(
        !output.status.success(),
        "runner accepted a scenario outside its evidence manifest\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("missing from manifest"),
        "runner must explain the scenario/manifest mismatch\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn real_client_prepare_denies_operators_when_scenario_policy_is_unknown() {
    let repo_root = repo_root();
    let manifest_dir = tempfile::tempdir().expect("create manifest directory");
    let manifest_path = manifest_dir.path().join("single-scenario.json");
    let run_root = tempfile::tempdir().expect("create prepare run root");
    std::fs::write(
        &manifest_path,
        r#"{
  "schema_version": 1,
  "pack_id": "prepare-policy-test",
  "quality_label": "test-only",
  "scenarios": [{"id": "declared-scenario"}]
}"#,
    )
    .expect("write manifest fixture");

    let output = Command::new("bash")
        .arg(repo_root.join("tools/run-real-client-regression.sh"))
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_MANIFEST", manifest_path)
        .env("SOLARIS_REAL_CLIENT_AGENT_SCENARIO", "undeclared-scenario")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .output()
        .expect("prepare undeclared scenario");

    assert!(
        output.status.success(),
        "prepare may remain manifest-agnostic but must fail closed on privileges\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let effective_config =
        std::fs::read_to_string(Path::new(run_dir_text.trim()).join("server.toml"))
            .expect("prepared server config exists");
    let effective_config: toml::Value =
        toml::from_str(&effective_config).expect("prepared server config parses");
    assert_eq!(
        effective_config["admin"]["operators"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "unknown scenario policy must not grant operator privileges"
    );
}

#[test]
fn core_replay_real_client_gate_checks_structured_pack_with_gradle_runner() {
    let repo_root = repo_root();
    let pack_path =
        repo_root.join("docs/real-client-regression/manifests/core-replay-seed-81.json");
    let pack: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&pack_path).expect("read core replay real-client pack"),
    )
    .expect("core replay real-client pack parses");
    assert_eq!(pack["pack_id"], "core-replay-seed-81-real-client");
    assert_eq!(
        pack["core_replay_manifest"],
        "tools/core-replay-scenarios/core-actions-seed-81.json"
    );
    assert_eq!(pack["scenarios"][0]["id"], "core-actions-seed-81");
    assert_eq!(pack["scenarios"][0]["no_debug_commands"], true);
    assert_eq!(pack["scenarios"][0]["screenshots_required"], true);

    let wrapper_path = repo_root.join("tools/run-core-replay-client-gate.sh");
    let output = Command::new("bash")
        .arg(&wrapper_path)
        .arg("--check")
        .output()
        .expect("run core replay client check");
    assert!(
        output.status.success(),
        "core replay wrapper did not select Gradle runClient\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn core_replay_real_client_prepare_copies_canonical_manifest() {
    let repo_root = repo_root();
    let run_root = tempfile::tempdir().expect("create core replay run root");
    let wrapper_path = repo_root.join("tools/run-core-replay-client-gate.sh");

    let output = Command::new("bash")
        .arg(&wrapper_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .env(
            "SOLARIS_REAL_CLIENT_AGENT_SECRET",
            "core-replay-test-secret",
        )
        .output()
        .expect("prepare core replay real-client gate");
    assert!(
        output.status.success(),
        "core replay prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared_dir = PathBuf::from(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .last()
            .expect("prepared run path"),
    );
    let copied = std::fs::read(prepared_dir.join("core-replay-manifest.json"))
        .expect("copied core replay manifest exists");
    let canonical =
        std::fs::read(repo_root.join("tools/core-replay-scenarios/core-actions-seed-81.json"))
            .expect("canonical core replay manifest exists");
    assert_eq!(copied, canonical);
    let automation = std::fs::read_to_string(prepared_dir.join("automation-driver.txt"))
        .expect("read prepared automation metadata");
    assert!(automation.contains("core_replay_manifest=core-replay-manifest.json"));
    assert!(automation.contains("client_adapter_task=:fabric-agent:runClientAgent"));
}

#[test]
fn core_replay_core_gate_manifest_supports_compact_ledger_rows_and_evidence_legs() {
    let repo_root = repo_root();
    let core_manifest = repo_root.join("tools/core-replay-scenarios/core-actions-seed-81.json");
    let mut scenario: Value = serde_json::from_slice(
        &std::fs::read(&core_manifest).expect("read core replay scenario manifest"),
    )
    .expect("core replay scenario manifest parses");

    let mut compact = scenario.clone();
    compact["ledger_rows"] = json!(["Q1", "Q2", "Q3"]);
    compact["evidence_legs"] = json!([
        "protocol-session",
        "oracle-comparison",
        "real-client-observation"
    ]);
    ReplayScenarioManifest::from_json(&compact.to_string())
        .expect("core replay scenario supports compact evidence contract");

    let mut bad = scenario.clone();
    bad["ledger_rows"] = json!(["Q1", "Q2"]);
    bad["evidence_legs"] = json!(["protocol-session"]);
    assert!(
        ReplayScenarioManifest::from_json(&bad.to_string()).is_err(),
        "compact rows and evidence legs mismatch must fail"
    );
    assert!(compact["id"].as_str().is_some_and(|id| id == "core-actions-seed-81"));
}

#[test]
fn playable_real_client_prepare_uses_playable_manifest_config_and_scenario() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");
    let run_root = tempfile::tempdir().expect("create playable run root");

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .output()
        .expect("prepare playable real-client gate");

    assert!(
        output.status.success(),
        "playable prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let run_dir = Path::new(run_dir_text.trim());
    let automation_driver = std::fs::read_to_string(run_dir.join("automation-driver.txt"))
        .expect("read automation driver");
    assert!(
        automation_driver.contains("playable.toml"),
        "prepared playable gate must use playable.toml\n{automation_driver}"
    );
    assert!(
        automation_driver.contains("client_agent_scenario=playable-04-twenty-minute-survival-loop"),
        "prepared playable gate must default to the full loop scenario\n{automation_driver}"
    );
    let effective_server_config = std::fs::read_to_string(run_dir.join("server.toml"))
        .expect("prepared playable gate writes its effective server config");
    let effective_server_config: toml::Value = toml::from_str(&effective_server_config)
        .expect("prepared playable server config parses as TOML");
    assert_eq!(
        effective_server_config["simulation"]["spawn_monsters"].as_bool(),
        Some(false),
        "the continuity soak must isolate natural hostile pressure from its separate combat gate"
    );
    assert!(
        automation_driver
            .contains("client_gate=agent-run real-client configured: gradle-runclient")
            && automation_driver.contains("client_kind=gradle-runclient")
            && automation_driver.contains("client_adapter_source=auto-gradle-runclient")
            && automation_driver.contains("client_adapter_task=:fabric-agent:runClientAgent")
            && !automation_driver
                .lines()
                .any(|line| line.contains(&legacy_primary_launcher_metadata_key())),
        "prepared playable gate must auto-select the Gradle runClient adapter\n{automation_driver}"
    );

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(run_dir.join("manifest.json")).expect("read prepared playable manifest"),
    )
    .expect("parse prepared playable manifest");
    assert_eq!(manifest["pack_id"], "playable-real-client-loop");
    assert_eq!(manifest["quality_label"], "playable-spike");
}

#[test]
fn playable_combat_prepare_keeps_natural_monsters_enabled() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");
    let run_root = tempfile::tempdir().expect("create playable combat run root");

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .env(
            "SOLARIS_REAL_CLIENT_AGENT_SCENARIO",
            "playable-21-earned-tool-zombie-combat",
        )
        .output()
        .expect("prepare playable combat gate");

    assert!(
        output.status.success(),
        "playable combat prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let run_dir = Path::new(run_dir_text.trim());
    let effective_server_config = std::fs::read_to_string(run_dir.join("server.toml"))
        .expect("prepared combat gate writes its effective server config");
    let effective_server_config: toml::Value = toml::from_str(&effective_server_config)
        .expect("prepared combat server config parses as TOML");
    assert_eq!(
        effective_server_config["simulation"]["spawn_monsters"].as_bool(),
        Some(true),
        "combat scenarios must retain natural hostile spawning"
    );
}

#[test]
fn playable_real_client_prepare_ignores_legacy_client_command_env() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");
    let run_root = tempfile::tempdir().expect("create playable run root");
    let legacy_command = "echo legacy-client-command-must-not-run";
    let legacy_command_env = legacy_primary_client_env_names()
        .into_iter()
        .next()
        .expect("legacy primary client command env is listed");

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .env(&legacy_command_env, legacy_command)
        .output()
        .expect("prepare playable real-client gate with legacy command env");

    assert!(
        output.status.success(),
        "playable prepare rejected an ignored legacy command env\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let run_dir = Path::new(run_dir_text.trim());
    let automation_driver = std::fs::read_to_string(run_dir.join("automation-driver.txt"))
        .expect("read automation driver");

    assert!(
        automation_driver.contains("client_kind=gradle-runclient")
            && automation_driver.contains("client_adapter_source=auto-gradle-runclient")
            && automation_driver.contains("client_adapter_task=:fabric-agent:runClientAgent")
            && !automation_driver.contains(&legacy_command_env)
            && !automation_driver.contains(legacy_command)
            && !automation_driver
                .lines()
                .any(|line| line.contains(&legacy_primary_launcher_metadata_key())),
        "legacy client command env must not affect the fixed Gradle runClient adapter\n{automation_driver}"
    );
}

#[test]
fn playable_real_client_prepare_picks_free_agent_port_when_default_is_busy() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");
    let run_root = tempfile::tempdir().expect("create playable run root");
    let _busy_default = std::net::TcpListener::bind(("127.0.0.1", 39094)).ok();

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .env("SOLARIS_REAL_CLIENT_AGENT_SECRET", "s_test_prepare")
        .output()
        .expect("prepare playable real-client gate");

    assert!(
        output.status.success(),
        "playable prepare with agent secret failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let run_dir = Path::new(run_dir_text.trim());
    let automation_driver = std::fs::read_to_string(run_dir.join("automation-driver.txt"))
        .expect("read automation driver");
    let port = automation_driver
        .lines()
        .find_map(|line| line.strip_prefix("client_agent_port="))
        .expect("client agent port is recorded")
        .parse::<u16>()
        .expect("client agent port is numeric");
    assert_ne!(
        port, 39094,
        "runner must avoid the occupied historical default port\n{automation_driver}"
    );
    assert!(
        automation_driver.contains(&format!(
            "client_agent_bridge_url=http://127.0.0.1:{port}/rpc"
        )),
        "runner must record a bridge URL matching the selected port\n{automation_driver}"
    );
}

#[test]
fn real_client_prepare_auto_selects_secondary_gradle_runclient_adapter() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-real-client-regression.sh");
    let run_root = tempfile::tempdir().expect("create real-client run root");

    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--prepare")
        .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
        .env("SOLARIS_REAL_CLIENT_AGENT_SECRET", "s_primary_prepare")
        .env(
            "SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET",
            "s_secondary_prepare",
        )
        .output()
        .expect("prepare two-client real-client gate");

    assert!(
        output.status.success(),
        "two-client prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_dir_text = String::from_utf8(output.stdout).expect("run dir stdout is utf-8");
    let run_dir = Path::new(run_dir_text.trim());
    let automation_driver = std::fs::read_to_string(run_dir.join("automation-driver.txt"))
        .expect("read automation driver");
    assert!(
        automation_driver.contains("second_client_kind=gradle-runclient")
            && automation_driver.contains("second_client_adapter_source=auto-gradle-runclient")
            && automation_driver
                .contains("second_client_adapter_task=:fabric-agent:runClientAgent")
            && automation_driver.contains("client_username=SolarisPrimary")
            && automation_driver.contains("second_client_username=SolarisSecondary")
            && automation_driver.contains("client_game_dir=")
            && automation_driver.contains("/clients/primary")
            && automation_driver.contains("second_client_game_dir=")
            && automation_driver.contains("/clients/secondary")
            && !automation_driver
                .lines()
                .any(|line| line.contains(&legacy_primary_launcher_metadata_key()))
            && automation_driver.contains("second_client_agent_secret=SET_REDACTED"),
        "prepared two-client gate must auto-select the secondary Gradle runClient adapter and isolate client state\n{automation_driver}"
    );
    let primary_port = automation_driver
        .lines()
        .find_map(|line| line.strip_prefix("client_agent_port="))
        .expect("primary port recorded");
    let secondary_port = automation_driver
        .lines()
        .find_map(|line| line.strip_prefix("second_client_agent_port="))
        .expect("secondary port recorded");
    assert_ne!(
        primary_port, secondary_port,
        "primary and secondary bridge ports must differ\n{automation_driver}"
    );
}

#[test]
fn gradle_runclient_adapter_is_the_default_real_client_launcher() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-playable-client-gate.sh");
    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--check")
        .output()
        .expect("run playable real-client check");

    assert!(
        output.status.success(),
        "playable runner failed to auto-select Gradle runClient\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("gradle-runclient"),
        "playable runner should report the auto-selected runClient adapter\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let fabric_build = std::fs::read_to_string(
        repo_root.join("client-mod/solaris-client-agent/fabric-agent/build.gradle.kts"),
    )
    .expect("read fabric-agent build");
    assert!(
        fabric_build.contains("net.neoforged.moddev")
            && fabric_build.contains("neoForge")
            && fabric_build.contains("create(\"client\")")
            && fabric_build.contains("create(\"clientAgent\")")
            && fabric_build.contains("tasks.named(\"runClientAgent\")")
            && fabric_build.contains("validateClientAgentRunProperties")
            && fabric_build.contains("srcDir(project(\":java-agent\")")
            && !fabric_build.contains("implementation(project(\":java-agent\"))")
            && fabric_build.contains("solaris.clientAgent.secret")
            && fabric_build.contains("solaris.clientAgent.runDir")
            && fabric_build.contains("solaris.clientAgent.gameDir")
            && fabric_build.contains("solaris.clientAgent.username")
            && fabric_build.contains("gameDirectory.set(file(clientAgentGameDir.get()))")
            && fabric_build.contains("programArgument(\"--username\")")
            && fabric_build.contains("programArgument(clientAgentUsername.get())"),
        "Gradle runClient adapter must define a ModDev client launch with client-agent system properties and per-client isolation"
    );
    let adapter_source = std::fs::read_to_string(
        repo_root.join(
            "client-mod/solaris-client-agent/fabric-agent/src/main/java/dev/solaris/agent/neoforge/SolarisClientAgentMod.java",
        ),
    )
    .expect("read runClient adapter source");
    assert!(
        adapter_source.contains("NeoForge.EVENT_BUS.addListener")
            && adapter_source.contains("ClientTickEvent.Post")
            && adapter_source.contains("Minecraft.getInstance()"),
        "runClient adapter must wait for the Minecraft client tick before starting the bridge"
    );
}

#[test]
fn runner_usage_names_gradle_adapter_not_injectable_client_launcher() {
    let repo_root = repo_root();
    let runner_path = repo_root.join("tools/run-real-client-regression.sh");
    let output = Command::new("bash")
        .arg(&runner_path)
        .arg("--help")
        .output()
        .expect("run real-client help");

    assert!(
        output.status.success(),
        "runner help failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("Primary client adapter is auto-selected")
            && help.contains("Gradle runClient adapter"),
        "runner help must document the fixed Gradle runClient adapter\n{help}"
    );
    assert!(
        !help.contains("client command") && !help.contains("configured command"),
        "runner help must not expose an injectable client command path\n{help}"
    );
}

#[test]
fn m94_real_client_manifest_covers_required_regression_rows() {
    let manifest: Value = serde_json::from_str(M94_MANIFEST).expect("M94 manifest is valid JSON");

    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["quality_label"], "stabilization");
    assert!(
        manifest["client_requirement"]
            .as_str()
            .expect("client requirement is present")
            .contains("vanilla 26.1.2 client"),
        "manifest must require a real vanilla client"
    );

    let forbidden = manifest["forbidden_client_evidence"]
        .as_array()
        .expect("forbidden client evidence list is present");
    assert!(
        forbidden.iter().any(|entry| entry == "wire-probe"),
        "wire-probe must be explicitly excluded as real-client evidence"
    );
    assert!(
        forbidden
            .iter()
            .any(|entry| entry == "mc_test_harness::client::Client"),
        "protocol harness client must be explicitly excluded as real-client evidence"
    );

    let scenarios = manifest["scenarios"]
        .as_array()
        .expect("scenarios are present");
    assert!(scenarios.len() > 1, "M94 cannot be a single-scenario pack");

    let required_artifacts = manifest["required_artifacts"]
        .as_array()
        .expect("required artifacts are present");
    let mut artifact_ids = BTreeSet::new();
    for artifact in required_artifacts {
        let artifact = artifact.as_str().expect("required artifact is a string");
        assert!(!artifact.is_empty(), "required artifact cannot be empty");
        assert!(
            artifact_ids.insert(artifact),
            "duplicate required artifact {artifact}"
        );
    }
    for artifact in [
        "manifest.json",
        "client.log",
        "server.log",
        "observations.json",
        "screenshots/",
        "git.txt",
        "toolchain.txt",
        "automation-driver.txt",
    ] {
        assert!(
            artifact_ids.contains(artifact),
            "missing required artifact {artifact}"
        );
    }

    let runner = &manifest["automation_runner"];
    assert_eq!(
        runner["script"].as_str(),
        Some("tools/run-real-client-regression.sh"),
        "M94 pack must name the approved real-client runner"
    );
    assert_eq!(
        runner["client_adapter"].as_str(),
        Some(
            "client-mod/solaris-client-agent/gradlew --no-configuration-cache :fabric-agent:runClientAgent"
        ),
        "runner must document the repo-native Gradle runClient adapter"
    );
    assert_eq!(
        runner["client_adapter_source"].as_str(),
        Some("auto-gradle-runclient"),
        "runner must document the auto-selected primary client source"
    );
    assert_eq!(
        runner["client_isolation"].as_str(),
        Some(
            "Gradle runClient launches use per-run game directories and distinct offline usernames for primary/secondary clients."
        ),
        "runner must document per-client Gradle runClient isolation"
    );
    assert_eq!(
        runner["passing_gate"].as_str(),
        Some("agent-run real-client"),
        "runner must distinguish completed real-client evidence from prepared scaffolding"
    );
    assert_eq!(
        runner["agent_driver"].as_str(),
        Some("tools/real-client-agent-driver.py"),
        "runner must name the approved in-client bridge driver"
    );
    assert_eq!(
        runner["agent_bridge_url_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_AGENT_BRIDGE_URL"),
        "runner must expose the loopback bridge URL env hook"
    );
    assert_eq!(
        runner["agent_secret_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_AGENT_SECRET"),
        "runner must expose the per-run bridge secret env hook"
    );
    assert_eq!(
        runner["agent_jar_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_AGENT_JAR"),
        "runner must expose the Java agent jar env hook"
    );
    assert_eq!(
        runner["agent_port_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_AGENT_PORT"),
        "runner must expose the Java agent port env hook"
    );
    assert_eq!(
        runner["agent_scenario_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_AGENT_SCENARIO"),
        "runner must expose the agent scenario override hook"
    );
    assert_eq!(
        runner["agent_server_addr_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_SERVER_ADDR"),
        "runner must expose the server address passed to the client agent"
    );
    assert_eq!(
        runner["second_client_adapter"].as_str(),
        Some(
            "client-mod/solaris-client-agent/gradlew --no-configuration-cache :fabric-agent:runClientAgent"
        ),
        "runner must document the repo-native secondary Gradle runClient adapter"
    );
    assert_eq!(
        runner["second_client_adapter_source"].as_str(),
        Some("auto-gradle-runclient"),
        "runner must document the auto-selected secondary client source"
    );
    assert!(
        runner.get("second_command_env").is_none(),
        "runner must not require a shell command hook for the second real client"
    );
    assert_eq!(
        runner["second_agent_secret_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_SECOND_AGENT_SECRET"),
        "runner must expose a separate second-client bridge secret"
    );
    assert_eq!(
        runner["second_agent_port_env"].as_str(),
        Some("SOLARIS_REAL_CLIENT_SECOND_AGENT_PORT"),
        "runner must expose a separate second-client bridge port"
    );
    let runner_modes = runner["modes"]
        .as_array()
        .expect("runner modes are present");
    for mode in ["--check", "--prepare", "--run", "--validate-run"] {
        assert!(
            runner_modes.iter().any(|entry| entry == mode),
            "runner must support mode {mode}"
        );
    }

    let mut covered_rows = BTreeSet::new();
    let mut scenario_ids = BTreeSet::new();
    for scenario in scenarios {
        let id = scenario["id"].as_str().expect("scenario id is present");
        assert!(scenario_ids.insert(id), "duplicate scenario id {id}");
        assert_eq!(scenario["status"], "manual-pending");
        assert_eq!(
            scenario["screenshots_required"], true,
            "scenario {} must require screenshots",
            scenario["id"]
        );
        assert!(
            scenario["bounded_time_minutes"]
                .as_u64()
                .unwrap_or_default()
                <= 25,
            "scenario {} must stay bounded",
            scenario["id"]
        );
        assert!(
            scenario["steps"]
                .as_array()
                .is_some_and(|steps| !steps.is_empty()),
            "scenario {} needs runnable steps",
            scenario["id"]
        );
        assert!(
            scenario["expected_observations"]
                .as_array()
                .is_some_and(|observations| !observations.is_empty()),
            "scenario {} needs expected observations",
            scenario["id"]
        );
        for row in scenario["ledger_rows"]
            .as_array()
            .expect("scenario ledger rows are present")
        {
            covered_rows.insert(row.as_str().expect("ledger row is a string"));
        }
    }
    assert!(
        scenario_ids.contains("m94-02c-water-bucket-place-pickup"),
        "M94 manifest must keep the focused accepted water-bucket real-client scenario"
    );
    assert!(
        scenario_ids.contains("m94-04a-regular-sign-place-text"),
        "M94 manifest must keep the focused regular sign real-client scenario"
    );
    assert!(
        scenario_ids.contains("m94-03a-inventory-oak-log-to-planks"),
        "M94 manifest must keep the focused inventory recipe real-client scenario"
    );
    assert!(
        scenario_ids.contains("m94-03b-two-client-shared-chest"),
        "M94 manifest must keep the focused two-client shared chest scenario"
    );
    assert!(
        scenario_ids.contains("m94-03c-two-client-shared-chest-live-update"),
        "M94 manifest must keep the focused two-client shared chest live-update scenario"
    );
    assert!(
        scenario_ids.contains("m94-08-enchanting-efficiency"),
        "M94 manifest must keep the focused enchanting scenario"
    );

    let mut manual_pending_rows = BTreeSet::new();
    for row in manifest["scoped_rows_manual_pending"]
        .as_array()
        .expect("manual-pending scoped rows are present")
    {
        assert!(
            row["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "manual-pending scoped row needs a reason"
        );
        manual_pending_rows.insert(row["row"].as_str().expect("manual-pending row is a string"));
    }

    for row in [
        "P1", "P2", "P3", "P4", "W3", "C1", "C2", "B1", "B2", "B3", "B4", "B5", "B6", "F1", "F2",
        "F3", "F4", "L1", "L2", "I1", "I2", "K1", "K2", "A1", "E1", "E2", "E3", "V1", "N1", "N2",
        "G1", "G2", "G3", "G4", "S1", "S2", "O1", "O2",
    ] {
        assert!(
            covered_rows.contains(row) || manual_pending_rows.contains(row),
            "missing M94 ledger row {row}"
        );
    }
}

#[test]
fn validate_run_rejects_missing_required_scenario_screenshots() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": []
            }]
        }),
    );
    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a passed required-screenshot scenario without screenshots\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("screenshots"),
        "validator error should name screenshots\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_misspelled_client_gate() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-runreal-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a misspelled client_gate\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("client_gate"),
        "validator error should name client_gate\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_legacy_client_command_adapter_metadata() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        [
            "client_gate=agent-run real-client configured: command-launcher",
            "client_kind=prismlauncher",
            "client_adapter_source=legacy-command",
            "client_command=redacted",
        ]
        .join("\n"),
    )
    .expect("write legacy automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted legacy client-command adapter metadata\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("automation-driver"),
        "validator error should name automation-driver.txt\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_nonzero_agent_driver_exit_status() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_bridge_wait_status_primary=ready\n\
client_agent_phase_exit_status_m94-01-join-rejoin-chunks-movement=0\n\
client_agent_driver_exit_status=1\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write nonzero driver automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a nonzero client-agent driver exit status\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exit status"),
        "validator error should name client-agent exit status\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_timed_out_agent_bridge_wait() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_bridge_wait_status_primary=timeout\n\
client_agent_phase_exit_status_m94-01-join-rejoin-chunks-movement=0\n\
client_agent_driver_exit_status=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write timed-out bridge automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a timed-out client-agent bridge wait\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("bridge wait"),
        "validator error should name client-agent bridge wait\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_two_client_enabled_without_secondary_bridge_ready() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}second_client_enabled=1\n\
second_client_kind=gradle-runclient\n\
second_client_adapter_source=auto-gradle-runclient\n\
second_client_adapter_task=:fabric-agent:runClientAgent\n\
second_client_agent_secret=SET_REDACTED\n\
client_agent_playable_two_client_opposite_chunk_phase_exit_status=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write missing secondary bridge automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a two-client artifact without secondary bridge-ready evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("secondary")
            && String::from_utf8_lossy(&output.stderr).contains("bridge"),
        "validator error should name the missing secondary bridge evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_two_client_observations_without_second_client_enabled() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-42-two-client-opposite-chunk-crossing",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-42-two-client-opposite-chunk-crossing",
                "result": "passed",
                "screenshots": []
            }]
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted two-client observations without second-client runner evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("two-client")
            && String::from_utf8_lossy(&output.stderr).contains("second_client_enabled"),
        "validator error should tie two-client observations to second_client_enabled\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_restart_observations_without_runner_restart_evidence() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-03-save-restart-after",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-03-save-restart-after",
                "result": "passed",
                "observations": ["restart marker persistence: passed"]
            }]
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted after-restart observations without runner restart evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server_restart_count")
            || String::from_utf8_lossy(&output.stderr).contains("restart"),
        "validator error should name missing restart runner evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_after_restart_observations_without_before_phase() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-03-save-restart-after",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-03-save-restart-after",
                "result": "passed",
                "observations": ["restart marker persistence: passed"]
            }]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}server_stop_phase=playable-03-before-restart signal=INT\n\
server_restart_count=1\n\
server_start_phase=playable-03-after-restart\n\
client_agent_phase_exit_status_playable-03-save-restart-after=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write restart automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted after-restart observations without a passed before/primary phase\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("before")
            || String::from_utf8_lossy(&output.stderr).contains("restart"),
        "validator error should name the missing paired before-restart observation\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_observed_scenario_without_matching_phase_exit_status() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-01-join-generated-spawn",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-01-join-generated-spawn",
                "result": "passed",
                "screenshots": []
            }]
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted passed observations without a matching client-agent phase status\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("phase_exit_status")
            && String::from_utf8_lossy(&output.stderr).contains("playable-01-join-generated-spawn"),
        "validator error should name the missing scenario phase status\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_wrong_observations_schema() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v0",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a wrong observations schema\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("schema"),
        "validator error should name schema\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_wrong_quality_label() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "release-ready",
            "result": "passed",
            "scenarios": []
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a wrong quality_label\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("quality_label"),
        "validator error should name quality_label\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_failed_observations_result() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "failed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "failed",
                "error": { "message": "Remote end closed connection without response" },
                "screenshots": []
            }]
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted failed observations\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("result"),
        "validator error should name result\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_passed_observations_without_scenarios() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted passed observations with no executed scenarios\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("scenarios"),
        "validator error should name scenarios\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_accepts_the_manifest_quality_label_for_playable_gate() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-01-join-generated-spawn",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-01-join-generated-spawn",
                "result": "passed",
                "screenshots": []
            }]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_playable-01-join-generated-spawn=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write playable phase automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        output.status.success(),
        "validator rejected a run whose observations match the manifest quality_label\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_accepts_playable_save_restart_phase_observations() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let manifest_path = repo_root.join("docs/playable/real-client-playable-loop.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &manifest,
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [
                {
                    "id": "playable-03-save-restart-before",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-03-save-restart-before.png"]
                },
                {
                    "id": "playable-03-save-restart-after",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-03-save-restart-after.png"]
                }
            ]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_playable-03-save-restart-before=0\n\
server_stop_phase=playable-03-before-restart signal=INT\n\
server_exit_phase=playable-03-before-restart status=0\n\
server_restart_count=1\n\
server_start_phase=playable-03-after-restart\n\
client_agent_phase_exit_status_playable-03-save-restart-after=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write restart automation driver");
    for screenshot in [
        "playable-03-save-restart-before.png",
        "playable-03-save-restart-after.png",
    ] {
        std::fs::write(
            run_dir.path().join("screenshots").join(screenshot),
            valid_png_320x180(),
        )
        .expect("write valid screenshot");
    }

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        output.status.success(),
        "validator rejected playable save/restart phase observations\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_playable_restart_when_server_stop_is_nonzero() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let manifest_path = repo_root.join("docs/playable/real-client-playable-loop.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &manifest,
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [
                {
                    "id": "playable-04-twenty-minute-survival-loop",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-04-twenty-minute-survival-loop.png"]
                },
                {
                    "id": "playable-03-save-restart-after",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-03-save-restart-after.png"]
                }
            ]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_playable-04-twenty-minute-survival-loop=0\n\
server_stop_phase=playable-04-before-restart signal=INT\n\
server_exit_phase=playable-04-before-restart status=5\n\
server_restart_count=1\n\
server_start_phase=playable-04-after-restart\n\
client_agent_phase_exit_status_playable-03-save-restart-after=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write non-clean restart automation driver");
    for screenshot in [
        "playable-04-twenty-minute-survival-loop.png",
        "playable-03-save-restart-after.png",
    ] {
        std::fs::write(
            run_dir.path().join("screenshots").join(screenshot),
            valid_png_320x180(),
        )
        .expect("write valid screenshot");
    }

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted restart evidence after a nonzero server stop"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server_exit_phase")
            && String::from_utf8_lossy(&output.stderr).contains("status=0"),
        "validator must require a clean stop before all restart evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_p46_after_observation_when_stop_exit_is_nonzero() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [
                {
                    "id": "playable-46-generated-ruin-cache-before",
                    "status": "manual-pending",
                    "screenshots_required": false
                },
                {
                    "id": "playable-46-generated-ruin-cache-after",
                    "status": "manual-pending",
                    "screenshots_required": false
                }
            ]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [
                {
                    "id": "playable-46-generated-ruin-cache-before",
                    "result": "passed",
                    "screenshots": []
                },
                {
                    "id": "playable-46-generated-ruin-cache-after",
                    "result": "passed",
                    "screenshots": []
                }
            ]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_runtime_kind_primary=compiled-classes\n\
client_agent_runtime_path_primary=/tmp/fabric-agent/build/classes/java/main\n\
client_agent_runtime_sha256_primary=0000000000000000000000000000000000000000000000000000000000000000\n\
client_agent_runtime_file_count_primary=1\n\
client_agent_runtime_validation_primary=verified\n\
client_agent_phase_exit_status_playable-46-generated-ruin-cache-before=0\n\
server_stop_phase=playable-46-before-restart signal=INT\n\
server_exit_phase=playable-46-before-restart status=5\n\
server_restart_count=1\n\
server_start_phase=playable-46-after-restart\n\
client_agent_phase_exit_status_playable-46-generated-ruin-cache-after=0\n\
server_world_dir={}/world\n\
server_op_users=NONE\n",
            gradle_automation_driver_fixture(),
            run_dir.path().display()
        ),
    )
    .expect("write P46 restart automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted P46 after evidence after a nonzero server stop\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server_exit_phase")
            && String::from_utf8_lossy(&output.stderr).contains("status=0"),
        "validator must require a recorded clean P46 stop status\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn p46_prepare_never_reuses_a_colliding_run_directory() {
    let repo_root = repo_root();
    let run_root = tempfile::tempdir().expect("create real-client run root");
    let command_dir = tempfile::tempdir().expect("create fake command directory");
    let date_path = command_dir.path().join("date");
    std::fs::write(
        &date_path,
        "#!/usr/bin/env bash\nprintf '20260718T000000Z\\n'\n",
    )
    .expect("write fake date command");
    let mut permissions = std::fs::metadata(&date_path)
        .expect("read fake date command metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    std::fs::set_permissions(&date_path, permissions).expect("make fake date command executable");
    let path = format!(
        "{}:{}",
        command_dir.path().display(),
        std::env::var("PATH").expect("PATH is set")
    );

    let prepare = || {
        Command::new("bash")
            .arg(repo_root.join("tools/run-real-client-regression.sh"))
            .arg("--prepare")
            .env("PATH", &path)
            .env("SOLARIS_REAL_CLIENT_RUN_ROOT", run_root.path())
            .env(
                "SOLARIS_REAL_CLIENT_AGENT_SCENARIO",
                "playable-46-generated-ruin-cache",
            )
            .output()
            .expect("prepare P46 real-client run")
    };

    let first = prepare();
    assert!(
        first.status.success(),
        "first P46 prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_dir = PathBuf::from(
        String::from_utf8(first.stdout)
            .expect("first run dir is utf-8")
            .trim(),
    );
    std::fs::create_dir(first_dir.join("world")).expect("create sentinel P46 world");
    std::fs::write(first_dir.join("world/sentinel"), "must not be reused")
        .expect("write sentinel P46 world state");

    let second = prepare();
    assert!(
        second.status.success(),
        "second P46 prepare failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let second_dir = PathBuf::from(
        String::from_utf8(second.stdout)
            .expect("second run dir is utf-8")
            .trim(),
    );
    assert_ne!(
        first_dir, second_dir,
        "P46 must create a new run directory rather than reusing a timestamp collision"
    );
    assert!(
        !second_dir.join("world/sentinel").exists(),
        "P46 must never reuse a preexisting world after a run-directory collision"
    );
}

#[test]
fn validate_run_accepts_two_client_shared_chest_restart_observations() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let manifest_path = repo_root.join("docs/playable/real-client-playable-loop.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &manifest,
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [
                {
                    "id": "playable-45-two-client-shared-chest-save-restart-before",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-45-before.png"]
                },
                {
                    "id": "playable-45-two-client-shared-chest-save-restart-after",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-45-after.png"]
                }
            ]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_playable-45-two-client-shared-chest-save-restart-before=0\n\
server_stop_phase=playable-45-before-restart signal=INT\n\
server_exit_phase=playable-45-before-restart status=0\n\
server_restart_count=1\n\
server_start_phase=playable-45-after-restart\n\
client_agent_phase_exit_status_playable-45-two-client-shared-chest-save-restart-after=0\n",
            two_client_gradle_automation_driver_fixture()
        ),
    )
    .expect("write two-client restart automation driver");
    for screenshot in ["playable-45-before.png", "playable-45-after.png"] {
        std::fs::write(
            run_dir.path().join("screenshots").join(screenshot),
            valid_png_320x180(),
        )
        .expect("write valid screenshot");
    }

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        output.status.success(),
        "validator rejected two-client shared-chest restart observations\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_two_client_shared_chest_after_without_restart_evidence() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    let manifest_path = repo_root.join("docs/playable/real-client-playable-loop.json");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", manifest_path.display()));
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &manifest,
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [
                {
                    "id": "playable-45-two-client-shared-chest-save-restart-before",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-45-before.png"]
                },
                {
                    "id": "playable-45-two-client-shared-chest-save-restart-after",
                    "result": "passed",
                    "screenshots": ["screenshots/playable-45-after.png"]
                }
            ]
        }),
    );
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_playable-45-two-client-shared-chest-save-restart-before=0\n\
client_agent_phase_exit_status_playable-45-two-client-shared-chest-save-restart-after=0\n",
            two_client_gradle_automation_driver_fixture()
        ),
    )
    .expect("write restart-free two-client automation driver");
    for screenshot in ["playable-45-before.png", "playable-45-after.png"] {
        std::fs::write(
            run_dir.path().join("screenshots").join(screenshot),
            valid_png_320x180(),
        )
        .expect("write valid screenshot");
    }

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a P45 after phase without real restart evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server_restart_count")
            || String::from_utf8_lossy(&output.stderr).contains("restart"),
        "validator error should name missing restart evidence\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_unknown_observed_scenario_id() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-unknown-scenario",
                "result": "passed",
                "screenshots": ["screenshots/m94-unknown-scenario.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("m94-unknown-scenario.png"),
        b"fake png bytes",
    )
    .expect("write screenshot");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a scenario id missing from the manifest\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unknown scenario"),
        "validator error should name the unknown scenario\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_invalid_screenshot_png() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": ["screenshots/m94-01-join-rejoin-chunks-movement.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("m94-01-join-rejoin-chunks-movement.png"),
        b"fake png bytes",
    )
    .expect("write invalid screenshot");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted an invalid screenshot PNG\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid PNG"),
        "validator error should name invalid PNG bytes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_invalid_optional_screenshot_png() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "optional-screenshot-pack",
            "quality_label": "stabilization",
            "scenarios": [{
                "id": "optional-screenshot-scenario",
                "status": "manual-pending",
                "screenshots_required": false
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "optional-screenshot-scenario",
                "result": "passed",
                "screenshots": ["screenshots/optional-screenshot.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("optional-screenshot.png"),
        b"fake png bytes",
    )
    .expect("write invalid optional screenshot");
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        format!(
            "{}client_agent_phase_exit_status_optional-screenshot-scenario=0\n",
            gradle_automation_driver_fixture()
        ),
    )
    .expect("write optional screenshot automation driver");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted an invalid optional screenshot PNG\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid PNG"),
        "validator error should validate optional screenshot bytes\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_tiny_required_screenshot_png() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": ["screenshots/m94-01-join-rejoin-chunks-movement.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("m94-01-join-rejoin-chunks-movement.png"),
        valid_png_1x1(),
    )
    .expect("write tiny screenshot");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a tiny required screenshot PNG\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("screenshot")
            && String::from_utf8_lossy(&output.stderr).contains("size"),
        "validator error should name screenshot size\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_accepts_valid_required_screenshot_png() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": ["screenshots/m94-01-join-rejoin-chunks-movement.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("m94-01-join-rejoin-chunks-movement.png"),
        valid_png_320x180(),
    )
    .expect("write valid screenshot");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        output.status.success(),
        "validator rejected a valid screenshot PNG\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("validated"),
        "validator success should report validated run\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_playable_server_log_degradation_warnings() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": []
        }),
    );
    std::fs::write(
        run_dir.path().join("server.log"),
        [
            r#"WARN mc_net::server: runtime tick exceeded performance budget tick_us=51000"#,
            r#"WARN mc_net::lock_metrics: lock wait exceeded M39 budget lock="chunk_prepare" operation="chunk prepare neighbour commit" wait_us=20000"#,
            r#"WARN mc_net::play::chunk_stream: chunk preparation deferred by dirty chunk cache pressure"#,
            r#"INFO mc_net::play::chunk_stream: view-distance window flushed degraded_delivery=true"#,
            r#"WARN mc_net::play: teleport confirmation id mismatch expected=10 received=9"#,
        ]
        .join("\n"),
    )
    .expect("write degraded server log");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a run with server-side playable degradation warnings\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("server.log"),
        "validator error should name server.log\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_accepts_subsecond_tick_budget_warnings() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": ["screenshots/m94-01-join-rejoin-chunks-movement.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots/m94-01-join-rejoin-chunks-movement.png"),
        valid_png_320x180(),
    )
    .expect("write valid screenshot");
    std::fs::write(
        run_dir.path().join("server.log"),
        "WARN mc_net::server: runtime tick exceeded performance budget tick_us=412302\n",
    )
    .expect("write subsecond tick warning");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        output.status.success(),
        "functional real-client evidence must not fail on a subsecond budget warning\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_half_second_tick_stall() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts(
        run_dir.path(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "stabilization",
            "result": "passed",
            "scenarios": [{
                "id": "m94-01-join-rejoin-chunks-movement",
                "result": "passed",
                "screenshots": ["screenshots/m94-01-join-rejoin-chunks-movement.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots/m94-01-join-rejoin-chunks-movement.png"),
        valid_png_320x180(),
    )
    .expect("write valid screenshot");
    std::fs::write(
        run_dir.path().join("server.log"),
        "WARN mc_net::server: runtime tick exceeded performance budget tick_us=500000\n",
    )
    .expect("write catastrophic tick warning");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "real-client validator accepted a half-second server stall\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("catastrophic runtime tick"),
        "validator must name the catastrophic tick threshold\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn validate_run_rejects_p42_slow_chunk_crossing_windows() {
    let repo_root = repo_root();
    let run_dir = tempfile::tempdir().expect("create run dir");
    write_validate_run_artifacts_with_manifest(
        run_dir.path(),
        &json!({
            "schema_version": 1,
            "pack_id": "playable-real-client-loop",
            "quality_label": "playable-spike",
            "scenarios": [{
                "id": "playable-42-two-client-opposite-chunk-crossing",
                "status": "manual-pending",
                "screenshots_required": true
            }]
        })
        .to_string(),
        json!({
            "schema": "solaris.real_client_observations.v1",
            "client_gate": "agent-run-real-client",
            "quality_label": "playable-spike",
            "result": "passed",
            "scenarios": [{
                "id": "playable-42-two-client-opposite-chunk-crossing",
                "result": "passed",
                "screenshots": ["screenshots/playable-42-primary.png"]
            }]
        }),
    );
    std::fs::write(
        run_dir
            .path()
            .join("screenshots")
            .join("playable-42-primary.png"),
        valid_png_320x180(),
    )
    .expect("write valid screenshot");
    std::fs::write(
        run_dir.path().join("automation-driver.txt"),
        two_client_gradle_automation_driver_fixture(),
    )
    .expect("write two-client automation driver");
    std::fs::write(
        run_dir.path().join("server.log"),
        [
            r#"INFO mc_net::play::chunk_stream: view-distance window flushed center_cx=0 center_cz=-2 degraded_delivery=false fetch_ms=1758 light_compute_ms=1124 slow_fetch_chunks=9 slow_light_compute_chunks=9"#,
            r#"INFO mc_net::play::chunk_stream: view-distance window flushed center_cx=0 center_cz=1 degraded_delivery=false fetch_ms=0 light_compute_ms=0 slow_fetch_chunks=0 slow_light_compute_chunks=0"#,
        ]
        .join("\n"),
    )
    .expect("write slow P42 server log");

    let output = validate_run(&repo_root, run_dir.path());

    assert!(
        !output.status.success(),
        "validator accepted a P42 run with slow chunk crossing windows\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("slow chunk"),
        "validator error should name slow chunk windows\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn legacy_primary_client_env_names() -> Vec<String> {
    vec![
        ["SOLARIS", "REAL", "CLIENT", "COMMAND"].join("_"),
        ["M94", "CLIENT", "COMMAND"].join("_"),
        ["SOLARIS", "REAL", "CLIENT", "KIND"].join("_"),
    ]
}

fn legacy_primary_launcher_metadata_key() -> String {
    ["client", "command"].join("_")
}

fn write_validate_run_artifacts(run_dir: &Path, observations: Value) {
    write_validate_run_artifacts_with_manifest(run_dir, M94_MANIFEST, observations);
}

fn write_validate_run_artifacts_with_manifest(run_dir: &Path, manifest: &str, observations: Value) {
    std::fs::write(run_dir.join("manifest.json"), manifest).expect("write manifest");
    for artifact in ["client.log", "server.log", "git.txt", "toolchain.txt"] {
        std::fs::write(run_dir.join(artifact), "").expect("write artifact");
    }
    std::fs::write(
        run_dir.join("automation-driver.txt"),
        gradle_automation_driver_fixture(),
    )
    .expect("write automation driver");
    std::fs::create_dir(run_dir.join("screenshots")).expect("create screenshots dir");
    std::fs::write(
        run_dir.join("observations.json"),
        serde_json::to_vec_pretty(&observations).expect("serialize observations"),
    )
    .expect("write observations");
}

fn gradle_automation_driver_fixture() -> &'static str {
    "client_gate=agent-run real-client configured: gradle-runclient\n\
client_kind=gradle-runclient\n\
client_adapter_source=auto-gradle-runclient\n\
client_adapter_task=:fabric-agent:runClientAgent\n\
client_agent_runtime_kind_primary=compiled-classes\n\
client_agent_runtime_path_primary=/tmp/solaris-client-agent/fabric-agent/build/classes/java/main\n\
client_agent_runtime_sha256_primary=0000000000000000000000000000000000000000000000000000000000000000\n\
client_agent_runtime_file_count_primary=1\n\
client_agent_runtime_validation_primary=verified\n\
client_agent_bridge_wait_status_primary=ready\n\
client_agent_phase_exit_status_m94-01-join-rejoin-chunks-movement=0\n\
client_agent_driver_exit_status=0\n"
}

fn two_client_gradle_automation_driver_fixture() -> &'static str {
    "client_gate=agent-run real-client configured: gradle-runclient\n\
client_kind=gradle-runclient\n\
client_adapter_source=auto-gradle-runclient\n\
client_adapter_task=:fabric-agent:runClientAgent\n\
client_agent_runtime_kind_primary=compiled-classes\n\
client_agent_runtime_path_primary=/tmp/solaris-client-agent/fabric-agent/build/classes/java/main\n\
client_agent_runtime_sha256_primary=0000000000000000000000000000000000000000000000000000000000000000\n\
client_agent_runtime_file_count_primary=1\n\
client_agent_runtime_validation_primary=verified\n\
client_agent_bridge_wait_status_primary=ready\n\
client_agent_bridge_wait_status_secondary=ready\n\
client_agent_runtime_kind_secondary=compiled-classes\n\
client_agent_runtime_path_secondary=/tmp/solaris-client-agent/fabric-agent/build/classes/java/main\n\
client_agent_runtime_sha256_secondary=1111111111111111111111111111111111111111111111111111111111111111\n\
client_agent_runtime_file_count_secondary=1\n\
client_agent_runtime_validation_secondary=verified\n\
client_agent_phase_exit_status_playable-42-two-client-opposite-chunk-crossing=0\n\
client_agent_driver_exit_status=0\n\
second_client_enabled=1\n\
second_client_kind=gradle-runclient\n\
second_client_adapter_source=auto-gradle-runclient\n\
second_client_adapter_task=:fabric-agent:runClientAgent\n\
second_client_agent_secret=SET_REDACTED\n"
}

fn validate_run(repo_root: &Path, run_dir: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(repo_root.join("tools/run-real-client-regression.sh"))
        .arg("--validate-run")
        .arg(run_dir)
        .output()
        .expect("run real-client validator")
}

fn valid_png_1x1() -> Vec<u8> {
    valid_png_rgba(1, 1)
}

fn valid_png_320x180() -> Vec<u8> {
    valid_png_rgba(320, 180)
}

fn valid_png_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    append_png_chunk(&mut png, b"IHDR", &ihdr);

    let row_len = 1 + (width as usize * 4);
    let mut raw = Vec::with_capacity(row_len * height as usize);
    for _ in 0..height {
        raw.push(0);
        raw.resize(raw.len() + width as usize * 4, 0);
    }

    let mut zlib = Vec::new();
    zlib.extend_from_slice(&[0x78, 0x01]);
    for (index, chunk) in raw.chunks(65_535).enumerate() {
        zlib.push(if index == (raw.len() - 1) / 65_535 {
            0x01
        } else {
            0x00
        });
        let len = chunk.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    append_png_chunk(&mut png, b"IDAT", &zlib);
    append_png_chunk(&mut png, b"IEND", &[]);
    png
}

fn append_png_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(chunk_type);
    png.extend_from_slice(data);
    let mut crc_data = Vec::with_capacity(chunk_type.len() + data.len());
    crc_data.extend_from_slice(chunk_type);
    crc_data.extend_from_slice(data);
    png.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD_ADLER: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + u32::from(*byte)) % MOD_ADLER;
        b = (b + a) % MOD_ADLER;
    }
    (b << 16) | a
}

#[allow(dead_code)]
const VALID_PNG_320X180: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x01, 0x40, 0x00, 0x00, 0x00, 0xb4, 0x08, 0x06, 0x00, 0x00, 0x00, 0xe5, 0xe4, 0xf5,
    0x11, 0x00, 0x00, 0x00, 0xf6, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0xed, 0xc1, 0x01, 0x0d, 0x00,
    0x00, 0x00, 0xc2, 0xa0, 0xf7, 0x4f, 0x6d, 0x0e, 0x37, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x78, 0x35, 0x84, 0xe1, 0x00, 0x01, 0xed, 0xc7, 0x86, 0x25, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];
