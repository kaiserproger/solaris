use std::num::NonZeroUsize;

use super::teleport::PluginTeleportAdapter;
use crate::server::ScriptEventSink;
use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind,
    ScriptPlayerTeleportFailure, start_lua_host,
};

async fn admitted_teleport_commands(
    plugin_id: &str,
    commands: &str,
    command_count: usize,
) -> Vec<AdmittedScriptCommand> {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join(plugin_id);
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!(
            r#"id = "{plugin_id}"
name = "Teleport test"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["player_teleport"]
"#,
        ),
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
{commands}
end
"#,
        ),
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let mut admitted = Vec::with_capacity(command_count);
    for _ in 0..command_count {
        let command = boundary.recv_command().await.unwrap();
        admitted.push(boundary.accept_host_command(command).unwrap());
    }
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
    admitted
}

#[tokio::test]
async fn teleport_adapter_publishes_exact_unavailable_result() {
    let mut commands = admitted_teleport_commands(
        "warps",
        r#"    solaris.teleport_player("offline", 77, 1, 65, 1)"#,
        1,
    )
    .await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginTeleportAdapter::new(ScriptEventSink::new(boundary));
    let sessions = crate::play::SessionRegistry::new();
    assert_eq!(
        adapter.route_admitted(commands.remove(0), &sessions).await,
        Ok(())
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerTeleportResult {
            request_id,
            failure: Some(ScriptPlayerTeleportFailure::PlayerUnavailable),
            ..
        } if request_id == "offline"
    ));
}
