use super::*;

fn config() -> ServerConfig {
    toml::from_str(
        r#"
        [server]
        name = "Operators"
        motd = "Operators"
        [network]
        bind_address = "127.0.0.1"
        port = 0
        [admin]
        operators_file = "ops.json"
        allow_local_dev_operators = false
        "#,
    )
    .unwrap()
}

#[test]
fn removal_revokes_overlapping_profiles_without_orphaning_aliases() {
    let uuid = "a01e3843-e521-3998-958a-f459800e4d11";
    let other_uuid = "b50ad385-829d-3141-a216-7e7d7539ba7f";
    for identity in [" BUILDER ", uuid] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.toml");
        let operator_path = dir.path().join("ops.json");
        std::fs::write(
            &operator_path,
            serde_json::to_vec(&serde_json::json!([
                {"name": "Alias", "uuid": other_uuid},
                {"name": "Builder", "uuid": other_uuid},
                {"name": " builder ", "uuid": uuid.to_uppercase()},
                {"uuid": uuid},
                {"name": "Other", "level": 4, "note": "keep"}
            ]))
            .unwrap(),
        )
        .unwrap();
        // A prior unrelated edit must not split duplicate profiles into aliases.
        config()
            .manage_operator_file(&path, OperatorFileOperation::Add("FreshOp".into()))
            .unwrap();
        let removed = config()
            .manage_operator_file(&path, OperatorFileOperation::Remove(identity.into()))
            .unwrap();
        assert!(removed.changed);
        assert_eq!(removed.identities, ["freshop", "other"]);
        let mut reloaded = config();
        reloaded.load_access_control_files(&path).unwrap();
        assert_eq!(reloaded.admin.operators, removed.identities);
        let profiles: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(operator_path).unwrap()).unwrap();
        assert_eq!(
            profiles[0],
            serde_json::json!({"name": "other", "level": 4, "note": "keep"})
        );
    }
}

#[test]
fn oversized_update_preserves_previous_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    let profiles: Vec<_> = (0..MAX_ACCESS_CONTROL_FILE_ENTRIES)
        .map(|index| serde_json::json!({"name": format!("User{index:04}")}))
        .collect();
    let before = serde_json::to_vec(&profiles).unwrap();
    std::fs::write(&operator_path, &before).unwrap();
    assert!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::Add("ExtraUser".into()))
            .is_err()
    );
    assert_eq!(std::fs::read(&operator_path).unwrap(), before);
    config().load_access_control_files(&path).unwrap();
}

#[test]
fn pretty_print_overflow_preserves_previous_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    let mut before = serde_json::to_vec(&serde_json::json!([{"name": "Builder", "note": "x".repeat(MAX_ACCESS_CONTROL_FILE_BYTES as usize - 64)}])).unwrap();
    before.resize(MAX_ACCESS_CONTROL_FILE_BYTES as usize, b' ');
    std::fs::write(&operator_path, &before).unwrap();
    assert!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::Add("ExtraUser".into()))
            .is_err()
    );
    assert_eq!(std::fs::read(&operator_path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn replacement_preserves_permissions_and_ownership() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    std::fs::write(&operator_path, b"[]").unwrap();
    std::fs::set_permissions(&operator_path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let before = std::fs::metadata(&operator_path).unwrap();
    config()
        .manage_operator_file(&path, OperatorFileOperation::Add("Builder".into()))
        .unwrap();
    let after = std::fs::metadata(&operator_path).unwrap();
    assert_eq!(after.mode(), before.mode());
    assert_eq!((after.uid(), after.gid()), (before.uid(), before.gid()));
}

#[cfg(unix)]
#[test]
fn failed_readonly_update_does_not_replace_existing_json() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    let before = br#"[{"name":"Builder","level":4}]"#;
    std::fs::write(&operator_path, before).unwrap();
    std::fs::set_permissions(&operator_path, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::Add("Other".into()))
            .is_err()
    );
    assert_eq!(std::fs::read(&operator_path).unwrap(), before);
    assert_eq!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::List)
            .unwrap()
            .identities,
        ["builder"]
    );
}

#[cfg(unix)]
#[test]
fn management_refuses_symlinks_and_hardlinks_without_changing_referent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    let referent = dir.path().join("actual.json");
    let before = br#"[{"name":"Builder","level":4}]"#;
    std::fs::write(&referent, before).unwrap();
    std::os::unix::fs::symlink(&referent, &operator_path).unwrap();
    assert!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::Add("Other".into()))
            .is_err()
    );
    assert!(
        std::fs::symlink_metadata(&operator_path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&referent).unwrap(), before);
    std::fs::remove_file(&operator_path).unwrap();
    std::fs::hard_link(&referent, &operator_path).unwrap();
    assert!(
        config()
            .manage_operator_file(&path, OperatorFileOperation::Remove("Builder".into()))
            .is_err()
    );
    assert_eq!(std::fs::read(&operator_path).unwrap(), before);
}

/// Config with the login whitelist enforced and no whitelist file configured.
fn whitelist_config() -> ServerConfig {
    toml::from_str(
        r#"
        [server]
        name = "Whitelist"
        motd = "Whitelist"
        [network]
        bind_address = "127.0.0.1"
        port = 0
        [auth]
        online_mode = false
        whitelist_enabled = true
        "#,
    )
    .unwrap()
}

#[test]
fn whitelist_entries_persist_through_the_default_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let whitelist_path = dir.path().join("whitelist.json");
    let mut config = whitelist_config();
    assert!(config.auth.whitelist_file.is_none());

    // The console supplies the default path in memory when the config omits it.
    config.auth.whitelist_file = Some(PathBuf::from("whitelist.json"));
    config.add_whitelist_identity(&path, " Builder ").unwrap();
    config.add_whitelist_identity(&path, "Alias").unwrap();
    config.remove_whitelist_identity(&path, "ALIAS").unwrap();

    let profiles: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&whitelist_path).unwrap()).unwrap();
    assert_eq!(profiles, [serde_json::json!({"name": "builder"})]);

    // A restart that leaves `auth.whitelist_file` unset still loads that file,
    // so console entries survive without a config edit.
    let mut reloaded = whitelist_config();
    reloaded.load_access_control_files(&path).unwrap();
    assert_eq!(reloaded.auth.whitelist, ["builder"]);
}

#[test]
fn whitelist_management_requires_a_configured_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.toml");
    let error = whitelist_config()
        .add_whitelist_identity(&path, "Builder")
        .unwrap_err();
    assert!(
        error.to_string().contains("auth.whitelist_file"),
        "unexpected error: {error}"
    );
}

#[test]
fn file_backed_access_control_resolves_relative_paths_and_merges_identities() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    std::fs::write(
        temp.path().join("ops.json"),
        r#"[
            {"name":"FileOp","uuid":"11111111-1111-1111-1111-111111111111","level":4,"bypassesPlayerLimit":false},
            {"name":"SecondOp"}
        ]"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("whitelist.json"),
        r#"[{"name":"Allowed","uuid":"22222222-2222-2222-2222-222222222222"}]"#,
    )
    .unwrap();
    std::fs::write(
        temp.path().join("banned-players.json"),
        r#"[{"name":"Banned","uuid":"33333333-3333-3333-3333-333333333333","reason":"test"}]"#,
    )
    .unwrap();

    let mut cfg: ServerConfig = toml::from_str(
        r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [admin]
            operators = ["InlineOp"]
            operators_file = "ops.json"

            [auth]
            whitelist_enabled = true
            whitelist = ["InlineAllowed"]
            whitelist_file = "whitelist.json"
            banned_players = ["InlineBan"]
            banned_players_file = "banned-players.json"
        "#,
    )
    .unwrap();

    let report = cfg.load_access_control_files(&config_path).unwrap();
    assert_eq!(report.files_loaded, 3);
    assert_eq!(report.operator_identities, 3);
    assert_eq!(report.whitelist_identities, 2);
    assert_eq!(report.banned_identities, 2);
    assert!(cfg.admin.operators.iter().any(|entry| entry == "InlineOp"));
    assert!(cfg.admin.operators.iter().any(|entry| entry == "fileop"));
    assert!(
        cfg.admin
            .operators
            .iter()
            .any(|entry| entry == "11111111-1111-1111-1111-111111111111")
    );
    assert!(cfg.auth.whitelist.iter().any(|entry| entry == "allowed"));
    assert!(
        cfg.auth
            .whitelist
            .iter()
            .any(|entry| entry == "22222222-2222-2222-2222-222222222222")
    );
    assert!(
        cfg.auth
            .banned_players
            .iter()
            .any(|entry| entry == "banned")
    );
    assert!(
        cfg.auth
            .banned_players
            .iter()
            .any(|entry| entry == "33333333-3333-3333-3333-333333333333")
    );
}

#[test]
fn file_backed_access_control_fails_closed_for_bad_files() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    let access_path = temp.path().join("whitelist.json");
    let config_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [auth]
        whitelist_enabled = true
        whitelist_file = "whitelist.json"
    "#;

    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    assert!(cfg.load_access_control_files(&config_path).is_err());

    std::fs::write(&access_path, b"{}").unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    assert!(cfg.load_access_control_files(&config_path).is_err());

    std::fs::write(&access_path, br#"[{}]"#).unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    let error = cfg
        .load_access_control_files(&config_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("must contain name and/or uuid"));
    assert!(error.contains(&access_path.display().to_string()));

    std::fs::write(&access_path, br#"[{"name":"bad name"}]"#).unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    let error = cfg
        .load_access_control_files(&config_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid Minecraft username"));
    assert!(error.contains(&access_path.display().to_string()));
    assert!(!error.contains("bad name"));

    std::fs::write(&access_path, br#"[{"uuid":"not-a-uuid"}]"#).unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    let error = cfg
        .load_access_control_files(&config_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid uuid"));
    assert!(error.contains(&access_path.display().to_string()));
    assert!(!error.contains("not-a-uuid"));

    let too_many = (0..=MAX_ACCESS_CONTROL_FILE_ENTRIES)
        .map(|index| format!(r#"{{"name":"User{index:04}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(&access_path, format!("[{too_many}]")).unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    let error = cfg
        .load_access_control_files(&config_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("maximum is 4096"));
    assert!(error.contains(&access_path.display().to_string()));

    std::fs::write(
        &access_path,
        vec![b' '; usize::try_from(MAX_ACCESS_CONTROL_FILE_BYTES).unwrap() + 1],
    )
    .unwrap();
    let mut cfg: ServerConfig = toml::from_str(config_src).unwrap();
    assert!(
        cfg.load_access_control_files(&config_path)
            .unwrap_err()
            .to_string()
            .contains("exceeds")
    );
}

#[test]
fn file_backed_access_control_reloads_on_fresh_server_config() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    let whitelist_path = temp.path().join("whitelist.json");
    let config_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565

        [auth]
        whitelist_enabled = true
        whitelist_file = "whitelist.json"
    "#;

    std::fs::write(&whitelist_path, br#"[{"name":"FirstUser"}]"#).unwrap();
    let mut first: ServerConfig = toml::from_str(config_src).unwrap();
    first.load_access_control_files(&config_path).unwrap();
    assert!(
        first
            .auth
            .whitelist
            .iter()
            .any(|entry| entry == "firstuser")
    );

    std::fs::write(&whitelist_path, br#"[{"name":"SecondUser"}]"#).unwrap();
    let mut restarted: ServerConfig = toml::from_str(config_src).unwrap();
    restarted.load_access_control_files(&config_path).unwrap();
    assert!(
        !restarted
            .auth
            .whitelist
            .iter()
            .any(|entry| entry == "firstuser")
    );
    assert!(
        restarted
            .auth
            .whitelist
            .iter()
            .any(|entry| entry == "seconduser")
    );
}

#[test]
fn default_operator_file_loads_for_fresh_server_config() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("server.toml");
    let config_src = r#"
        [server]
        name = "S"
        motd = "M"

        [network]
        bind_address = "127.0.0.1"
        port = 25565
    "#;
    let mut configured: ServerConfig = toml::from_str(config_src).unwrap();
    configured.admin.operators_file = Some(PathBuf::from("ops.json"));
    configured
        .manage_operator_file(
            &config_path,
            OperatorFileOperation::Add("FreshOp".to_owned()),
        )
        .unwrap();

    let mut restarted: ServerConfig = toml::from_str(config_src).unwrap();
    let report = restarted.load_access_control_files(&config_path).unwrap();

    assert_eq!(report.operator_identities, 1);
    assert_eq!(restarted.admin.operators, vec!["freshop".to_owned()]);
}

#[test]
fn operator_file_mutations_persist_and_deduplicate() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    let operator_path = dir.path().join("ops.json");
    std::fs::write(
        &operator_path,
        r#"[{"name":"Builder","level":4},{"name":"builder"},{"name":"Other","note":"keep"}]"#,
    )
    .unwrap();
    let config: ServerConfig = toml::from_str(
        r#"
            [server]
            name = "Test"
            motd = "Test"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
            [admin]
            operators_file = "ops.json"
        "#,
    )
    .unwrap();

    let listed = config
        .manage_operator_file(&config_path, OperatorFileOperation::List)
        .unwrap();
    assert_eq!(listed.identities, vec!["builder", "other"]);
    let added = config
        .manage_operator_file(&config_path, OperatorFileOperation::Add("Alice".to_owned()))
        .unwrap();
    assert!(added.changed);
    let persisted = std::fs::read_to_string(&operator_path).unwrap();
    assert!(persisted.contains(r#""level": 4"#));
    assert!(persisted.contains(r#""note": "keep""#));
    let removed = config
        .manage_operator_file(
            &config_path,
            OperatorFileOperation::Remove("builder".to_owned()),
        )
        .unwrap();
    assert!(removed.changed);
    assert!(
        !std::fs::read_to_string(&operator_path)
            .unwrap()
            .contains("builder")
    );
}

#[test]
fn invalid_operator_add_has_no_file_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("server.toml");
    let config: ServerConfig = toml::from_str(
        r#"
            [server]
            name = "Test"
            motd = "Test"
            [network]
            bind_address = "127.0.0.1"
            port = 25565
        "#,
    )
    .unwrap();
    let config = ServerConfig {
        admin: AdminSection {
            operators_file: Some(PathBuf::from("ops.json")),
            ..config.admin
        },
        ..config
    };

    let error = config
        .manage_operator_file(&config_path, OperatorFileOperation::Add("no".to_owned()))
        .unwrap_err();

    assert!(error.to_string().contains("invalid operator identity"));
    assert!(!dir.path().join("ops.json").exists());
}
