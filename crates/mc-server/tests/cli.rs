//! Integration tests for the `mc-server` binary's CLI surface.
//!
//! These cover only the synchronous `--check` path (parse a config, print
//! it, exit). The end-to-end "actually serve a connection" test lives in
//! `tests/status.rs` because it needs tokio and a real socket.

use std::io::Write;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use tempfile::NamedTempFile;

const SAMPLE_TOML: &str = r#"
[server]
name = "TestServer"
motd = "Hello"

[network]
bind_address = "127.0.0.1"
port = 30000

[data]
world_dir = ".analysis/cli-check-world"
"#;

fn write_current_vanilla_version(vanilla_dir: &Path) {
    std::fs::write(
        vanilla_dir.join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
}

fn write_minimal_registry_tree(vanilla_dir: &Path) {
    let minecraft_root = vanilla_dir.join("data").join("minecraft");
    for (_, fs_subpath) in mc_data::KNOWN_REGISTRIES {
        let registry_dir = minecraft_root.join(fs_subpath);
        std::fs::create_dir_all(&registry_dir).expect("create registry dir");
        std::fs::write(registry_dir.join("solaris_placeholder.json"), "{}")
            .expect("write registry placeholder");
    }
}

fn write_valid_block_light_report(vanilla_dir: &Path) {
    let reports_dir = vanilla_dir.join("reports");
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    std::fs::write(
        reports_dir.join("block_light.json"),
        format!(
            r#"{{"version":"{}","max_state_id":0,"entries":[[0,0,1,0]]}}"#,
            mc_protocol::TARGET_RELEASE
        ),
    )
    .expect("write block_light.json");
}

fn write_minimal_registries_report(vanilla_dir: &Path) {
    let reports_dir = vanilla_dir.join("reports");
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    std::fs::write(
        reports_dir.join("registries.json"),
        r#"{
            "minecraft:block": {
                "entries": {
                    "minecraft:stone": { "protocol_id": 1 }
                }
            },
            "minecraft:item": {
                "entries": {
                    "minecraft:apple": { "protocol_id": 1 }
                }
            },
            "minecraft:entity_type": {
                "entries": {
                    "minecraft:pig": { "protocol_id": 1 }
                }
            }
        }"#,
    )
    .expect("write registries.json");
}

fn write_minimal_resolved_tags(vanilla_dir: &Path) {
    for (root, tag, entry) in [
        ("block", "natural", "minecraft:stone"),
        ("item", "food", "minecraft:apple"),
        ("entity_type", "animals", "minecraft:pig"),
    ] {
        let tags_dir = vanilla_dir
            .join("data")
            .join("minecraft")
            .join("tags")
            .join(root);
        std::fs::create_dir_all(&tags_dir).expect("create tags dir");
        std::fs::write(
            tags_dir.join(format!("{tag}.json")),
            format!(r#"{{ "values": [ "{entry}" ] }}"#),
        )
        .expect("write tag file");
    }
}

fn write_minimal_supported_recipe(vanilla_dir: &Path) {
    let recipes_dir = vanilla_dir.join("data").join("minecraft").join("recipe");
    std::fs::create_dir_all(&recipes_dir).expect("create recipe dir");
    std::fs::write(
        recipes_dir.join("apple_slice.json"),
        r##"{
            "type": "minecraft:crafting_shapeless",
            "category": "misc",
            "ingredients": [{ "item": "minecraft:apple" }],
            "result": {
                "id": "minecraft:apple",
                "count": 1
            }
        }"##,
    )
    .expect("write recipe file");
}

#[test]
fn check_prints_parsed_config_and_exits_zero() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(SAMPLE_TOML.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"TestServer\""))
        .stdout(contains("\"Hello\""))
        .stdout(contains("\"127.0.0.1\""))
        .stdout(contains("30000"));
}

#[test]
fn check_reports_derived_deployment_for_every_plugin() {
    let root = tempfile::tempdir().expect("plugin root");
    let plugins = root.path().join("plugins");
    let server_only = plugins.join("server-only");
    let server_and_client = plugins.join("server-and-client");
    std::fs::create_dir_all(&server_only).expect("create server-only plugin");
    std::fs::create_dir_all(server_and_client.join("client"))
        .expect("create client-required plugin");
    std::fs::write(
        server_only.join("plugin.toml"),
        r#"
            id = "server-only"
            name = "Server Only"
            version = "0.1.0"
            api = "0.6.0"
        "#,
    )
    .expect("write server-only manifest");
    std::fs::write(server_only.join("main.lua"), "").expect("write server-only source");
    std::fs::write(
        server_and_client.join("plugin.toml"),
        r#"
            id = "server-and-client"
            name = "Server And Client"
            version = "0.1.0"
            api = "0.6.0"

            [client]
            schema = 1

            [[client.bundles]]
            id = "assets"
            version = "1"
            artifact = "client/assets.zip"
            sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
            size_bytes = 1
            loaders = ["fabric"]
            content = ["assets"]
            permissions = ["load_assets"]
        "#,
    )
    .expect("write client-required manifest");
    std::fs::write(server_and_client.join("main.lua"), "").expect("write client-required source");
    std::fs::write(server_and_client.join("client/assets.zip"), b"x")
        .expect("write client artifact");
    let world = root.path().join("world");
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            r#"
                [server]
                name = "Plugin Deployment Check"
                motd = "Hello"

                [network]
                bind_address = "127.0.0.1"
                port = 30000

                [data]
                world_dir = "{}"

                [plugins]
                directory = "{}"
            "#,
            world.display(),
            plugins.display()
        ),
    )
    .expect("write config");

    let output = Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let check: serde_json::Value = serde_json::from_slice(&output).expect("parse check JSON");

    assert_eq!(
        check["discovered_plugins"],
        serde_json::json!([
            {
                "id": "server-and-client",
                "deployment": "server_and_client",
                "supported_loaders": ["fabric"],
                "permissions": ["load_assets"],
                "total_artifact_bytes": 1,
                "client_bundles": [{
                    "id": "assets",
                    "version": "1",
                    "artifact": "client/assets.zip",
                    "sha256": "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
                    "size_bytes": 1,
                    "loaders": ["fabric"],
                    "content": ["assets"],
                    "permissions": ["load_assets"]
                }]
            },
            {
                "id": "server-only",
                "deployment": "server_only",
                "supported_loaders": [],
                "permissions": [],
                "total_artifact_bytes": 0,
                "client_bundles": []
            }
        ])
    );
}

#[test]
fn check_prints_automatic_worker_capacity() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "TestServer"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = ".analysis/cli-worker-world"
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"effective_chunk_pipeline\""))
        .stdout(contains("\"chunk_io_threads\""))
        .stdout(contains("\"chunk_worker_threads\""));
}

#[test]
fn check_rejects_zero_save_interval() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let mut file = NamedTempFile::new().expect("tempfile");
    let toml = format!(
        r#"
            [server]
            name = "ZeroSaveInterval"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"

            [simulation]
            save_interval_ticks = 0
        "#,
        world_dir.path().display()
    );
    file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains(
            "simulation.save_interval_ticks=0 must be between 1 and 1728000",
        ));
}

#[test]
fn check_rejects_view_distance_above_vanilla_limit() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let mut file = NamedTempFile::new().expect("tempfile");
    let toml = format!(
        r#"
            [server]
            name = "OversizedViewDistance"
            motd = "Hello"
            view_distance = 33

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.path().display()
    );
    file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("server.view_distance must be between 2 and 32"));
}

#[test]
fn check_rejects_simulation_distance_above_vanilla_limit() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let mut file = NamedTempFile::new().expect("tempfile");
    let toml = format!(
        r#"
            [server]
            name = "OversizedSimulationDistance"
            motd = "Hello"
            simulation_distance = 33

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.path().display()
    );
    file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains(
            "server.simulation_distance must be between 2 and 32",
        ));
}

#[test]
fn check_reports_blank_admin_operator_warning() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let mut file = NamedTempFile::new().expect("tempfile");
    let toml = format!(
        r#"
            [server]
            name = "BlankOperators"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"

            [admin]
            operators = ["", "  "]
        "#,
        world_dir.path().display()
    );
    file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("admin_operator_entry_blank"))
        .stdout(contains("missing_world_dir").not());
}

#[test]
fn check_reports_blank_auth_access_entries_warning() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let mut file = NamedTempFile::new().expect("tempfile");
    let toml = format!(
        r#"
            [server]
            name = "BlankAuthAccess"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"

            [auth]
            whitelist_enabled = true
            whitelist = ["", "  "]
            banned_players = ["", "  "]
        "#,
        world_dir.path().display()
    );
    file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("auth_whitelist_entry_blank"))
        .stdout(contains("auth_banned_player_entry_blank"))
        .stdout(contains("missing_world_dir").not());
}

#[test]
fn check_reports_public_bind_security_warnings() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "PublicTest"
            motd = "Hello"

            [network]
            bind_address = "8.8.8.8"
            port = 30000

            [data]
            world_dir = ".analysis/cli-public-world"

            [admin]
            allow_local_dev_operators = true
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("public_bind_offline_mode"))
        .stdout(contains("public_bind_local_dev_operators"));
}

#[test]
fn check_accepts_online_mode_on_loopback() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "OnlineModeTest"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = ".analysis/cli-online-world"

            [auth]
            online_mode = true
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("online_mode_unsupported").not())
        .stdout(contains("public_bind_online_mode").not());
}

#[test]
fn check_rejects_missing_world_dir() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "MissingWorldConfig"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("data.world_dir is required"));
}

#[test]
fn check_rejects_world_dir_file() {
    let world_file = NamedTempFile::new().expect("world tempfile");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "FileWorld"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_file.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("data.world_dir is not a directory"));
}

#[test]
fn check_reports_missing_world_dir_path_warning() {
    let missing_parent = tempfile::tempdir().expect("world parent tempdir");
    let world_dir = missing_parent.path().join("missing-world");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "MissingWorld"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("world_dir_missing_on_disk"));
}

#[test]
fn check_rejects_world_dir_parent_file() {
    let world_parent_file = NamedTempFile::new().expect("world parent tempfile");
    let world_dir = world_parent_file.path().join("child-world");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "BlockedWorldParent"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("data.world_dir has a non-directory parent"));
}

#[test]
fn check_rejects_world_region_file() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    std::fs::write(world_dir.path().join("region"), b"not a directory")
        .expect("write blocking region file");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "BlockedWorldRegion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("data.world_dir region path is not a directory"));
}

#[test]
fn check_accepts_legacy_region_file_when_modern_region_exists() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let modern_region = world_dir
        .path()
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    std::fs::create_dir_all(modern_region).expect("create modern region dir");
    std::fs::write(world_dir.path().join("region"), b"not a directory")
        .expect("write legacy region file");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "ModernWorldRegion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
        "#,
        world_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("world_region_not_directory").not())
        .stdout(contains("world_dir_not_directory").not())
        .stdout(contains("world_dir_missing_on_disk").not());
}

#[test]
fn check_rejects_vanilla_data_protocol_mismatch() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":999999}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarDrift"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("protocol_version 999999 does not match"));
}

#[test]
fn check_rejects_vanilla_data_release_mismatch() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"26.0-test","world_version":{},"protocol_version":{}}}"#,
            mc_protocol::WORLD_VERSION,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarReleaseDrift"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("release id \"26.0-test\" does not match"));
}

#[test]
fn check_rejects_vanilla_data_world_version_mismatch() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":999999,"protocol_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarWorldDrift"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("world_version 999999 does not match"));
}

#[test]
fn check_rejects_version_drift_before_loading_sidecar_data() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":999999}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION
        ),
    )
    .expect("write version.json");
    write_minimal_registry_tree(vanilla_dir.path());
    let reports_dir = vanilla_dir.path().join("reports");
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    std::fs::write(
        reports_dir.join("block_light.json"),
        format!(
            r#"{{"version":"{}","max_state_id":0,"entries":[[0,0]]}}"#,
            mc_protocol::TARGET_RELEASE
        ),
    )
    .expect("write malformed block_light.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarVersionBeforeBlockLight"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("protocol_version 999999 does not match"));
}

#[test]
fn check_reports_vanilla_data_registry_tree_warning() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{},"protocol_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoRegistries"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_registry_tree_incomplete"))
        .stdout(contains("vanilla_data_version_missing").not())
        .stdout(contains("vanilla_data_version_invalid").not())
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not())
        .stdout(contains("vanilla_data_block_light_report_invalid").not())
        .stdout(contains("vanilla_data_tags_unavailable").not())
        .stdout(contains("vanilla_data_recipes_unavailable").not())
        .stdout(contains("vanilla_data_loot_unavailable").not());
}

#[test]
fn check_reports_malformed_vanilla_block_light_warning() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    write_current_vanilla_version(vanilla_dir.path());
    write_minimal_registry_tree(vanilla_dir.path());
    let reports_dir = vanilla_dir.path().join("reports");
    std::fs::create_dir_all(&reports_dir).expect("create reports dir");
    std::fs::write(
        reports_dir.join("block_light.json"),
        format!(
            r#"{{"version":"{}","max_state_id":0,"entries":[[0,0]]}}"#,
            mc_protocol::TARGET_RELEASE
        ),
    )
    .expect("write malformed block_light.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarBadBlockLight"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_block_light_report_invalid"))
        .stdout(contains("vanilla_data_registry_tree_incomplete").not())
        .stdout(contains("vanilla_data_version_missing").not())
        .stdout(contains("vanilla_data_version_invalid").not())
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not())
        .stdout(contains("vanilla_data_tags_unavailable").not())
        .stdout(contains("vanilla_data_recipes_unavailable").not())
        .stdout(contains("vanilla_data_loot_unavailable").not());
}

#[test]
fn check_reports_missing_vanilla_tags_warning_after_block_light() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    write_current_vanilla_version(vanilla_dir.path());
    write_minimal_registry_tree(vanilla_dir.path());
    write_valid_block_light_report(vanilla_dir.path());
    write_minimal_registries_report(vanilla_dir.path());
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoTags"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_tags_unavailable"))
        .stdout(contains("vanilla_data_registry_tree_incomplete").not())
        .stdout(contains("vanilla_data_block_light_report_invalid").not())
        .stdout(contains("vanilla_data_version_missing").not())
        .stdout(contains("vanilla_data_version_invalid").not())
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not())
        .stdout(contains("vanilla_data_recipes_unavailable").not())
        .stdout(contains("vanilla_data_loot_unavailable").not());
}

#[test]
fn check_reports_missing_vanilla_recipes_warning_after_tags() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    write_current_vanilla_version(vanilla_dir.path());
    write_minimal_registry_tree(vanilla_dir.path());
    write_valid_block_light_report(vanilla_dir.path());
    write_minimal_registries_report(vanilla_dir.path());
    write_minimal_resolved_tags(vanilla_dir.path());
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoRecipes"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_recipes_unavailable"))
        .stdout(contains("vanilla_data_registry_tree_incomplete").not())
        .stdout(contains("vanilla_data_block_light_report_invalid").not())
        .stdout(contains("vanilla_data_tags_unavailable").not())
        .stdout(contains("vanilla_data_loot_unavailable").not())
        .stdout(contains("vanilla_data_version_missing").not())
        .stdout(contains("vanilla_data_version_invalid").not())
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not());
}

#[test]
fn check_reports_missing_vanilla_loot_warning_after_recipes() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    write_current_vanilla_version(vanilla_dir.path());
    write_minimal_registry_tree(vanilla_dir.path());
    write_valid_block_light_report(vanilla_dir.path());
    write_minimal_registries_report(vanilla_dir.path());
    write_minimal_resolved_tags(vanilla_dir.path());
    write_minimal_supported_recipe(vanilla_dir.path());
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoLoot"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_loot_unavailable"))
        .stdout(contains("vanilla_data_registry_tree_incomplete").not())
        .stdout(contains("vanilla_data_block_light_report_invalid").not())
        .stdout(contains("vanilla_data_tags_unavailable").not())
        .stdout(contains("vanilla_data_recipes_unavailable").not())
        .stdout(contains("vanilla_data_version_missing").not())
        .stdout(contains("vanilla_data_version_invalid").not())
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not());
}

#[test]
fn check_rejects_missing_vanilla_data_version() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoVersion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("reading vanilla sidecar version"));
}

#[test]
fn check_rejects_invalid_vanilla_data_version() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(vanilla_dir.path().join("version.json"), b"not json")
        .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarBadVersion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("parsing vanilla sidecar version"));
}

#[test]
fn check_rejects_non_utf8_vanilla_data_version() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(vanilla_dir.path().join("version.json"), [0xff, 0xfe, 0xfd])
        .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarUnreadableVersion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("parsing vanilla sidecar version"));
}

#[test]
fn check_rejects_vanilla_data_version_without_protocol() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","world_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::WORLD_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoProtocol"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("missing field `protocol_version`"));
}

#[test]
fn check_rejects_vanilla_data_version_without_id() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"world_version":{},"protocol_version":{}}}"#,
            mc_protocol::WORLD_VERSION,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoId"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("missing field `id`"));
}

#[test]
fn check_rejects_vanilla_data_version_without_world_version() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_dir = tempfile::tempdir().expect("vanilla tempdir");
    std::fs::write(
        vanilla_dir.path().join("version.json"),
        format!(
            r#"{{"id":"{}","protocol_version":{}}}"#,
            mc_protocol::TARGET_RELEASE,
            mc_protocol::PROTOCOL_VERSION
        ),
    )
    .expect("write version.json");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarNoWorldVersion"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("missing field `world_version`"));
}

#[test]
fn check_rejects_missing_vanilla_data_dir() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_parent = tempfile::tempdir().expect("vanilla parent tempdir");
    let vanilla_dir = vanilla_parent.path().join("missing-sidecar");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarMissingRoot"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("reading vanilla sidecar directory metadata"));
}

#[test]
fn check_rejects_vanilla_data_dir_file() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_file = NamedTempFile::new().expect("vanilla file");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarFileRoot"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_file.path().display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("data.vanilla_data_dir is not a directory"));
}

#[test]
fn check_rejects_vanilla_data_dir_parent_file() {
    let world_dir = tempfile::tempdir().expect("world tempdir");
    let vanilla_parent_file = NamedTempFile::new().expect("vanilla parent tempfile");
    let vanilla_dir = vanilla_parent_file.path().join("child-sidecar");
    let mut config_file = NamedTempFile::new().expect("config tempfile");
    let toml = format!(
        r#"
            [server]
            name = "SidecarBlockedParent"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            world_dir = "{}"
            vanilla_data_dir = "{}"
        "#,
        world_dir.path().display(),
        vanilla_dir.display()
    );
    config_file.write_all(toml.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(config_file.path())
        .assert()
        .failure()
        .stderr(contains("reading vanilla sidecar directory metadata"));
}

#[test]
fn check_missing_config_exits_nonzero_with_clear_error() {
    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg("/definitely/does/not/exist.toml")
        .assert()
        .failure()
        .stderr(contains("reading config file"));
}

#[test]
fn check_malformed_config_exits_nonzero_with_clear_error() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(b"this is not = valid [toml\n")
        .expect("write");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("parsing config file"));
}

#[test]
fn check_file_backed_access_control_is_loaded_and_missing_or_malformed_files_fail_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("server.toml");
    std::fs::write(
        &config_path,
        r#"
            [server]
            name = "AccessFiles"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [admin]
            operators_file = "ops.json"

            [auth]
            whitelist_enabled = true
            whitelist_file = "whitelist.json"
            banned_players_file = "banned-players.json"

            [data]
            world_dir = "world"
        "#,
    )
    .expect("write config");
    std::fs::write(
        temp.path().join("ops.json"),
        br#"[{"name":"FileOp","level":4}]"#,
    )
    .expect("write ops");
    std::fs::write(
        temp.path().join("whitelist.json"),
        br#"[{"name":"Allowed"}]"#,
    )
    .expect("write whitelist");
    std::fs::write(temp.path().join("banned-players.json"), b"[]").expect("write bans");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .success()
        .stdout(contains("fileop"))
        .stdout(contains("allowed"));

    std::fs::remove_file(temp.path().join("whitelist.json")).expect("remove whitelist");
    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("auth.whitelist_file"))
        .stderr(contains("whitelist.json"));

    std::fs::write(temp.path().join("whitelist.json"), b"{}").expect("write malformed whitelist");
    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("parsing auth.whitelist_file JSON"));

    std::fs::write(
        temp.path().join("whitelist.json"),
        br#"[{"uuid":"secret-raw-uuid"}]"#,
    )
    .expect("write invalid identity");
    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("auth.whitelist_file"))
        .stderr(contains("whitelist.json"))
        .stderr(contains("invalid uuid"))
        .stderr(contains("secret-raw-uuid").not());

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--config")
        .arg(&config_path)
        .assert()
        .failure()
        .stderr(contains("auth.whitelist_file"))
        .stderr(contains("whitelist.json"))
        .stderr(contains("invalid uuid"))
        .stderr(contains("secret-raw-uuid").not());
}

#[test]
fn check_invalid_bind_address_exits_nonzero_with_clear_error() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "InvalidBind"
            motd = "Hello"

            [network]
            bind_address = "not-an-ip"
            port = 30000
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("network.bind_address"))
        .stderr(contains("not-an-ip"));
}

#[test]
fn check_unknown_config_field_exits_nonzero_with_clear_error() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "TestServer"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [data]
            vanilla_dir = "data/vanilla"
        "#,
    )
    .expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("parsing config file"))
        .stderr(contains("unknown field `vanilla_dir`"));
}
