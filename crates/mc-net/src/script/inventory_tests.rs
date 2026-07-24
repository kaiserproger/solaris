use std::num::NonZeroUsize;

use super::inventory::{InventoryAdapterError, PluginInventoryAdapter};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;
use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind,
    ScriptPlayerInventoryFailure, start_lua_host,
};

async fn admitted_inventory_command(player_id: u64) -> AdmittedScriptCommand {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("kits");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "kits"
name = "Kits"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["player_inventory"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
    solaris.inventory_transaction({player_id}, "grant", {{
        {{ resource = "minecraft:apple", delta = 2 }},
    }})
end
"#
        ),
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
async fn inventory_adapter_publishes_exact_unavailable_result() {
    let admitted = admitted_inventory_command(77).await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginInventoryAdapter::new(ScriptEventSink::new(boundary));

    assert_eq!(
        adapter
            .route_admitted(admitted, &SessionRegistry::new(), true,)
            .await,
        Ok(())
    );
    let event = events.recv_event().await.unwrap();
    assert_eq!(event.target_plugin_id(), Some("kits"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::PlayerInventoryTransactionResult {
            request_id,
            player_id,
            failure: Some(ScriptPlayerInventoryFailure::PlayerUnavailable),
        } if request_id == "grant" && player_id.value() == 77
    ));
}

#[tokio::test]
async fn inventory_adapter_rejects_unavailable_inventory_runtime_before_session_lookup() {
    let admitted = admitted_inventory_command(77).await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginInventoryAdapter::new(ScriptEventSink::new(boundary));

    assert_eq!(
        adapter
            .route_admitted(admitted, &SessionRegistry::new(), false,)
            .await,
        Ok(())
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerInventoryTransactionResult {
            failure: Some(ScriptPlayerInventoryFailure::RuntimeUnavailable),
            ..
        }
    ));
}

#[tokio::test]
async fn closed_result_channel_stops_inventory_adapter() {
    let admitted = admitted_inventory_command(77).await;
    let (boundary, events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    drop(events);
    let adapter = PluginInventoryAdapter::new(ScriptEventSink::new(boundary));

    assert_eq!(
        adapter
            .route_admitted(admitted, &SessionRegistry::new(), true,)
            .await,
        Err(InventoryAdapterError::PublicationClosed)
    );
}
