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
