use std::num::NonZeroUsize;

use super::player_query::{PlayerQueryAdapterError, PluginPlayerQueryAdapter};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;
use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind, start_lua_host,
};

async fn admitted_query() -> AdmittedScriptCommand {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("who");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "who"
name = "Who test"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["player_queries"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"function on_server_started(_event)
    solaris.list_online_players("who-now", 8)
end
"#,
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
    admitted
}

#[tokio::test]
async fn player_query_adapter_publishes_authoritative_targeted_snapshot() {
    let registry = SessionRegistry::new();
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginPlayerQueryAdapter::new(ScriptEventSink::new(boundary));
    assert_eq!(
        adapter
            .route_admitted(admitted_query().await, &registry)
            .await,
        Ok(())
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::OnlinePlayersResult { request_id, players, truncated }
            if request_id == "who-now"
                && players.is_empty()
                && !truncated
    ));
}

#[tokio::test]
async fn player_query_adapter_reports_closed_targeted_delivery() {
    let registry = SessionRegistry::new();
    let (boundary, events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginPlayerQueryAdapter::new(ScriptEventSink::new(boundary));
    drop(events);

    assert_eq!(
        adapter
            .route_admitted(admitted_query().await, &registry)
            .await,
        Err(PlayerQueryAdapterError::PublicationClosed)
    );
}
