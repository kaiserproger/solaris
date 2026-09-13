use super::*;

fn authoring_package(source: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let plugin = root.path().join("hello");
    fs::create_dir(&plugin).unwrap();
    fs::write(
        plugin.join("plugin.toml"),
        "id='hello'\nname='Hello'\nversion='0.1.0'\napi='0.6.0'\nplayer_commands=['hello']\n",
    )
    .unwrap();
    fs::write(plugin.join("main.lua"), source).unwrap();
    root
}

#[test]
fn strict_discovery_rejects_host_api_errors_in_uninvoked_handlers() {
    for call in [
        "solaris.send_messsage(event.player_id, 'hello')",
        "solaris.send_message({}, 'hello')",
        "solaris.send_message(event.player_id)",
        "solaris.inventory_transaction(event.player_id, 'buy', {{resource='minecraft:apple'}})",
        "local result = solaris.storage_get('read', 'balance'); print(result.value)",
    ] {
        let root = authoring_package(&format!(
            "function on_player_command(event: any)\n    {call}\nend\n"
        ));
        let result = prepare_lua_plugins(LuaHostConfig::new(root.path()).strict_discovery(true));
        assert!(
            matches!(result, Err(LuaHostError::InvalidStartupPlugin { path, .. })
                if path == root.path().join("hello")),
            "invalid future handler passed discovery: {call}"
        );
    }
}

#[test]
fn typed_command_and_optional_arguments_load_without_runtime_declarations() {
    let root = authoring_package(
        r#"--!strict
function on_player_command(event: SolarisPlayerCommandEvent)
    solaris.send_message(event.player_id, "Hello, " .. event.username .. "!")
    solaris.list_online_players("online")
    solaris.query_owned_inventory("inventory", {kind="player_inventory", player_id=event.player_id})
    solaris.storage_cas("save", "balance", nil, "1")
end
"#,
    );
    let prepared = prepare_lua_plugins(LuaHostConfig::new(root.path()).strict_discovery(true))
        .expect("valid typed host calls must pass discovery");
    assert_eq!(prepared.discovered_plugins().len(), 1);
}
