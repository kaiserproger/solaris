use super::*;

#[tokio::test]
async fn operator_removal_revokes_name_and_uuid_permissions_after_reload() {
    let paired_uuid = mc_net::offline_uuid("UuidOp");
    for identity in [
        " NameOp ".to_owned(),
        paired_uuid.simple().to_string().to_uppercase(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("server.toml");
        let source = r#"
            [server]
            name = "Operator removal"
            motd = "Operator removal"
            [network]
            bind_address = "127.0.0.1"
            port = 0
            [admin]
            operators_file = "ops.json"
            allow_local_dev_operators = false
        "#;
        std::fs::write(&config_path, source).unwrap();
        std::fs::write(
            dir.path().join("ops.json"),
            serde_json::to_vec(&serde_json::json!([
                {"name": "NameOp", "uuid": paired_uuid.to_string(), "level": 4},
                {"name": "OtherOp", "note": "keep"}
            ]))
            .unwrap(),
        )
        .unwrap();
        let config: mc_server::ServerConfig = toml::from_str(source).unwrap();
        let before = start_server_from_access_file_config(&config_path).await;
        // Separate clients exercise name-only and UUID-only matching inside
        // CommandPermissionConfig::is_operator, not just the config's strings.
        for name in ["NameOp", "UuidOp", "OtherOp"] {
            let roots =
                tokio::time::timeout(Duration::from_secs(5), command_roots_for(before, name))
                    .await
                    .unwrap();
            assert!(
                roots.iter().any(|root| root == "time"),
                "{name} must initially be an operator"
            );
        }
        let removed = config
            .manage_operator_file(
                &config_path,
                mc_server::OperatorFileOperation::Remove(identity),
            )
            .unwrap();
        assert!(removed.changed);
        assert_eq!(removed.identities, ["otherop"]);
        let after = start_server_from_access_file_config(&config_path).await;
        for name in ["NameOp", "UuidOp"] {
            let roots =
                tokio::time::timeout(Duration::from_secs(5), command_roots_for(after, name))
                    .await
                    .unwrap();
            assert!(
                !roots.iter().any(|root| root == "time"),
                "removed {name} retained operator commands"
            );
        }
        let roots =
            tokio::time::timeout(Duration::from_secs(5), command_roots_for(after, "OtherOp"))
                .await
                .unwrap();
        assert!(roots.iter().any(|root| root == "time"));
    }
}
