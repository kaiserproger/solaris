//! Integration tests for the `mc-server` binary's CLI surface.
//!
//! These cover only the synchronous `--check` path (parse a config, print
//! it, exit). The end-to-end "actually serve a connection" test lives in
//! `tests/status.rs` because it needs tokio and a real socket.

use std::io::Write;

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
"#;

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
fn check_prints_effective_thread_minimums() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(
        br#"
            [server]
            name = "TestServer"
            motd = "Hello"

            [network]
            bind_address = "127.0.0.1"
            port = 30000

            [chunk_pipeline]
            chunk_io_threads_percent = 0
            chunk_worker_threads_percent = 0
            entity_worker_threads_percent = 0
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
        .stdout(contains("\"chunk_io_threads\": 1"))
        .stdout(contains("\"chunk_worker_threads\": 1"))
        .stdout(contains("\"entity_worker_threads\": 1"));
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
fn check_reports_missing_world_dir_warning() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(SAMPLE_TOML.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--check")
        .arg("--config")
        .arg(file.path())
        .assert()
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("missing_world_dir"));
}

#[test]
fn check_reports_world_dir_file_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("world_dir_not_directory"));
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
fn check_reports_vanilla_data_protocol_mismatch_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_protocol_mismatch"));
}

#[test]
fn check_reports_vanilla_data_release_mismatch_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_release_mismatch"))
        .stdout(contains("vanilla_data_world_version_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not());
}

#[test]
fn check_reports_vanilla_data_world_version_mismatch_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_world_version_mismatch"))
        .stdout(contains("vanilla_data_release_mismatch").not())
        .stdout(contains("vanilla_data_protocol_mismatch").not());
}

#[test]
fn check_reports_missing_vanilla_data_version_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_missing"));
}

#[test]
fn check_reports_invalid_vanilla_data_version_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_invalid"));
}

#[test]
fn check_reports_unreadable_vanilla_data_version_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_invalid"))
        .stdout(contains("vanilla_data_version_missing").not());
}

#[test]
fn check_reports_vanilla_data_version_without_protocol_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_invalid"));
}

#[test]
fn check_reports_vanilla_data_version_without_id_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_invalid"));
}

#[test]
fn check_reports_vanilla_data_version_without_world_version_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_version_invalid"));
}

#[test]
fn check_reports_missing_vanilla_data_dir_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_dir_missing_on_disk"))
        .stdout(contains("vanilla_data_version_missing").not());
}

#[test]
fn check_reports_vanilla_data_dir_file_warning() {
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
        .success()
        .stdout(contains("\"operator_warnings\""))
        .stdout(contains("vanilla_data_dir_not_directory"))
        .stdout(contains("vanilla_data_version_missing").not());
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
