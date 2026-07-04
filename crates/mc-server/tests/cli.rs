//! Integration tests for the `mc-server` binary's CLI surface.
//!
//! These cover only the synchronous `--check` path (parse a config, print
//! it, exit). The end-to-end "actually serve a connection" test lives in
//! `tests/status.rs` because it needs tokio and a real socket.

use std::io::Write;

use assert_cmd::Command;
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
