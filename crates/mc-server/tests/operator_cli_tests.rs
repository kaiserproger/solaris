use std::process::{Command, Stdio};

#[test]
fn concurrent_operator_managers_preserve_all_updates_with_relative_config_path() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"
        [server]
        name = "Concurrent operators"
        motd = "Concurrent operators"
        [network]
        bind_address = "127.0.0.1"
        port = 0
        [admin]
        operators_file = "ops.json"
        allow_local_dev_operators = false
    "#;
    std::fs::write(dir.path().join("server.toml"), source).unwrap();
    // Queue independent CLI processes behind the same stable sidecar. The
    // target deliberately does not exist until the first add commits.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(dir.path().join("ops.json.lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut children = Vec::new();
    for index in 0..24 {
        children.push(
            Command::new(env!("CARGO_BIN_EXE_mc-server"))
                .current_dir(dir.path())
                .args([
                    "--config",
                    "server.toml",
                    "operator",
                    "add",
                    &format!("Worker{index:02}"),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    drop(lock);
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut reloaded: mc_server::ServerConfig = toml::from_str(source).unwrap();
    reloaded
        .load_access_control_files(&dir.path().join("server.toml"))
        .unwrap();
    let expected: Vec<_> = (0..24).map(|index| format!("worker{index:02}")).collect();
    assert_eq!(reloaded.admin.operators, expected);
}
