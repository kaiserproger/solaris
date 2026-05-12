//! Integration test for the `mc-server` binary's M0 CLI.

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
fn prints_parsed_config_and_exits_zero() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(SAMPLE_TOML.as_bytes()).expect("write toml");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
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
fn missing_config_exits_nonzero_with_clear_error() {
    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--config")
        .arg("/definitely/does/not/exist.toml")
        .assert()
        .failure()
        .stderr(contains("reading config file"));
}

#[test]
fn malformed_config_exits_nonzero_with_clear_error() {
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(b"this is not = valid [toml\n")
        .expect("write");

    Command::cargo_bin("mc-server")
        .expect("locate mc-server binary")
        .arg("--config")
        .arg(file.path())
        .assert()
        .failure()
        .stderr(contains("parsing config file"));
}
